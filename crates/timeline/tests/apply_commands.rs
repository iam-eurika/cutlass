//! Behavioural tests for [`timeline::apply`].
//!
//! Each command has at least one happy-path test and one invariant-violation
//! test. The fixtures use a 90 000-tick timebase to mirror what the app
//! produces in `crates/app/src/main.rs::empty_project`, so the numbers in
//! these tests are directly comparable to what users actually edit.
//!
//! Two things these tests collectively guarantee:
//!
//!   1. **Atomicity** — every failing command leaves the project byte-for-byte
//!      identical (see `failed_command_leaves_project_unchanged`). That property
//!      is the entire reason `apply` validates before it writes; if a future
//!      refactor breaks it, undo and replay both fall apart.
//!
//!   2. **Effect inversion** — `CommandEffect` snapshots enough prior state
//!      to mechanically invert each command. The split-+-undo composite test
//!      simulates the inverse step manually as a contract for the future
//!      history layer.

use models::{
    AudioStreamInfo, Clip, ClipId, Color, MediaId, MediaKind, MediaSource, Project, ProjectId,
    Rational, RationalTime, SchemaVersion, Sequence, SequenceId, TrackId, TrackKind,
    VideoStreamInfo,
};
use std::path::PathBuf;
use timeline::{
    AddClip, AddTrack, Command, CommandEffect, MoveClip, RemoveClip, SplitClip, TimelineError,
    TrimClipIn, TrimClipOut, apply,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const TB: u32 = 90_000;

fn rt(num: i64) -> RationalTime {
    RationalTime::new_raw(num, TB)
}

fn empty_project() -> Project {
    Project {
        id: ProjectId::new(),
        name: "Test".into(),
        file_path: None,
        schema: SchemaVersion::CURRENT,
        sequence: Sequence {
            id: SequenceId::new(),
            name: "Main".into(),
            width: 1920,
            height: 1080,
            fps: Rational::new_raw(30, 1),
            sample_rate: 48_000,
            timebase: TB,
            duration: rt(0),
            in_point: None,
            out_point: None,
            tracks: Vec::new(),
        },
        media_bin: Vec::new(),
        is_dirty: false,
    }
}

fn media_source(id: MediaId, name: &str) -> MediaSource {
    MediaSource {
        id,
        name: name.into(),
        path: PathBuf::from(format!("/tmp/{name}")),
        kind: MediaKind::Video,
        has_video: true,
        has_audio: true,
        duration: rt(9_000_000),
        video: Some(VideoStreamInfo {
            width: 1920,
            height: 1080,
            fps: Rational::new_raw(30, 1),
            codec: "h264".into(),
        }),
        audio: Some(AudioStreamInfo {
            sample_rate: 48_000,
            codec: "aac".into(),
        }),
        is_supported: true,
        is_loading: false,
        is_missing: false,
        error: None,
    }
}

fn clip(id: ClipId, track_id: TrackId, media_id: Option<MediaId>, start: i64, dur: i64) -> Clip {
    Clip {
        id,
        media_id,
        track_id,
        name: "C".into(),
        start: rt(start),
        duration: rt(dur),
        source_in: rt(0),
        source_out: rt(dur),
        speed: Rational::ONE,
        opacity: 1.0,
        volume: 1.0,
        enabled: true,
        color: Color::rgb(64, 96, 160),
    }
}

/// Build a project with one V1 track and N clips at `(start, duration)`.
fn project_with_clips(positions: &[(i64, i64)]) -> (Project, TrackId, MediaId, Vec<ClipId>) {
    let mut project = empty_project();
    let media_id = MediaId::new();
    project.media_bin.push(media_source(media_id, "vid.mp4"));

    let track_id = TrackId::new();
    apply(
        &mut project,
        &Command::AddTrack(AddTrack {
            track_id,
            kind: TrackKind::Video,
            name: "V1".into(),
            height_px: None,
        }),
    )
    .unwrap();

    let mut clip_ids = Vec::new();
    for &(start, dur) in positions {
        let clip_id = ClipId::new();
        clip_ids.push(clip_id);
        apply(
            &mut project,
            &Command::AddClip(AddClip {
                track_id,
                clip: clip(clip_id, track_id, Some(media_id), start, dur),
            }),
        )
        .unwrap();
    }
    (project, track_id, media_id, clip_ids)
}

// ---------------------------------------------------------------------------
// AddTrack
// ---------------------------------------------------------------------------

#[test]
fn add_track_appends_and_sets_defaults() {
    let mut p = empty_project();
    let v_id = TrackId::new();
    let a_id = TrackId::new();

    let v = apply(
        &mut p,
        &Command::AddTrack(AddTrack {
            track_id: v_id,
            kind: TrackKind::Video,
            name: "V1".into(),
            height_px: None,
        }),
    )
    .unwrap();
    let a = apply(
        &mut p,
        &Command::AddTrack(AddTrack {
            track_id: a_id,
            kind: TrackKind::Audio,
            name: "A1".into(),
            height_px: Some(60),
        }),
    )
    .unwrap();

    assert!(matches!(v, CommandEffect::AddTrack(e) if e.track_id == v_id));
    assert!(matches!(a, CommandEffect::AddTrack(e) if e.track_id == a_id));
    assert_eq!(p.sequence.tracks.len(), 2);
    assert_eq!(p.sequence.tracks[0].id, v_id);
    assert_eq!(p.sequence.tracks[0].kind, TrackKind::Video);
    assert_eq!(p.sequence.tracks[0].height_px, 72, "video default height");
    assert!(p.sequence.tracks[0].visible);
    assert_eq!(p.sequence.tracks[1].id, a_id);
    assert_eq!(p.sequence.tracks[1].kind, TrackKind::Audio);
    assert_eq!(p.sequence.tracks[1].height_px, 60, "explicit override wins");
    assert!(
        !p.sequence.tracks[1].visible,
        "audio tracks start hidden — eye toggle is video-only"
    );
}

#[test]
fn add_track_rejects_duplicate_id() {
    let mut p = empty_project();
    let id = TrackId::new();
    apply(
        &mut p,
        &Command::AddTrack(AddTrack {
            track_id: id,
            kind: TrackKind::Video,
            name: "V1".into(),
            height_px: None,
        }),
    )
    .unwrap();

    let err = apply(
        &mut p,
        &Command::AddTrack(AddTrack {
            track_id: id,
            kind: TrackKind::Video,
            name: "V2".into(),
            height_px: None,
        }),
    )
    .unwrap_err();
    assert!(matches!(err, TimelineError::DuplicateId { kind: "track", .. }));
    assert_eq!(p.sequence.tracks.len(), 1, "duplicate id must not write");
}

// ---------------------------------------------------------------------------
// AddClip
// ---------------------------------------------------------------------------

#[test]
fn add_clip_inserts_sorted_and_updates_duration() {
    let (mut p, track_id, media_id, _) = project_with_clips(&[]);

    // Insert two clips out of order; the track should end up sorted by start.
    let later_id = ClipId::new();
    apply(
        &mut p,
        &Command::AddClip(AddClip {
            track_id,
            clip: clip(later_id, track_id, Some(media_id), 270_000, 90_000),
        }),
    )
    .unwrap();
    let earlier_id = ClipId::new();
    apply(
        &mut p,
        &Command::AddClip(AddClip {
            track_id,
            clip: clip(earlier_id, track_id, Some(media_id), 0, 180_000),
        }),
    )
    .unwrap();

    let track = &p.sequence.tracks[0];
    assert_eq!(track.clips.len(), 2);
    assert_eq!(track.clips[0].id, earlier_id);
    assert_eq!(track.clips[1].id, later_id);
    assert_eq!(
        p.sequence.duration.num,
        270_000 + 90_000,
        "sequence duration tracks max clip end"
    );
}

#[test]
fn add_clip_rejects_overlap() {
    let (mut p, track_id, media_id, _) = project_with_clips(&[(0, 100_000)]);

    let overlap_id = ClipId::new();
    let err = apply(
        &mut p,
        &Command::AddClip(AddClip {
            track_id,
            // Starts inside [0, 100_000).
            clip: clip(overlap_id, track_id, Some(media_id), 50_000, 100_000),
        }),
    )
    .unwrap_err();
    assert!(matches!(err, TimelineError::ClipOverlap { .. }));
    assert_eq!(p.sequence.tracks[0].clips.len(), 1, "overlap must not insert");
}

#[test]
fn add_clip_allows_touching_at_boundary() {
    // Half-open intervals — [0, 100) and [100, 200) do not overlap.
    let (mut p, track_id, media_id, _) = project_with_clips(&[(0, 100_000)]);

    let touch_id = ClipId::new();
    apply(
        &mut p,
        &Command::AddClip(AddClip {
            track_id,
            clip: clip(touch_id, track_id, Some(media_id), 100_000, 50_000),
        }),
    )
    .expect("touching at boundary is legal");

    assert_eq!(p.sequence.tracks[0].clips.len(), 2);
}

#[test]
fn add_clip_rejects_track_mismatch() {
    let (mut p, track_id, media_id, _) = project_with_clips(&[]);

    let wrong_track = TrackId::new();
    let err = apply(
        &mut p,
        &Command::AddClip(AddClip {
            track_id,
            clip: clip(ClipId::new(), wrong_track, Some(media_id), 0, 1_000),
        }),
    )
    .unwrap_err();
    assert!(matches!(err, TimelineError::ClipTrackMismatch { .. }));
}

#[test]
fn add_clip_rejects_missing_media() {
    let (mut p, track_id, _, _) = project_with_clips(&[]);

    let bogus = MediaId::new();
    let err = apply(
        &mut p,
        &Command::AddClip(AddClip {
            track_id,
            clip: clip(ClipId::new(), track_id, Some(bogus), 0, 1_000),
        }),
    )
    .unwrap_err();
    assert_eq!(err, TimelineError::SourceNotFound(bogus));
}

#[test]
fn add_clip_accepts_none_media_for_generators() {
    let (mut p, track_id, _, _) = project_with_clips(&[]);

    apply(
        &mut p,
        &Command::AddClip(AddClip {
            track_id,
            // No media — a generator / title / colour matte clip.
            clip: clip(ClipId::new(), track_id, None, 0, 1_000),
        }),
    )
    .expect("media_id == None is the placeholder for generators");
}

#[test]
fn add_clip_rejects_timebase_mismatch() {
    let (mut p, track_id, media_id, _) = project_with_clips(&[]);

    let mut bad = clip(ClipId::new(), track_id, Some(media_id), 0, 1_000);
    // Sequence timebase is 90 000; force a clip onto 48 000 to simulate
    // an agent forgetting to convert.
    bad.duration = RationalTime::new_raw(1_000, 48_000);

    let err = apply(&mut p, &Command::AddClip(AddClip { track_id, clip: bad })).unwrap_err();
    assert!(
        matches!(
            err,
            TimelineError::TimebaseMismatch {
                expected_den: TB,
                got_den: 48_000
            }
        ),
        "wrong timebase must surface explicitly: {err:?}"
    );
}

#[test]
fn add_clip_rejects_invalid_source_range() {
    let (mut p, track_id, media_id, _) = project_with_clips(&[]);

    let mut bad = clip(ClipId::new(), track_id, Some(media_id), 0, 1_000);
    bad.source_in = rt(500);
    bad.source_out = rt(500);

    let err = apply(&mut p, &Command::AddClip(AddClip { track_id, clip: bad })).unwrap_err();
    assert!(matches!(err, TimelineError::InvalidTime { .. }));
}

// ---------------------------------------------------------------------------
// RemoveClip
// ---------------------------------------------------------------------------

#[test]
fn remove_clip_returns_full_clip_for_undo() {
    let (mut p, _, _, clip_ids) = project_with_clips(&[(0, 100_000), (200_000, 50_000)]);
    let id = clip_ids[0];

    let effect = apply(&mut p, &Command::RemoveClip(RemoveClip { clip_id: id })).unwrap();
    let CommandEffect::RemoveClip(effect) = effect else {
        panic!("expected RemoveClip effect, got {effect:?}");
    };

    assert_eq!(effect.clip.id, id);
    assert_eq!(effect.clip.start, rt(0));
    assert_eq!(effect.clip.duration, rt(100_000));
    assert_eq!(
        p.sequence.tracks[0].clips.len(),
        1,
        "remaining clip stays on the track"
    );
    assert_eq!(
        p.sequence.duration.num,
        250_000,
        "sequence duration follows the last surviving clip"
    );
}

#[test]
fn remove_clip_rejects_missing_id() {
    let (mut p, _, _, _) = project_with_clips(&[(0, 100_000)]);
    let bogus = ClipId::new();
    let err = apply(&mut p, &Command::RemoveClip(RemoveClip { clip_id: bogus })).unwrap_err();
    assert_eq!(err, TimelineError::ClipNotFound(bogus));
}

// ---------------------------------------------------------------------------
// MoveClip
// ---------------------------------------------------------------------------

#[test]
fn move_clip_resorts_track() {
    let (mut p, _, _, clip_ids) = project_with_clips(&[(0, 100_000), (200_000, 100_000)]);
    let first = clip_ids[0];

    // Move the first clip past the second; ordering by start must flip.
    apply(
        &mut p,
        &Command::MoveClip(MoveClip {
            clip_id: first,
            new_start: rt(400_000),
        }),
    )
    .unwrap();

    let track = &p.sequence.tracks[0];
    assert_eq!(
        track.clips[0].id, clip_ids[1],
        "previously-second clip now sorts first"
    );
    assert_eq!(track.clips[1].id, first);
    assert_eq!(track.clips[1].start, rt(400_000));
    assert_eq!(p.sequence.duration.num, 500_000);
}

#[test]
fn move_clip_rejects_overlap_with_neighbour() {
    let (mut p, _, _, clip_ids) = project_with_clips(&[(0, 100_000), (200_000, 100_000)]);
    let first = clip_ids[0];

    let err = apply(
        &mut p,
        &Command::MoveClip(MoveClip {
            clip_id: first,
            // 150_000 + 100_000 = 250_000 — straddles the second clip.
            new_start: rt(150_000),
        }),
    )
    .unwrap_err();
    assert!(matches!(err, TimelineError::ClipOverlap { .. }));
    assert_eq!(
        p.sequence.tracks[0].clips[0].start,
        rt(0),
        "failed move must not mutate"
    );
}

#[test]
fn move_clip_rejects_negative_start() {
    let (mut p, _, _, clip_ids) = project_with_clips(&[(100_000, 100_000)]);

    let err = apply(
        &mut p,
        &Command::MoveClip(MoveClip {
            clip_id: clip_ids[0],
            new_start: rt(-1),
        }),
    )
    .unwrap_err();
    assert!(matches!(err, TimelineError::InvalidTime { .. }));
}

// ---------------------------------------------------------------------------
// SplitClip
// ---------------------------------------------------------------------------

#[test]
fn split_clip_partitions_timeline_and_source_at_speed_one() {
    let (mut p, track_id, media_id, _) = project_with_clips(&[]);

    // Build a clip whose source range starts at a non-zero offset so the
    // test catches anyone who hard-codes `source_in == 0`.
    let original_id = ClipId::new();
    apply(
        &mut p,
        &Command::AddClip(AddClip {
            track_id,
            clip: Clip {
                source_in: rt(50_000),
                source_out: rt(50_000 + 200_000),
                ..clip(original_id, track_id, Some(media_id), 0, 200_000)
            },
        }),
    )
    .unwrap();

    let right_id = ClipId::new();
    let effect = apply(
        &mut p,
        &Command::SplitClip(SplitClip {
            clip_id: original_id,
            at: rt(80_000),
            right_clip_id: right_id,
        }),
    )
    .unwrap();

    assert!(matches!(
        effect,
        CommandEffect::SplitClip(e)
            if e.left_clip_id == original_id && e.right_clip_id == right_id
    ));

    let track = &p.sequence.tracks[0];
    assert_eq!(track.clips.len(), 2);
    let left = &track.clips[0];
    let right = &track.clips[1];

    assert_eq!(left.id, original_id);
    assert_eq!(left.start, rt(0));
    assert_eq!(left.duration, rt(80_000));
    assert_eq!(left.source_in, rt(50_000));
    assert_eq!(left.source_out, rt(50_000 + 80_000));

    assert_eq!(right.id, right_id);
    assert_eq!(right.start, rt(80_000));
    assert_eq!(right.duration, rt(120_000));
    assert_eq!(
        right.source_in,
        rt(50_000 + 80_000),
        "right.source_in continues where left.source_out ended"
    );
    assert_eq!(right.source_out, rt(50_000 + 200_000));
}

#[test]
fn split_clip_rejects_split_at_clip_edge() {
    let (mut p, _, _, clip_ids) = project_with_clips(&[(0, 100_000)]);

    for at in [rt(0), rt(100_000)] {
        let err = apply(
            &mut p,
            &Command::SplitClip(SplitClip {
                clip_id: clip_ids[0],
                at,
                right_clip_id: ClipId::new(),
            }),
        )
        .unwrap_err();
        assert!(matches!(err, TimelineError::InvalidSplit { .. }));
    }
    assert_eq!(p.sequence.tracks[0].clips.len(), 1, "no edits on failure");
}

#[test]
fn split_clip_rejects_speed_not_one() {
    let (mut p, track_id, media_id, _) = project_with_clips(&[]);
    let id = ClipId::new();
    apply(
        &mut p,
        &Command::AddClip(AddClip {
            track_id,
            clip: Clip {
                speed: Rational::new_raw(1, 2),
                ..clip(id, track_id, Some(media_id), 0, 100_000)
            },
        }),
    )
    .unwrap();

    let err = apply(
        &mut p,
        &Command::SplitClip(SplitClip {
            clip_id: id,
            at: rt(50_000),
            right_clip_id: ClipId::new(),
        }),
    )
    .unwrap_err();
    assert!(
        matches!(err, TimelineError::InvalidTrim { .. }),
        "varispeed split is documented-deferred and must error: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// TrimClipIn / TrimClipOut
// ---------------------------------------------------------------------------

#[test]
fn trim_clip_in_shifts_start_and_source_in_keeping_right_edge() {
    // Original: start=100_000, duration=200_000, source_in=10_000, source_out=210_000
    // Right edge on timeline = 300_000.
    let (mut p, track_id, media_id, _) = project_with_clips(&[]);
    let id = ClipId::new();
    apply(
        &mut p,
        &Command::AddClip(AddClip {
            track_id,
            clip: Clip {
                source_in: rt(10_000),
                source_out: rt(210_000),
                ..clip(id, track_id, Some(media_id), 100_000, 200_000)
            },
        }),
    )
    .unwrap();

    // Trim head right by 30_000 (new source_in = 40_000).
    apply(
        &mut p,
        &Command::TrimClipIn(TrimClipIn {
            clip_id: id,
            new_source_in: rt(40_000),
        }),
    )
    .unwrap();

    let c = &p.sequence.tracks[0].clips[0];
    assert_eq!(c.source_in, rt(40_000));
    assert_eq!(
        c.start,
        rt(130_000),
        "start shifts right by the same delta as source_in"
    );
    assert_eq!(c.duration, rt(170_000), "duration shrinks by the trim delta");
    assert_eq!(
        c.start.num + c.duration.num,
        300_000,
        "timeline right edge is anchored"
    );
}

#[test]
fn trim_clip_in_can_extend_when_neighbour_allows() {
    let (mut p, track_id, media_id, _) = project_with_clips(&[]);
    let id = ClipId::new();
    apply(
        &mut p,
        &Command::AddClip(AddClip {
            track_id,
            clip: Clip {
                source_in: rt(50_000),
                source_out: rt(150_000),
                ..clip(id, track_id, Some(media_id), 100_000, 100_000)
            },
        }),
    )
    .unwrap();

    // Roll head left to source_in = 0 → start moves to 50_000, duration grows to 150_000.
    apply(
        &mut p,
        &Command::TrimClipIn(TrimClipIn {
            clip_id: id,
            new_source_in: rt(0),
        }),
    )
    .unwrap();

    let c = &p.sequence.tracks[0].clips[0];
    assert_eq!(c.start, rt(50_000));
    assert_eq!(c.duration, rt(150_000));
    assert_eq!(c.source_in, rt(0));
}

#[test]
fn trim_clip_in_rejects_overlap_when_extending_left() {
    let (mut p, _, _, ids) = project_with_clips(&[(0, 100_000), (200_000, 100_000)]);
    let right_id = ids[1];

    // Extending the right clip's head leftward would collide with the left one.
    let err = apply(
        &mut p,
        &Command::TrimClipIn(TrimClipIn {
            clip_id: right_id,
            // Current source_in == 0 for this fixture (built via project_with_clips),
            // so the only way to push start leftward through `new_source_in` is to
            // make it negative.
            new_source_in: rt(-50_000),
        }),
    )
    .unwrap_err();
    assert!(matches!(err, TimelineError::InvalidTrim { .. }));
}

#[test]
fn trim_clip_out_extends_right_edge() {
    let (mut p, track_id, media_id, _) = project_with_clips(&[]);
    let id = ClipId::new();
    apply(
        &mut p,
        &Command::AddClip(AddClip {
            track_id,
            clip: Clip {
                source_in: rt(0),
                source_out: rt(100_000),
                ..clip(id, track_id, Some(media_id), 0, 100_000)
            },
        }),
    )
    .unwrap();

    apply(
        &mut p,
        &Command::TrimClipOut(TrimClipOut {
            clip_id: id,
            new_source_out: rt(180_000),
        }),
    )
    .unwrap();

    let c = &p.sequence.tracks[0].clips[0];
    assert_eq!(c.duration, rt(180_000));
    assert_eq!(c.source_out, rt(180_000));
    assert_eq!(c.start, rt(0), "start is unchanged by trim-out");
    assert_eq!(
        p.sequence.duration.num,
        180_000,
        "extending the right edge bumps the sequence duration"
    );
}

#[test]
fn trim_clip_out_rejects_overlap_with_next_clip() {
    let (mut p, _, _, ids) = project_with_clips(&[(0, 100_000), (150_000, 100_000)]);

    let err = apply(
        &mut p,
        &Command::TrimClipOut(TrimClipOut {
            clip_id: ids[0],
            new_source_out: rt(200_000),
        }),
    )
    .unwrap_err();
    assert!(matches!(err, TimelineError::ClipOverlap { .. }));
}

#[test]
fn trim_clip_out_rejects_collapse_to_zero() {
    let (mut p, _, _, ids) = project_with_clips(&[(0, 100_000)]);

    let err = apply(
        &mut p,
        &Command::TrimClipOut(TrimClipOut {
            clip_id: ids[0],
            new_source_out: rt(0),
        }),
    )
    .unwrap_err();
    assert!(matches!(err, TimelineError::InvalidTrim { .. }));
}

// ---------------------------------------------------------------------------
// Atomicity contract
// ---------------------------------------------------------------------------

/// Crash-test the all-or-nothing promise: a command that fails midway must
/// leave the project byte-for-byte identical. We don't have `PartialEq` on
/// the domain types so we compare field-by-field on the parts the failing
/// command would have touched.
#[test]
fn failed_command_leaves_project_unchanged() {
    let (mut p, track_id, media_id, ids) = project_with_clips(&[(0, 100_000), (200_000, 100_000)]);

    let before = (
        p.sequence.tracks[0].clips[0].start,
        p.sequence.tracks[0].clips[0].duration,
        p.sequence.tracks[0].clips[0].source_in,
        p.sequence.tracks[0].clips[1].start,
        p.sequence.duration,
    );

    // A clearly-invalid AddClip (overlap) — must not write.
    let dup_id = ClipId::new();
    let err = apply(
        &mut p,
        &Command::AddClip(AddClip {
            track_id,
            clip: clip(dup_id, track_id, Some(media_id), 50_000, 100_000),
        }),
    )
    .unwrap_err();
    assert!(matches!(err, TimelineError::ClipOverlap { .. }));

    let after = (
        p.sequence.tracks[0].clips[0].start,
        p.sequence.tracks[0].clips[0].duration,
        p.sequence.tracks[0].clips[0].source_in,
        p.sequence.tracks[0].clips[1].start,
        p.sequence.duration,
    );
    assert_eq!(before, after);
    assert_eq!(p.sequence.tracks[0].clips.len(), 2);
    assert!(p.sequence.tracks[0].clips.iter().all(|c| c.id != dup_id));
    assert!(ids.iter().all(|id| p
        .sequence
        .tracks
        .iter()
        .any(|t| t.clips.iter().any(|c| c.id == *id))));
}

// ---------------------------------------------------------------------------
// Effect-driven inversion (proxy for the future undo layer)
// ---------------------------------------------------------------------------

/// Simulate a hand-rolled undo for `SplitClip` using only `CommandEffect`
/// data. If this test ever breaks, the future `History` layer will too —
/// the effect is what undo will consume.
#[test]
fn split_clip_effect_carries_enough_data_to_undo() {
    let (mut p, _, _, ids) = project_with_clips(&[(0, 200_000)]);
    let original = ids[0];
    let right_id = ClipId::new();

    let effect = apply(
        &mut p,
        &Command::SplitClip(SplitClip {
            clip_id: original,
            at: rt(80_000),
            right_clip_id: right_id,
        }),
    )
    .unwrap();

    let CommandEffect::SplitClip(effect) = effect else {
        panic!("expected SplitClip effect");
    };

    // Hand-invert: drop the right piece, restore the left's duration / source_out.
    apply(
        &mut p,
        &Command::RemoveClip(RemoveClip {
            clip_id: effect.right_clip_id,
        }),
    )
    .unwrap();
    apply(
        &mut p,
        &Command::TrimClipOut(TrimClipOut {
            clip_id: effect.left_clip_id,
            new_source_out: effect.prev_source_out,
        }),
    )
    .unwrap();

    let track = &p.sequence.tracks[0];
    assert_eq!(track.clips.len(), 1);
    assert_eq!(track.clips[0].id, original);
    assert_eq!(track.clips[0].duration, effect.prev_duration);
    assert_eq!(track.clips[0].source_out, effect.prev_source_out);
}
