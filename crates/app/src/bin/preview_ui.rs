//! Slint preview window: playhead slider drives [`PreviewSession::preview_render`].
//!
//! ```text
//! cargo run -p app --bin preview_ui
//! cargo run -p app -- assets/your_video.mp4
//! ```

use std::path::PathBuf;
use std::process;
use std::sync::{Arc, Mutex};

use slint::ComponentHandle;
use tracing_subscriber::EnvFilter;

use app::{
    fixture_available, h264_fixture_path, preview_worker::PreviewWorker, ui, PreviewSeek,
    PreviewSession, PREVIEW_MAX_EDGE_PX,
};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let media = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let p = h264_fixture_path();
            eprintln!("preview_ui: using {}", p.display());
            p
        });

    if !fixture_available(&media) {
        eprintln!(
            "error: media not found: {}\n\
             bash crates/decoder/tests/assets/regenerate.sh",
            media.display()
        );
        process::exit(1);
    }

    let (project, track_id, _, _) = PreviewSession::single_clip_project(&media).unwrap_or_else(|e| {
        eprintln!("project error: {e}");
        process::exit(1);
    });
    let session = PreviewSession::from_project(project, track_id).unwrap_or_else(|e| {
        eprintln!("preview session error: {e}");
        process::exit(1);
    });

    let ui = ui::PreviewWindow::new().expect("slint window");
    ui.set_playhead_max(ui::playhead_max_seconds(
        &session.project,
        session.video_track(),
    ));
    ui.set_playhead_seconds(0.0);
    ui.set_preview_image(ui::black_placeholder_image());
    ui.set_status_text(format!("Loading {}…", media.display()).into());

    let session = Arc::new(Mutex::new(session));
    let ui_handle = ui.as_weak();
    let worker = PreviewWorker::spawn(Arc::clone(&session), ui_handle.clone());

    worker.request(0.0, PreviewSeek::Exact);
    if let Some(ui) = ui_handle.upgrade() {
        if let Ok(s) = session.lock() {
            let max = ui::effective_playhead_max_seconds(&s);
            ui.set_status_text(
                format!(
                    "{}  (preview ≤{PREVIEW_MAX_EDGE_PX}px, 0–{max:.0}s)",
                    media.display()
                )
                .into(),
            );
        }
    }

    ui.on_scrub_requested({
        let ui_handle = ui_handle.clone();
        let worker = worker.clone();
        move |seconds| {
            if let Some(ui) = ui_handle.upgrade() {
                ui.set_playhead_seconds(seconds);
            }
            worker.request_scrub(seconds);
        }
    });

    ui.run().expect("run UI");
}
