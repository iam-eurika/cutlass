//! Cutlass application layer: connects [`timeline`], [`engine`], and [`renderer`].

mod playhead;
mod preview;
pub mod preview_worker;
pub mod ui;

use std::path::PathBuf;
use std::process;

use decoder::Rational;
use tracing::info;

pub use playhead::{
    default_video_track, max_timeline_end, plan_playhead, seconds_to_rational, PlayheadPlan,
};
pub use preview::{
    fixture_available, h264_fixture_path, pixel_variance_rgba, preview_target_dimensions,
    rgba_fingerprint, PreviewError, PreviewOutcome, PreviewSeek, PreviewSession,
    PREVIEW_MAX_EDGE_PX,
};
pub use ui::{effective_playhead_max_seconds, refresh_playhead_range};

/// CLI entry: timeline playhead → engine → renderer for a single-clip project.
pub fn run_preview_cli(media: Option<PathBuf>) {
    let media = media.unwrap_or_else(|| {
        let p = h264_fixture_path();
        eprintln!("app: no media given, using {}", p.display());
        p
    });

    if !fixture_available(&media) {
        eprintln!(
            "error: media not found: {}\n\
             generate decoder fixtures: bash crates/decoder/tests/assets/regenerate.sh",
            media.display()
        );
        process::exit(1);
    }

    let (project, track_id, _, _) = PreviewSession::single_clip_project(&media).unwrap_or_else(|e| {
        eprintln!("error: project: {e}");
        process::exit(1);
    });
    let mut session = PreviewSession::from_project(project, track_id).unwrap_or_else(|e| {
        eprintln!("error: preview session: {e}");
        process::exit(1);
    });

    // Open + probe at t=0 so short assets get a correct duration bound.
    let source_id = *session.project.sources.keys().next().unwrap();
    if let Err(e) = session.ensure_source_open(source_id) {
        eprintln!("error: open media: {e}");
        process::exit(1);
    }
    session.drain_events(std::time::Duration::from_millis(100)).ok();

    let max_secs = effective_playhead_max_seconds(&session);
    let mid = (max_secs / 2.0).max(0.0);
    let times = [
        Rational::new_raw(0, 1),
        seconds_to_rational(mid),
        seconds_to_rational((max_secs - 0.5).max(0.0)),
        Rational::new_raw(99, 1),
    ];

    eprintln!("probed playhead range 0..{max_secs:.1}s");

    for t in times {
        match session.preview_at(t) {
            Ok(PreviewOutcome::Gap) => info!(timeline = %t, "gap (no clip)"),
            Ok(PreviewOutcome::Frame {
                clip_id,
                media_time,
                width,
                height,
                rgba,
            }) => {
                info!(
                    timeline = %t,
                    %clip_id,
                    %media_time,
                    %width,
                    %height,
                    bytes = rgba.len(),
                    variance = pixel_variance_rgba(&rgba),
                    "frame"
                );
            }
            Err(e) => {
                eprintln!("preview at {t}: {e}");
                process::exit(1);
            }
        }
    }
}
