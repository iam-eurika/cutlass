mod command;
mod models;
mod projector;
mod ruler;
mod state;
mod timecode;
mod timeline;

use std::cell::RefCell;
use std::rc::Rc;

use slint::SharedString;

use slint::BackendSelector;
use slint::wgpu_28::WGPUConfiguration;

use crate::command::Command;
use crate::state::Editor;

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    BackendSelector::new()
        .require_wgpu_28(WGPUConfiguration::default())
        .select()?;

    let app = AppWindow::new()?;

    // The Editor is the only thing in the program allowed to mutate
    // the project. Slint never writes to `EditorStore.project`; UI
    // gestures dispatch commands through callbacks like `move-clip`
    // below, the Editor applies them to the domain model, and the
    // projector patches the matching row in the Slint projection.
    let editor = Rc::new(RefCell::new(Editor::new(models::sample_project())));

    // One-shot hand-off of the projection to Slint. After this, the
    // projector keeps the underlying `VecModel`s alive and writes
    // into them in place; we never set `project` again.
    app.global::<EditorStore>()
        .set_project(editor.borrow().slint_project().clone());

    {
        let editor = editor.clone();
        app.global::<EditorStore>().on_move_clip(
            move |track_id: SharedString, clip_id: SharedString, new_start_value: i32| {
                let cmd = Command::MoveClip {
                    track_id: track_id.into(),
                    clip_id: clip_id.into(),
                    new_start_value,
                };
                if let Err(err) = editor.borrow_mut().apply(&cmd) {
                    // The gesture layer can produce ids that no
                    // longer exist (e.g. the clip was deleted by
                    // another command mid-drag). Log and drop —
                    // never panic on UI input.
                    eprintln!("move-clip rejected: {err}");
                }
            },
        );
    }

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
