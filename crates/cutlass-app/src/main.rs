use std::path::Path;
use std::time::{Duration, Instant};

use cutlass_decode::{DecodeOptions, Decoder, HwAccel, ffmpeg_version};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

fn setup_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
}

fn main() {
    setup_tracing();
    info!(version = ffmpeg_version(), "cutlass-app starting");
    // cutlass_engines::init();
    // cutlass_compositor::init();

    let path = String::from("assets/13232364_3840_2160_24fps.mp4");

    // let hw = HwAccel::None;
    let hw = HwAccel::VideoToolbox;
    let options = DecodeOptions::default().hw_accel(hw);

    let Ok(mut decoder) = Decoder::open_with(Path::new(&path), options) else {
        warn!(path, "failed to open video");
        return;
    };
    let info = decoder.info().clone();
    info!(
        width = info.width,
        height = info.height,
        ?info.pixel_format,
        hw = info.hw_accel.name(),
        "decoder ready"
    );

    let _t = Duration::from_secs(27);

    let now = Instant::now();
    let mut n = 0;
    while let Some(_f) = decoder.next_video_frame().unwrap() {
        n += 1;
        if n >= 300 {
            break;
        }
    }
    let per_frame = now.elapsed().as_secs_f64() * 1000.0 / n as f64;
    info!(per_frame_ms = per_frame, frames = n, "decode throughput");
}
