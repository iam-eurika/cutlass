//! Cold-seek latency benchmark.
//!
//! A *cold* seek is a jump into a freshly opened decoder that holds no warmed-up
//! state: it flushes decoder buffers, enters the stream at the keyframe at or
//! before the target, and decodes forward to the landing frame. This is the
//! worst case the timeline hits on the first scrub of a clip or any backward /
//! far-forward jump, and it is the latency a user feels as scrub lag.
//!
//! Each sample re-opens the decoder (via `iter_batched`'s setup, excluded from
//! the timing) so the measured routine only covers `seek_to_frame`: the seek,
//! the throwaway decode from the keyframe, and the single CPU frame copy.
//!
//! The asset defaults to the bundled 5s/320x240 test clip; point
//! `CUTLASS_BENCH_ASSET` at a representative (e.g. 1080p, long-GOP) file for a
//! meaningful number:
//!
//! ```bash
//! CUTLASS_BENCH_ASSET=target/bench-assets/testsrc_1080p_gop150.mp4 \
//!   cargo bench -p cutlass-decode --bench cold_seek
//! ```

use std::path::PathBuf;
use std::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};

use cutlass_decode::{DecodeOptions, Decoder, HwAccel};

fn asset_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CUTLASS_BENCH_ASSET") {
        let path = PathBuf::from(p);
        return path.exists().then_some(path);
    }
    // Fallback: the small fixture shared with the sibling decoder crate.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../cutlass-main/crates/decoder/tests/assets/testsrc_h264.mp4");
    path.exists().then_some(path)
}

fn bench_cold_seek(c: &mut Criterion) {
    let Some(path) = asset_path() else {
        eprintln!(
            "skipping cold_seek: no asset found (set CUTLASS_BENCH_ASSET, or place the \
             sibling cutlass-main test fixtures)"
        );
        return;
    };

    // Probe the clip once to size seek targets relative to its real duration.
    let duration = Decoder::open(&path)
        .expect("open asset")
        .duration()
        .unwrap_or_else(|| Duration::from_secs(5));
    let secs = duration.as_secs_f64();

    eprintln!(
        "cold_seek asset: {} ({:.1}s)",
        path.display(),
        secs
    );

    // Representative cold targets: mid-clip and a far (near-end) jump. Both enter
    // a fresh decoder, so they exercise the full flush + decode-from-keyframe path.
    let targets = [
        ("mid_50pct", Duration::from_secs_f64(secs * 0.50)),
        ("far_90pct", Duration::from_secs_f64(secs * 0.90)),
    ];

    // Software vs. the app's default (hardware-auto) decode path.
    let configs = [
        ("auto", DecodeOptions::default()),
        ("software", DecodeOptions::default().hw_accel(HwAccel::None)),
    ];

    let mut group = c.benchmark_group("cold_seek_ms");
    for (cfg_name, options) in &configs {
        for (target_name, target) in &targets {
            let id = BenchmarkId::new(*cfg_name, target_name);
            group.bench_with_input(id, target, |b, &target| {
                b.iter_batched_ref(
                    || Decoder::open_with(&path, options.clone()).expect("open decoder"),
                    |decoder| {
                        let frame = decoder
                            .seek_to_frame(target)
                            .expect("seek")
                            .expect("frame after seek");
                        criterion::black_box(frame);
                    },
                    // One fresh decoder per sample keeps every seek genuinely cold.
                    BatchSize::PerIteration,
                );
            });
        }
    }
    group.finish();
}

criterion_group!(benches, bench_cold_seek);
criterion_main!(benches);
