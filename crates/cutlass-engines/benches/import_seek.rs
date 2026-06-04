//! End-to-end "import → wait → seek" benchmark for the proxy tier (P2).
//!
//! Models what a user does after dropping a clip on the timeline: import kicks
//! off a background proxy build (the P2 lane pool), and a moment later they scrub
//! to some point. This bench:
//!
//! 1. Builds the proxy through the real [`MediaPool`] path (lanes + completion
//!    pump) and reports the wall time — the "little wait" before scrubbing.
//! 2. Compares **cold seek** latency on the original source (the live path used
//!    before a proxy exists) vs the engine-built all-intra proxy (the path used
//!    after the wait). Each sample re-opens the decoder, so every seek is genuinely
//!    cold — no RAM-cache hits hiding the seek cost.
//!
//! Source is read with the app's default (hardware-auto) decode; the proxy is
//! read in software, exactly as [`MediaPool`] reads each tier.
//!
//! Defaults to a bundled asset; point `CUTLASS_BENCH_ASSET` at a representative
//! long-GOP clip (e.g. 4K60) for a meaningful number:
//!
//! ```bash
//! CUTLASS_BENCH_ASSET="$PWD/assets/16078825_3840_2160_60fps.mp4" \
//!   cargo bench -p cutlass-engines --bench import_seek
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};

use cutlass_decode::{DecodeOptions, Decoder, HwAccel};
use cutlass_engines::{proxy_path, MediaPool, ProxyStatus};
use cutlass_models::{MediaSource, Rational};

/// Resolve the asset: `CUTLASS_BENCH_ASSET`, else the largest bundled clip, else
/// the sibling decoder fixture.
fn asset_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CUTLASS_BENCH_ASSET") {
        let path = PathBuf::from(p);
        return path.exists().then_some(path);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest.join("../../assets/16078825_3840_2160_60fps.mp4"),
        manifest.join("../../../cutlass-main/crates/decoder/tests/assets/testsrc_h264.mp4"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

/// Build the proxy through the real pool path and return how long the wait was,
/// plus the proxy file path. Removes any stale proxy first so we time a real build.
fn build_proxy_via_pool(media: &MediaSource, proxy_height: u32) -> Option<(Duration, PathBuf)> {
    let pp = proxy_path(&media.path, proxy_height);
    let _ = std::fs::remove_file(&pp);

    let mut pool = MediaPool::new();
    pool.open(media).expect("open source in pool");
    let start = Instant::now();
    pool.request_proxy(media);

    let deadline = start + Duration::from_secs(180);
    loop {
        pool.poll_proxies();
        match pool.proxy_status(media.id) {
            Some(ProxyStatus::Ready(p)) => return Some((start.elapsed(), p.clone())),
            Some(ProxyStatus::Failed(e)) => {
                eprintln!("proxy build failed: {e}");
                return None;
            }
            _ => {}
        }
        if Instant::now() >= deadline {
            eprintln!("proxy build timed out");
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn bench_import_seek(c: &mut Criterion) {
    let Some(path) = asset_path() else {
        eprintln!(
            "skipping import_seek: no asset found (set CUTLASS_BENCH_ASSET, or place the \
             sibling cutlass-main test fixtures)"
        );
        return;
    };

    // Probe the clip: duration (for seek targets) and fps (for the MediaSource).
    let probe = Decoder::open(&path).expect("open asset");
    let secs = probe
        .duration()
        .unwrap_or_else(|| Duration::from_secs(5))
        .as_secs_f64();
    let (fps_num, fps_den) = probe.info().frame_rate_parts();
    let (width, height) = (probe.info().width, probe.info().height);
    drop(probe);

    let fps = if fps_den != 0 {
        fps_num as f64 / fps_den as f64
    } else {
        30.0
    };
    let total_frames = (secs * fps).round() as i64;
    let media = MediaSource::new(
        path.clone(),
        width,
        height,
        Rational::new(fps_num.max(1), fps_den.max(1)),
        total_frames.max(1),
        false,
    );

    eprintln!(
        "import_seek asset: {} ({}x{}, {:.3} fps, {:.1}s, {} frames)",
        path.display(),
        width,
        height,
        fps,
        secs,
        total_frames
    );

    // Step 1: the "little wait" — build the proxy through the pool/lane pool.
    let proxy_height = 1080;
    let proxy = build_proxy_via_pool(&media, proxy_height);
    if let Some((wait, ref pp)) = proxy {
        let src_mb = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) as f64 / 1e6;
        let pxy_mb = std::fs::metadata(pp).map(|m| m.len()).unwrap_or(0) as f64 / 1e6;
        eprintln!(
            "proxy ready in {:.2}s  (source {:.0} MB -> proxy {:.0} MB at {}p)",
            wait.as_secs_f64(),
            src_mb,
            pxy_mb,
            proxy_height
        );
    }

    // Step 2: cold-seek latency, source vs proxy, at a mid and a far target.
    let targets = [
        ("mid_50pct", Duration::from_secs_f64(secs * 0.50)),
        ("far_90pct", Duration::from_secs_f64(secs * 0.90)),
    ];

    let mut group = c.benchmark_group("seek_after_import");
    // Cold seeks on a 4K long-GOP source are slow (~0.5–1.5s each); keep the
    // sample count low so the run finishes in a sane time.
    group.sample_size(10);

    for (target_name, target) in &targets {
        // The live path before a proxy exists: original source, hardware-auto.
        let id = BenchmarkId::new("source_auto", target_name);
        group.bench_with_input(id, target, |b, &target| {
            b.iter_batched_ref(
                || Decoder::open_with(&path, DecodeOptions::default()).expect("open source"),
                |dec| {
                    let frame = dec.seek_to_frame(target).expect("seek").expect("frame");
                    criterion::black_box(frame);
                },
                BatchSize::PerIteration,
            );
        });

        // The path after the wait: all-intra proxy, software (as the pool reads it).
        if let Some((_, pp)) = &proxy {
            let id = BenchmarkId::new("proxy_sw", target_name);
            group.bench_with_input(id, target, |b, &target| {
                b.iter_batched_ref(
                    || {
                        Decoder::open_with(pp, DecodeOptions::default().hw_accel(HwAccel::None))
                            .expect("open proxy")
                    },
                    |dec| {
                        let frame = dec.seek_to_frame(target).expect("seek").expect("frame");
                        criterion::black_box(frame);
                    },
                    BatchSize::PerIteration,
                );
            });
        }
    }
    group.finish();
}

criterion_group!(benches, bench_import_seek);
criterion_main!(benches);
