use std::path::Path;

use cutlass_cache::{CacheSpec, FrameCache, SourceFingerprint};
use cutlass_decoder::{DecodeOptions, Decoder, HwAccel};
use cutlass_models::{MediaSource, Rational};
use tracing::debug;

use crate::error::EngineError;

/// Probe a video file and register it with the frame cache.
pub fn import_media(path: &Path, cache: &FrameCache) -> Result<MediaSource, EngineError> {
    let decoder = Decoder::open_with(path, DecodeOptions::default().hw_accel(HwAccel::None))?;
    let info = decoder.info();

    let (fr_num, fr_den) = info.frame_rate_parts();
    let frame_rate = Rational::new(fr_num, fr_den);
    let frame_rate = if frame_rate.is_valid() {
        frame_rate
    } else {
        Rational::FPS_24
    };

    let duration_ticks = decoder
        .duration()
        .map(|d| duration_ticks_from_micros(frame_rate, d.as_micros() as u64))
        .unwrap_or(0);

    let fingerprint = SourceFingerprint::from_path(path)?;
    let spec = CacheSpec {
        width: info.width,
        height: info.height,
        pixfmt: "yuv420p".into(),
    };
    cache
        .register_source(fingerprint, spec)
        .map_err(cutlass_cache::DiskCacheError::from)?;

    let media = MediaSource::new(
        path,
        info.width,
        info.height,
        frame_rate,
        duration_ticks,
        false,
    );

    debug!(
        path = %path.display(),
        width = info.width,
        height = info.height,
        duration_ticks,
        "imported media"
    );

    Ok(media)
}

fn duration_ticks_from_micros(frame_rate: Rational, micros: u64) -> i64 {
    if !frame_rate.is_valid() || micros == 0 {
        return 0;
    }
    let ticks = (i128::from(micros) * i128::from(frame_rate.num))
        / (i128::from(frame_rate.den) * 1_000_000);
    ticks.clamp(0, i128::from(i64::MAX)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_ticks_from_micros_one_second_at_24fps() {
        assert_eq!(
            duration_ticks_from_micros(Rational::FPS_24, 1_000_000),
            24
        );
    }

    #[test]
    fn duration_ticks_from_micros_zero_micros_is_zero() {
        assert_eq!(duration_ticks_from_micros(Rational::FPS_24, 0), 0);
    }
}
