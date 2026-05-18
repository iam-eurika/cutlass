//! Slint preview helpers (RGBA → [`slint::Image`]).

use decoder::Rational;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use timeline::Project;

use crate::playhead::{max_timeline_end, seconds_to_rational};
use crate::preview::{PreviewOutcome, PreviewRender, PreviewSeek};
use crate::PreviewSession;

slint::include_modules!();

/// Build a Slint image from RGBA8 bytes (row-major).
pub fn image_from_rgba8(width: u32, height: u32, rgba: &[u8]) -> Image {
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba, width, height);
    Image::from_rgba8(buffer)
}

/// 1×1 black frame for gap / placeholder.
pub fn black_placeholder_image() -> Image {
    image_from_rgba8(1, 1, &[0, 0, 0, 255])
}

/// Apply a [`PreviewRender`] using pixels already in `rgba` (no extra engine work).
pub fn apply_render_to_window(ui: &PreviewWindow, render: PreviewRender, rgba: &[u8]) {
    match render {
        PreviewRender::Gap => {
            ui.set_preview_image(black_placeholder_image());
            ui.set_status_text("No clip at this time (gap)".into());
        }
        PreviewRender::Frame {
            clip_id,
            media_time,
            width,
            height,
        } => {
            ui.set_preview_image(image_from_rgba8(width, height, rgba));
            ui.set_status_text(
                format!("clip {clip_id}  media {media_time}  {width}×{height}").into(),
            );
        }
    }
}

/// Apply a [`PreviewOutcome`] to the preview window properties.
pub fn apply_outcome_to_window(ui: &PreviewWindow, outcome: &PreviewOutcome) {
    match outcome {
        PreviewOutcome::Gap => {
            ui.set_preview_image(black_placeholder_image());
            ui.set_status_text("No clip at this time (gap)".into());
        }
        PreviewOutcome::Frame {
            clip_id,
            media_time,
            width,
            height,
            rgba,
        } => {
            ui.set_preview_image(image_from_rgba8(*width, *height, rgba));
            ui.set_status_text(
                format!("clip {clip_id}  media {media_time}  {width}×{height}").into(),
            );
        }
    }
}

/// Seek the session and refresh the Slint preview window.
pub fn seek_and_update(
    session: &mut PreviewSession,
    ui: &PreviewWindow,
    timeline_time: Rational,
) -> Result<(), crate::PreviewError> {
    seek_and_update_mode(session, ui, timeline_time, PreviewSeek::Exact)
}

/// Seek with explicit engine mode (scrub vs exact).
pub fn seek_and_update_mode(
    session: &mut PreviewSession,
    ui: &PreviewWindow,
    timeline_time: Rational,
    mode: PreviewSeek,
) -> Result<(), crate::PreviewError> {
    let outcome = session.preview_at_with_mode(timeline_time, mode)?;
    apply_outcome_to_window(ui, &outcome);
    Ok(())
}

/// Playhead slider maximum from clip layout on the video track (at least 1s).
pub fn playhead_max_seconds(project: &Project, video_track: timeline::TrackId) -> f32 {
    let Some(end) = max_timeline_end(project, video_track) else {
        return 10.0;
    };
    rational_to_seconds(end).max(1.0).min(3600.0) as f32
}

/// Cap slider max by probed media duration when known (avoids seek EOF on short files).
pub fn effective_playhead_max_seconds(session: &PreviewSession) -> f32 {
    let layout_max = playhead_max_seconds(&session.project, session.video_track());
    let probe_max = session
        .project
        .sources
        .values()
        .filter_map(|s| s.probed.as_ref()?.duration)
        .map(rational_to_seconds)
        .fold(0.0_f64, f64::max);
    if probe_max > 0.0 {
        layout_max.min(probe_max as f32)
    } else {
        layout_max
    }
}

/// Update the Slint slider upper bound after probe / open.
pub fn refresh_playhead_range(session: &PreviewSession, ui: &PreviewWindow) {
    let max = effective_playhead_max_seconds(session);
    ui.set_playhead_max(max);
    if ui.get_playhead_seconds() > max {
        ui.set_playhead_seconds(max);
    }
}

fn rational_to_seconds(r: Rational) -> f64 {
    r.num as f64 / f64::from(r.den)
}

/// Convenience: seek from slider seconds.
pub fn seek_and_update_seconds(
    session: &mut PreviewSession,
    ui: &PreviewWindow,
    seconds: f32,
    mode: PreviewSeek,
) -> Result<(), crate::PreviewError> {
    seek_and_update_mode(session, ui, seconds_to_rational(seconds), mode)
}

/// Seek without a UI handle (for background threads); apply on the event loop.
pub fn preview_at_seconds(
    session: &mut PreviewSession,
    seconds: f32,
    mode: PreviewSeek,
) -> Result<PreviewOutcome, crate::PreviewError> {
    session.preview_at_with_mode(seconds_to_rational(seconds), mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_image_dimensions() {
        let img = image_from_rgba8(2, 2, &[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255]);
        assert_eq!(img.size().width, 2);
        assert_eq!(img.size().height, 2);
    }
}
