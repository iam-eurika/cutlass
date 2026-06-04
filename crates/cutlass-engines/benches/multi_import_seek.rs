//! Multi-import + random-seek benchmark for the proxy tier (P2).
//!
//! Models dropping several clips on the timeline at once and then scrubbing
//! around randomly across all of them. It exercises the two things P2 added on
//! top of the single-clip path:
//!
//! 1. **Parallel builds across lanes.** All clips are imported into one
//!    [`MediaPool`]; the SW + HW lanes drain the shared queue concurrently. We
//!    report each clip's ready-time offset and the total wall — the wall should
//!    track the *busiest lane*, not the sum of all builds.
//! 2. **Random cold seeks** over a fixed, shuffled (clip, timestamp) sequence,
//!    source vs the engine-built proxies. Random access is the realistic scrub
//!    pattern (every jump is a fresh keyframe hunt), and it's where the long-GOP
//!    source hurts most.
//!
//! Asset selection (in order): `CUTLASS_BENCH_ASSETS` (comma/colon list) → the
//! smallest `CUTLASS_BENCH_N` (default 4) 4K clips under `assets/` → any `.mp4`
//! there → three copies of the sibling decoder fixture (so CI still runs).
//!
//! ```bash
//! CUTLASS_BENCH_N=4 cargo bench -p cutlass-engines --bench multi_import_seek
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};

use cutlass_decode::{DecodeOptions, Decoder, HwAccel};
use cutlass_engines::{proxy_path, MediaPool, ProxyStatus};
use cutlass_models::{MediaSource, Rational};

const PROXY_HEIGHT: u32 = 1080;
const SEEK_SEQUENCE_LEN: usize = 48;

/// One clip prepared for the bench: source + proxy paths and probed timing.
struct Clip {
    name: String,
    media: MediaSource,
    proxy: PathBuf,
    secs: f64,
}

/// Deterministic LCG so the random seek sequence is identical across runs and
/// between the source and proxy benches (apples-to-apples).
fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

fn unit(state: &mut u64) -> f64 {
    (lcg(state) >> 11) as f64 / (1u64 << 53) as f64
}

/// Pick the clip source files. Returns the chosen paths plus any temp copies that
/// must be cleaned up afterward.
fn choose_assets() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let n: usize = std::env::var("CUTLASS_BENCH_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);

    if let Some(list) = std::env::var_os("CUTLASS_BENCH_ASSETS") {
        let paths: Vec<PathBuf> = list
            .to_string_lossy()
            .split([',', ':'])
            .map(|s| PathBuf::from(s.trim()))
            .filter(|p| p.exists())
            .collect();
        if !paths.is_empty() {
            return (paths, Vec::new());
        }
    }

    let assets_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    if let Ok(entries) = std::fs::read_dir(&assets_dir) {
        let mut mp4s: Vec<(PathBuf, u64)> = entries
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("mp4") {
                    return None;
                }
                let len = e.metadata().ok()?.len();
                Some((p, len))
            })
            .collect();
        // Prefer 4K clips (named with resolution); smallest first for fast builds.
        let mut four_k: Vec<(PathBuf, u64)> = mp4s
            .iter()
            .filter(|(p, _)| p.to_string_lossy().contains("3840_2160"))
            .cloned()
            .collect();
        let pool = if four_k.len() >= 2 { &mut four_k } else { &mut mp4s };
        pool.sort_by_key(|(_, len)| *len);
        let chosen: Vec<PathBuf> = pool.iter().take(n).map(|(p, _)| p.clone()).collect();
        if chosen.len() >= 2 {
            return (chosen, Vec::new());
        }
    }

    // CI fallback: clone the sibling fixture a few times for distinct sources.
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../cutlass-main/crates/decoder/tests/assets/testsrc_h264.mp4");
    if fixture.exists() {
        let mut paths = Vec::new();
        let mut temps = Vec::new();
        for i in 0..n.max(2) {
            let dst = std::env::temp_dir().join(format!("cutlass_multi_fixture_{i}.mp4"));
            if std::fs::copy(&fixture, &dst).is_ok() {
                paths.push(dst.clone());
                temps.push(dst);
            }
        }
        return (paths, temps);
    }

    (Vec::new(), Vec::new())
}

/// Probe a source into a [`Clip`] (dimensions, fps, duration).
fn probe(path: PathBuf) -> Option<Clip> {
    let dec = Decoder::open(&path).ok()?;
    let secs = dec.duration().unwrap_or(Duration::from_secs(5)).as_secs_f64();
    let (num, den) = dec.info().frame_rate_parts();
    let (w, h) = (dec.info().width, dec.info().height);
    drop(dec);
    let fps = if den != 0 { num as f64 / den as f64 } else { 30.0 };
    let total_frames = ((secs * fps).round() as i64).max(1);
    let media = MediaSource::new(
        path.clone(),
        w,
        h,
        Rational::new(num.max(1), den.max(1)),
        total_frames,
        false,
    );
    let proxy = proxy_path(&path, PROXY_HEIGHT);
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(Clip {
        name,
        media,
        proxy,
        secs,
    })
}

/// Build one clip alone (fresh pool) and return its wall time. With a single
/// queued job only one lane runs, so this is the clip's true build duration.
fn build_one(clip: &Clip) -> Option<f64> {
    let mut pool = MediaPool::new();
    pool.open(&clip.media).expect("open source");
    let _ = std::fs::remove_file(&clip.proxy);
    let start = Instant::now();
    pool.request_proxy(&clip.media);
    let deadline = start + Duration::from_secs(300);
    loop {
        pool.poll_proxies();
        match pool.proxy_status(clip.media.id) {
            Some(ProxyStatus::Ready(_)) => return Some(start.elapsed().as_secs_f64()),
            Some(ProxyStatus::Failed(e)) => {
                eprintln!("  serial build failed for {}: {e}", clip.name);
                return None;
            }
            _ => {}
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Serial baseline: sum of each clip's solo build time (one lane at a time).
fn build_serial(clips: &[Clip]) -> f64 {
    let mut total = 0.0;
    for c in clips {
        if let Some(t) = build_one(c) {
            total += t;
            eprintln!("  solo {t:6.2}s  {}", c.name);
        }
    }
    total
}

/// Import all clips into one pool and build their proxies in parallel across the
/// lanes; report per-clip ready offsets and the total wall. Returns
/// `(all_ok, wall_seconds)`.
fn build_parallel(clips: &[Clip]) -> (bool, f64) {
    let mut pool = MediaPool::new();
    for c in clips {
        pool.open(&c.media).expect("open source");
        let _ = std::fs::remove_file(&c.proxy); // time a real build, not adoption
    }

    let start = Instant::now();
    for c in clips {
        pool.request_proxy(&c.media);
    }

    let mut done_at: Vec<Option<f64>> = vec![None; clips.len()];
    let deadline = start + Duration::from_secs(600);
    loop {
        pool.poll_proxies();
        for (i, c) in clips.iter().enumerate() {
            if done_at[i].is_some() {
                continue;
            }
            match pool.proxy_status(c.media.id) {
                Some(ProxyStatus::Ready(_)) => done_at[i] = Some(start.elapsed().as_secs_f64()),
                Some(ProxyStatus::Failed(e)) => {
                    eprintln!("  proxy build failed for {}: {e}", c.name);
                    done_at[i] = Some(-1.0);
                }
                _ => {}
            }
        }
        if done_at.iter().all(Option::is_some) {
            break;
        }
        if Instant::now() >= deadline {
            eprintln!("  multi-import build timed out");
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    let wall = start.elapsed().as_secs_f64();
    for (i, c) in clips.iter().enumerate() {
        match done_at[i] {
            Some(t) if t >= 0.0 => eprintln!("  ready @ {t:6.2}s  {}", c.name),
            _ => eprintln!("  FAILED          {}", c.name),
        }
    }
    let all_ok = done_at.iter().all(|t| matches!(t, Some(v) if *v >= 0.0));
    (all_ok, wall)
}

/// A fixed, shuffled list of `(clip_index, timestamp)` jumps.
fn seek_sequence(clips: &[Clip]) -> Vec<(usize, Duration)> {
    let mut state: u64 = 0x9E3779B97F4A7C15;
    (0..SEEK_SEQUENCE_LEN)
        .map(|_| {
            let idx = (lcg(&mut state) as usize) % clips.len();
            let frac = 0.05 + 0.90 * unit(&mut state);
            (idx, Duration::from_secs_f64(clips[idx].secs * frac))
        })
        .collect()
}

fn bench_multi_import_seek(c: &mut Criterion) {
    let (paths, temps) = choose_assets();
    if paths.len() < 2 {
        eprintln!(
            "skipping multi_import_seek: need >=2 assets (set CUTLASS_BENCH_ASSETS, or place \
             clips under assets/)"
        );
        return;
    }

    let clips: Vec<Clip> = paths.into_iter().filter_map(probe).collect();
    if clips.len() < 2 {
        eprintln!("skipping multi_import_seek: fewer than 2 probeable clips");
        for t in &temps {
            let _ = std::fs::remove_file(t);
        }
        return;
    }

    eprintln!("multi_import_seek clips:");
    for c in &clips {
        let mb = std::fs::metadata(&c.media.path)
            .map(|m| m.len())
            .unwrap_or(0) as f64
            / 1e6;
        eprintln!(
            "  {} ({}x{}, {:.1}s, {:.0} MB)",
            c.name, c.media.width, c.media.height, c.secs, mb
        );
    }

    // Serial baseline first (one lane at a time), then the parallel multi-import
    // (all lanes). Parallel runs last so every proxy is freshly on disk for the
    // seek bench below.
    eprintln!("serial baseline (one clip at a time):");
    let serial = build_serial(&clips);
    eprintln!("parallel multi-import (all lanes):");
    let (built, wall) = build_parallel(&clips);
    eprintln!(
        "multi-import: {} clips  serial {:.2}s vs parallel {:.2}s  => {:.2}x lane speedup",
        clips.len(),
        serial,
        wall,
        if wall > 0.0 { serial / wall } else { 1.0 }
    );

    let seq = seek_sequence(&clips);

    let mut group = c.benchmark_group("multi_random_seek");
    group.sample_size(10);

    // The live path before proxies exist: original sources, hardware-auto.
    {
        let cursor = std::cell::Cell::new(0usize);
        group.bench_function(BenchmarkId::new("source_auto", "random"), |b| {
            b.iter_batched_ref(
                || {
                    let i = cursor.get();
                    cursor.set(i + 1);
                    let (clip, target) = seq[i % seq.len()];
                    let dec = Decoder::open_with(&clips[clip].media.path, DecodeOptions::default())
                        .expect("open source");
                    (dec, target)
                },
                |(dec, target)| {
                    let frame = dec.seek_to_frame(*target).expect("seek").expect("frame");
                    criterion::black_box(frame);
                },
                BatchSize::PerIteration,
            );
        });
    }

    // The path after the wait: all-intra proxies, software (as the pool reads them).
    if built {
        let cursor = std::cell::Cell::new(0usize);
        group.bench_function(BenchmarkId::new("proxy_sw", "random"), |b| {
            b.iter_batched_ref(
                || {
                    let i = cursor.get();
                    cursor.set(i + 1);
                    let (clip, target) = seq[i % seq.len()];
                    let dec = Decoder::open_with(
                        &clips[clip].proxy,
                        DecodeOptions::default().hw_accel(HwAccel::None),
                    )
                    .expect("open proxy");
                    (dec, target)
                },
                |(dec, target)| {
                    let frame = dec.seek_to_frame(*target).expect("seek").expect("frame");
                    criterion::black_box(frame);
                },
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();

    for t in &temps {
        let _ = std::fs::remove_file(t);
    }
}

criterion_group!(benches, bench_multi_import_seek);
criterion_main!(benches);
