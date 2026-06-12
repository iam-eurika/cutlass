//! Media probing: inspect files for container, codec, and stream metadata
//! without opening a full decode pipeline.
//!
//! Used at import time (duration, frame rate, resolution, audio layout) before
//! [`cutlass-decoder`] takes over for frame reads.

mod error;
mod media;
mod probe;

pub use error::ProbeError;
pub use media::MediaProbe;
pub use probe::{duration_ticks_from_micros, is_image_path, probe};

use tracing::info;

pub fn init() {
    let _ = probe::ensure_ffmpeg_init();
    info!("cutlass-probe ready");
}
