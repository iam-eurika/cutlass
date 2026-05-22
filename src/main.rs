mod models;
mod ruler;
mod timecode;
mod timeline;

use slint::SharedString;

use slint::BackendSelector;
use slint::wgpu_28::WGPUConfiguration;

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    BackendSelector::new()
        .require_wgpu_28(WGPUConfiguration::default())
        .select()?;

    let app = AppWindow::new()?;

    let editor = app.global::<EditorStore>();
    // Inline Slint array literals are read-only MapModels; hydrate once
    // so timeline edits can use Model::set_row_data in place.
    editor.set_project(models::dto::hydrate_project(editor.get_project()));
    let editor_weak = editor.as_weak();

    let backend = app.global::<EditorBackend>();
    backend.on_move_clip(move |clip_id, track_id, timeline_start| {
        let Some(editor) = editor_weak.upgrade() else {
            return;
        };
        let project = editor.get_project();
        timeline::move_clip(
            &project,
            clip_id.as_str(),
            track_id.as_str(),
            timeline_start,
        );
    });

    let timeline = app.global::<TimelineLib>();
    timeline.on_sequence_duration(timeline::sequence_duration);
    timeline.on_format_timecode(|frame, fps_num, fps_den, drop_frame| {
        SharedString::from(crate::timecode::format_timecode(
            i64::from(frame),
            i64::from(fps_num),
            i64::from(fps_den),
            drop_frame,
        ))
    });

    app.run()
}
