//! Timeline layer resolution and decode helpers for GPU compositing.

use std::time::Instant;

use cutlass_cache::{FrameCache, SourceFingerprint};
use cutlass_compositor::{CompositeLayer, CompositorConfig};
use cutlass_decoder::DecodedFrame;
use cutlass_models::{Clip, Generator, ModelError, Project, RationalTime, TrackKind};
use tracing::debug;

use crate::ColorConvertPath;
use crate::decoder_pool::DecoderPool;
use crate::error::EngineError;
use crate::frame::{decoded_to_yuv_layer, legacy_decoded_to_rgba, RgbaFrame};

const DEFAULT_WIDTH: u32 = 1920;
const DEFAULT_HEIGHT: u32 = 1080;

/// Output canvas size: max media dimensions on the timeline, or 1920×1080 fallback.
/// Width and height are forced even for downstream H.264 export.
pub fn composite_canvas_size(project: &Project) -> (u32, u32) {
    let mut max_w = 0u32;
    let mut max_h = 0u32;

    for track in project.timeline().tracks_ordered() {
        if track.kind != TrackKind::Video {
            continue;
        }
        for clip in track.clips() {
            if let Some(media_id) = clip.media()
                && let Some(media) = project.media(media_id)
            {
                max_w = max_w.max(media.width);
                max_h = max_h.max(media.height);
            }
        }
    }

    if max_w == 0 || max_h == 0 {
        (DEFAULT_WIDTH, DEFAULT_HEIGHT)
    } else {
        (to_even(max_w), to_even(max_h))
    }
}

/// Resolve enabled video layers at `time`, bottom to top.
///
/// Pass `cache: Some(...)` for interactive preview (disk cache on hit). Pass
/// `None` for export so every media frame is decoded from the original source
/// file — never from cached YUV blobs (and never from future proxy paths).
pub fn resolve_layers(
    project: &Project,
    cache: Option<&FrameCache>,
    pool: &mut DecoderPool,
    time: RationalTime,
    canvas: &CompositorConfig,
    color_convert: ColorConvertPath,
) -> Result<Vec<CompositeLayer>, EngineError> {
    let mut layers = Vec::new();

    for track in project.timeline().tracks_ordered() {
        if !track.kind.is_visual() || !track.enabled {
            continue;
        }
        let Some(clip) = track.clip_at(time)? else {
            continue;
        };

        match &clip.content {
            cutlass_models::ClipSource::Media { .. } => {
                let layer = match color_convert {
                    ColorConvertPath::Gpu => {
                        let decoded = decode_media_frame(project, cache, pool, clip, time)?;
                        CompositeLayer::Yuv420p(decoded_to_yuv_layer(&decoded)?)
                    }
                    ColorConvertPath::LegacyCpu => {
                        let frame = decode_media_rgba_legacy(project, cache, pool, clip, time)?;
                        let bytes = legacy_resize_rgba(&frame, canvas.width, canvas.height)?;
                        CompositeLayer::Rgba { bytes }
                    }
                };
                layers.push(layer);
            }
            cutlass_models::ClipSource::Generated(generator) => match generator {
                Generator::SolidColor { rgba } => {
                    layers.push(CompositeLayer::Solid { rgba: *rgba });
                }
                Generator::Text { .. }
                | Generator::Shape { .. }
                | Generator::Sticker
                | Generator::Effect
                | Generator::Filter
                | Generator::Adjustment => {
                    debug!(?generator, "skipping unsupported generator for composite");
                }
            },
        }
    }

    Ok(layers)
}

fn decode_media_frame(
    project: &Project,
    cache: Option<&FrameCache>,
    pool: &mut DecoderPool,
    clip: &Clip,
    time: RationalTime,
) -> Result<DecodedFrame, EngineError> {
    let source_time = clip
        .source_time_at(time)?
        .ok_or_else(|| EngineError::Preview("timeline position outside clip".into()))?;

    let media_id = clip
        .media()
        .ok_or_else(|| EngineError::Preview("clip has no backing media".into()))?;
    let media = project
        .media(media_id)
        .ok_or(ModelError::UnknownMedia(media_id))?;

    let fingerprint = SourceFingerprint::from_path(media.path())?;
    let source_id = fingerprint.id();

    let (decoder, index) = pool.decoder_and_index(media_id, media.path())?;
    // Exact rational → stream-tick conversion. The old `RationalTime →
    // Duration → ticks` path truncated twice, landing rate-matched targets
    // one tick below the frame's stored PTS — a guaranteed cache miss on
    // every revisit (measured: 1080p24 media on the 24fps timeline only hit
    // on ticks where the nanosecond hop happened to be exact).
    let target_ticks = index.rate_ticks_to_stream_ticks(
        source_time.value,
        source_time.rate.num,
        source_time.rate.den,
    );

    let start = Instant::now();
    if let Some(cache) = cache
        && let Some(packed) = cache.get(source_id, target_ticks)
    {
        debug!(
            us = start.elapsed().as_micros() as u64,
            pts = target_ticks,
            "preview frame cache hit"
        );
        return crate::frame::unpack_yuv420p(&packed, media.width, media.height);
    }

    let decoded = decoder
        .frame_at_ticks(target_ticks)?
        .ok_or_else(|| EngineError::Preview("decoder returned no frame".into()))?;
    debug!(
        ms = start.elapsed().as_secs_f64() * 1000.0,
        pts = decoded.pts_ticks,
        "decoded media frame"
    );

    if let Some(cache) = cache
        && let Ok(packed) = crate::frame::pack_yuv420p(&decoded)
    {
        cache.cache_frame(source_id, decoded.pts_ticks, packed);
    }

    Ok(decoded)
}

fn decode_media_rgba_legacy(
    project: &Project,
    cache: Option<&FrameCache>,
    pool: &mut DecoderPool,
    clip: &Clip,
    time: RationalTime,
) -> Result<RgbaFrame, EngineError> {
    let decoded = decode_media_frame(project, cache, pool, clip, time)?;
    legacy_decoded_to_rgba(&decoded)
}

fn legacy_resize_rgba(frame: &RgbaFrame, dst_w: u32, dst_h: u32) -> Result<Vec<u8>, EngineError> {
    if frame.width == dst_w && frame.height == dst_h {
        return Ok(frame.bytes.clone());
    }
    if frame.width == 0 || frame.height == 0 {
        return Err(EngineError::Preview("cannot resize empty frame".into()));
    }

    let src_w = frame.width as f32;
    let src_h = frame.height as f32;
    let dst_w_f = dst_w as f32;
    let dst_h_f = dst_h as f32;
    let mut out = vec![0u8; (dst_w * dst_h * 4) as usize];

    for y in 0..dst_h {
        for x in 0..dst_w {
            let src_x = (x as f32 + 0.5) * src_w / dst_w_f - 0.5;
            let src_y = (y as f32 + 0.5) * src_h / dst_h_f - 0.5;
            let px = sample_bilinear(&frame.bytes, frame.width, frame.height, src_x, src_y);
            let i = ((y * dst_w + x) * 4) as usize;
            out[i..i + 4].copy_from_slice(&px);
        }
    }
    Ok(out)
}

fn sample_bilinear(bytes: &[u8], w: u32, h: u32, x: f32, y: f32) -> [u8; 4] {
    let x = x.clamp(0.0, (w.saturating_sub(1)) as f32);
    let y = y.clamp(0.0, (h.saturating_sub(1)) as f32);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(w.saturating_sub(1));
    let y1 = (y0 + 1).min(h.saturating_sub(1));
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;

    let p = |px: u32, py: u32| -> [f32; 4] {
        let i = ((py * w + px) * 4) as usize;
        [
            bytes[i] as f32,
            bytes[i + 1] as f32,
            bytes[i + 2] as f32,
            bytes[i + 3] as f32,
        ]
    };

    let c00 = p(x0, y0);
    let c10 = p(x1, y0);
    let c01 = p(x0, y1);
    let c11 = p(x1, y1);

    let mut out = [0u8; 4];
    for ch in 0..4 {
        let top = c00[ch] * (1.0 - tx) + c10[ch] * tx;
        let bot = c01[ch] * (1.0 - tx) + c11[ch] * tx;
        let v = top * (1.0 - ty) + bot * ty;
        out[ch] = v.round().clamp(0.0, 255.0) as u8;
    }
    out
}

fn to_even(v: u32) -> u32 {
    if v.is_multiple_of(2) { v } else { v + 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutlass_models::Rational;

    #[test]
    fn composite_canvas_size_defaults_without_media() {
        let project = Project::new("test", Rational::FPS_24);
        assert_eq!(composite_canvas_size(&project), (1920, 1080));
    }

    #[test]
    fn legacy_resize_identity_is_copy() {
        let frame = RgbaFrame::new(2, 2, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16])
            .unwrap();
        let out = legacy_resize_rgba(&frame, 2, 2).unwrap();
        assert_eq!(out, frame.bytes);
    }
}
