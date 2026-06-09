//! Inverse-command undo for import and clip edits.

mod common;

use common::{import_asset, rt, small_video_asset, temp_engine, tr};
use cutlass_commands::{Command, EditCommand};
use cutlass_models::TrackKind;

#[test]
fn undo_import_removes_media() {
    let Some(path) = small_video_asset() else {
        return;
    };
    let (_dir, mut engine) = temp_engine();
    let media_id = import_asset(&mut engine, &path);
    assert!(engine.project().media(media_id).is_some());

    assert!(engine.undo());
    assert!(engine.project().media(media_id).is_none());

    assert!(engine.redo());
    assert!(engine.project().media(media_id).is_some());
}

#[test]
fn undo_add_clip_uses_inverse_not_snapshot() {
    let Some(path) = small_video_asset() else {
        return;
    };
    let (_dir, mut engine) = temp_engine();
    let media_id = import_asset(&mut engine, &path);
    let track = engine.project_mut().add_track(TrackKind::Video, "V1");

    engine
        .apply(Command::Edit(EditCommand::AddClip {
            track,
            media: media_id,
            source: tr(0, 48),
            start: rt(0),
        }))
        .expect("add clip");

    assert_eq!(engine.project().timeline().clip_count(), 1);
    assert!(engine.undo());
    assert_eq!(engine.project().timeline().clip_count(), 0);
    assert_eq!(engine.project().media_count(), 1, "import survives clip undo");
}

#[test]
fn undo_import_fails_while_media_referenced() {
    let Some(path) = small_video_asset() else {
        return;
    };
    let (_dir, mut engine) = temp_engine();
    let media_id = import_asset(&mut engine, &path);
    let track = engine.project_mut().add_track(TrackKind::Video, "V1");
    // Place clip without pushing undo — stack only holds the import inverse.
    engine
        .project_mut()
        .add_clip(track, media_id, tr(0, 48), rt(0))
        .expect("direct add");

    assert!(!engine.undo());
    assert!(engine.project().media(media_id).is_some());
    assert_eq!(engine.project().timeline().clip_count(), 1);
}
