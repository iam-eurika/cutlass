//! Register a file in the media pool using demux-only probing.

use std::path::Path;

use cutlass_cache::{CacheSpec, FrameCache, SourceFingerprint};
use cutlass_probe::probe;
use cutlass_models::MediaSource;
use tracing::debug;

use crate::error::EngineError;

/// Probe a video file and register it with the frame cache.
pub fn import_media(path: &Path, cache: &FrameCache) -> Result<MediaSource, EngineError> {
    let probed = probe(path)?;

    let fingerprint = SourceFingerprint::from_path(path)?;
    let spec = CacheSpec {
        width: probed.width,
        height: probed.height,
        pixfmt: "yuv420p".into(),
    };
    cache
        .register_source(fingerprint, spec)
        .map_err(cutlass_cache::DiskCacheError::from)?;

    let media = probed.to_media_source(path);

    debug!(
        path = %path.display(),
        width = media.width,
        height = media.height,
        duration_ticks = media.duration.value,
        has_audio = media.has_audio,
        codec = %probed.video_codec,
        "imported media"
    );

    Ok(media)
}
