//! Timeline preview: resolve a timeline time to a composited RGBA frame.

use std::time::Duration;

use cutlass_cache::{FrameCache, SourceFingerprint};
use cutlass_models::{Clip, ModelError, Project, RationalTime, TrackKind};

use crate::decoder_pool::DecoderPool;
use crate::error::EngineError;
use crate::frame::{decoded_to_rgba, pack_yuv420p, unpack_yuv420p, RgbaFrame};

pub fn get_frame(
    project: &Project,
    cache: &FrameCache,
    pool: &mut DecoderPool,
    time: RationalTime,
) -> Result<RgbaFrame, EngineError> {
    let tl_rate = project.timeline().frame_rate;
    if time.rate != tl_rate {
        return Err(ModelError::RateMismatch {
            expected: tl_rate,
            got: time.rate,
        }
        .into());
    }

    let clip = top_video_clip_at(project, time)?
        .ok_or_else(|| EngineError::Preview("no video at timeline position".into()))?;

    if clip.is_generated() {
        return Err(EngineError::Preview(
            "generated clips cannot be previewed yet".into(),
        ));
    }

    let source_time = clip
        .source_time_at(time)?
        .ok_or_else(|| EngineError::Preview("timeline position outside clip".into()))?;

    let media_id = clip.media().ok_or_else(|| {
        EngineError::Preview("clip has no backing media".into())
    })?;
    let media = project
        .media(media_id)
        .ok_or(ModelError::UnknownMedia(media_id))?;

    let fingerprint = SourceFingerprint::from_path(media.path())?;
    let source_id = fingerprint.id();
    let target = rational_time_to_duration(source_time);

    let (decoder, index) = pool.decoder_and_index(media_id, media.path())?;
    let pts = index.duration_to_ticks(target);

    if let Some(packed) = cache.get(source_id, pts) {
        let decoded = unpack_yuv420p(&packed, media.width, media.height)?;
        return decoded_to_rgba(&decoded);
    }

    let decoded = decoder
        .seek_to_frame(target)?
        .ok_or_else(|| EngineError::Preview("decoder returned no frame".into()))?;

    if let Ok(packed) = pack_yuv420p(&decoded) {
        cache.cache_frame(source_id, decoded.pts_ticks, packed);
    }

    decoded_to_rgba(&decoded)
}

fn top_video_clip_at<'a>(
    project: &'a Project,
    time: RationalTime,
) -> Result<Option<&'a Clip>, ModelError> {
    let tracks: Vec<_> = project.timeline().tracks_ordered().collect();
    for track in tracks.into_iter().rev() {
        if track.kind != TrackKind::Video || !track.enabled {
            continue;
        }
        if let Some(clip) = track.clip_at(time)? {
            return Ok(Some(clip));
        }
    }
    Ok(None)
}

fn rational_time_to_duration(time: RationalTime) -> Duration {
    let num = i128::from(time.rate.num);
    let den = i128::from(time.rate.den);
    if num <= 0 || den <= 0 || time.value <= 0 {
        return Duration::ZERO;
    }
    let nanos = (i128::from(time.value) * 1_000_000_000 * den) / num;
    Duration::from_nanos(nanos.clamp(0, i128::from(u64::MAX)) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutlass_models::Rational;

    #[test]
    fn rational_time_to_duration_one_second_at_24fps() {
        let t = RationalTime::new(24, Rational::FPS_24);
        assert_eq!(rational_time_to_duration(t), Duration::from_secs(1));
    }
}
