//! Drag placement helpers operating on the Slint view model.
//!
//! While the user drags (a clip, or a library tile), the gesture layer calls
//! these every frame; the same resolution is used for the live ghost and for
//! the commit on release, so the preview can never disagree with the drop.
//!
//! Policy follows CapCut:
//! - magnet snapping pulls the dragged edges to clip edges on *all* lanes,
//!   the playhead, and tick 0, with a vertical guide line at the snap point;
//! - lanes only accept their own kind; hovering a foreign-kind lane, empty
//!   space, or a conflicting (overlapping) span resolves to a *new lane*
//!   inserted at the hovered row.

use slint::Model;

use crate::{ClipDragResolution, ClipTrimResolution, Sequence, SnapResult, TrackKind};

pub fn compute_drag_snap(
    sequence: &Sequence,
    dragging_source_track_id: &str,
    dragging_clip_id: &str,
    cursor_start_value: i32,
    clip_duration_ticks: i32,
    snap_threshold_ticks: i32,
    playhead_tick: i32,
) -> SnapResult {
    if snap_threshold_ticks <= 0 {
        return SnapResult::none(cursor_start_value);
    }

    let cursor_end = cursor_start_value.saturating_add(clip_duration_ticks);
    let mut best: Option<(i32, i32, i32)> = None;

    let mut consider = |candidate: i32| {
        let d_leading = (candidate - cursor_start_value).abs();
        if d_leading <= snap_threshold_ticks && best.map_or(true, |(d, _, _)| d_leading < d) {
            best = Some((d_leading, candidate, candidate));
        }
        let d_trailing = (candidate - cursor_end).abs();
        if d_trailing <= snap_threshold_ticks && best.map_or(true, |(d, _, _)| d_trailing < d) {
            let snapped_start = candidate.saturating_sub(clip_duration_ticks);
            best = Some((d_trailing, snapped_start, candidate));
        }
    };

    consider(0);
    consider(playhead_tick);

    for track_idx in 0..sequence.tracks.row_count() {
        let Some(track) = sequence.tracks.row_data(track_idx) else {
            continue;
        };
        for clip_idx in 0..track.clips.row_count() {
            let Some(clip) = track.clips.row_data(clip_idx) else {
                continue;
            };
            if track.id == dragging_source_track_id && clip.id == dragging_clip_id {
                continue;
            }
            let s = clip.timeline_start.value;
            let e = s.saturating_add(clip.source_range.duration.value);
            consider(s);
            consider(e);
        }
    }

    match best {
        None => SnapResult::none(cursor_start_value),
        Some((_, snapped_start, line)) => SnapResult {
            has_snap: true,
            snapped_start_value: snapped_start,
            snap_line_tick: line,
        },
    }
}

/// Resolve where a clip being dragged by (`dx_ticks`, `hover_row`) would land.
///
/// `hover_row` is the lane-list row under the dragged clip's vertical center
/// (top-first; may be out of range, meaning above/below the existing lanes).
/// O(total clips) per call — evaluated per drag frame, fine at editing scale.
pub fn resolve_clip_drag(
    sequence: &Sequence,
    source_track_id: &str,
    dragging_clip_id: &str,
    dx_ticks: i32,
    hover_row: i32,
    playhead_tick: i32,
    snap_threshold_ticks: i32,
) -> ClipDragResolution {
    let track_count = sequence.tracks.row_count() as i32;
    let Some((source_kind, orig_start, duration)) =
        find_dragged_clip(sequence, source_track_id, dragging_clip_id)
    else {
        return ClipDragResolution::invalid();
    };

    let desired = (orig_start.saturating_add(dx_ticks)).max(0);
    let snap = compute_drag_snap(
        sequence,
        source_track_id,
        dragging_clip_id,
        desired,
        duration,
        snap_threshold_ticks,
        playhead_tick,
    );
    let snapped = snap.snapped_start_value.max(0);

    // Hovered lane accepts the clip only when kinds match.
    let hover_track = (0..track_count)
        .contains(&hover_row)
        .then(|| sequence.tracks.row_data(hover_row as usize))
        .flatten()
        .filter(|t| t.kind == source_kind);

    if let Some(track) = hover_track {
        let exclude = (track.id == source_track_id).then_some(dragging_clip_id);
        if span_is_free(&track, snapped, duration, exclude) {
            return ClipDragResolution {
                valid: true,
                is_new_lane: false,
                target_track_id: track.id.clone(),
                target_row: hover_row,
                resolved_start: snapped,
                duration_ticks: duration,
                has_snap: snap.has_snap,
                snap_line_tick: snap.snap_line_tick,
                is_noop: track.id == source_track_id && snapped == orig_start,
            };
        }
        // The snap pulled us into a conflict the raw position doesn't have —
        // prefer landing free without the magnet over forcing a new lane.
        if snap.has_snap && snapped != desired && span_is_free(&track, desired, duration, exclude)
        {
            return ClipDragResolution {
                valid: true,
                is_new_lane: false,
                target_track_id: track.id.clone(),
                target_row: hover_row,
                resolved_start: desired,
                duration_ticks: duration,
                has_snap: false,
                snap_line_tick: 0,
                is_noop: track.id == source_track_id && desired == orig_start,
            };
        }
    }

    // Foreign kind, out of range, or conflicting span: a new lane is inserted
    // at the hovered row (clamped to just above / just below the stack).
    ClipDragResolution {
        valid: true,
        is_new_lane: true,
        target_track_id: Default::default(),
        target_row: hover_row.clamp(0, track_count),
        resolved_start: snapped,
        duration_ticks: duration,
        has_snap: snap.has_snap,
        snap_line_tick: snap.snap_line_tick,
        is_noop: false,
    }
}

/// Resolve an edge-trim drag of `dx_ticks` (`trim_head` ⇔ the left edge).
///
/// The dragged edge magnets to clip edges / the playhead / tick 0, then
/// clamps to (in CapCut order of feel): the neighboring clips on the lane,
/// the source media headroom (`head-room-ticks` / `tail-room-ticks` from the
/// projection), tick 0, and a 1-tick minimum duration. A snap that the clamp
/// rejects is dropped (no guide line) rather than shown lying.
pub fn resolve_clip_trim(
    sequence: &Sequence,
    track_id: &str,
    clip_id: &str,
    trim_head: bool,
    dx_ticks: i32,
    playhead_tick: i32,
    snap_threshold_ticks: i32,
) -> ClipTrimResolution {
    let Some(ctx) = trim_context(sequence, track_id, clip_id) else {
        return ClipTrimResolution::invalid();
    };
    let old_end = ctx.start.saturating_add(ctx.duration);

    // Magnet the dragged edge alone: duration 0 turns the (start, end) pair
    // snap into a single-point snap.
    let desired = if trim_head { ctx.start } else { old_end }.saturating_add(dx_ticks);
    let snap = compute_drag_snap(
        sequence,
        track_id,
        clip_id,
        desired,
        0,
        snap_threshold_ticks,
        playhead_tick,
    );

    // Both ranges are non-empty: `lo ≤ start < end ≤ hi` holds by
    // construction (neighbors can't overlap the clip, rooms are ≥ 0).
    let (lo, hi) = if trim_head {
        let source_lo = ctx.start.saturating_sub(ctx.head_room);
        (ctx.prev_end.max(source_lo).max(0), old_end - 1)
    } else {
        let source_hi = old_end.saturating_add(ctx.tail_room);
        (ctx.start + 1, ctx.next_start.min(source_hi))
    };
    let edge = snap.snapped_start_value.clamp(lo, hi);
    let has_snap = snap.has_snap && edge == snap.snapped_start_value;

    let (new_start, new_duration) = if trim_head {
        (edge, old_end - edge)
    } else {
        (ctx.start, edge - ctx.start)
    };

    ClipTrimResolution {
        valid: true,
        new_start,
        new_duration,
        has_snap,
        snap_line_tick: if has_snap { snap.snap_line_tick } else { 0 },
        is_noop: new_start == ctx.start && new_duration == ctx.duration,
    }
}

/// The trimmed clip's placement, source headroom, and lane neighbors.
struct TrimContext {
    start: i32,
    duration: i32,
    head_room: i32,
    tail_room: i32,
    /// End of the nearest clip left of this one on the lane (0 if none).
    prev_end: i32,
    /// Start of the nearest clip right of this one (huge sentinel if none).
    next_start: i32,
}

fn trim_context(sequence: &Sequence, track_id: &str, clip_id: &str) -> Option<TrimContext> {
    let track = (0..sequence.tracks.row_count())
        .filter_map(|i| sequence.tracks.row_data(i))
        .find(|t| t.id == track_id)?;
    let clip = (0..track.clips.row_count())
        .filter_map(|i| track.clips.row_data(i))
        .find(|c| c.id == clip_id)?;

    let start = clip.timeline_start.value;
    let duration = clip.source_range.duration.value;
    let end = start.saturating_add(duration);

    let mut prev_end = 0;
    let mut next_start = i32::MAX / 2;
    for idx in 0..track.clips.row_count() {
        let Some(other) = track.clips.row_data(idx) else {
            continue;
        };
        if other.id == clip_id {
            continue;
        }
        let s = other.timeline_start.value;
        let e = s.saturating_add(other.source_range.duration.value);
        // Lane clips never overlap, so every other clip is fully on one side.
        if e <= start {
            prev_end = prev_end.max(e);
        }
        if s >= end {
            next_start = next_start.min(s);
        }
    }

    Some(TrimContext {
        start,
        duration,
        head_room: clip.head_room_ticks.max(0),
        tail_room: clip.tail_room_ticks.max(0),
        prev_end,
        next_start,
    })
}

/// `(track kind, timeline start, duration ticks)` of the dragged clip.
fn find_dragged_clip(
    sequence: &Sequence,
    source_track_id: &str,
    clip_id: &str,
) -> Option<(TrackKind, i32, i32)> {
    for track_idx in 0..sequence.tracks.row_count() {
        let track = sequence.tracks.row_data(track_idx)?;
        if track.id != source_track_id {
            continue;
        }
        for clip_idx in 0..track.clips.row_count() {
            let clip = track.clips.row_data(clip_idx)?;
            if clip.id == clip_id {
                return Some((
                    track.kind,
                    clip.timeline_start.value,
                    clip.source_range.duration.value,
                ));
            }
        }
    }
    None
}

/// Whether `[start, start + duration)` overlaps no clip on `track`
/// (excluding the dragged clip itself when moving within its own lane).
fn span_is_free(track: &crate::Track, start: i32, duration: i32, exclude: Option<&str>) -> bool {
    let end = start.saturating_add(duration);
    for clip_idx in 0..track.clips.row_count() {
        let Some(clip) = track.clips.row_data(clip_idx) else {
            continue;
        };
        if exclude.is_some_and(|id| clip.id == id) {
            continue;
        }
        let s = clip.timeline_start.value;
        let e = s.saturating_add(clip.source_range.duration.value);
        if start < e && s < end {
            return false;
        }
    }
    true
}

impl SnapResult {
    fn none(cursor: i32) -> Self {
        Self {
            has_snap: false,
            snapped_start_value: cursor,
            snap_line_tick: cursor,
        }
    }
}

impl ClipDragResolution {
    fn invalid() -> Self {
        Self {
            valid: false,
            is_new_lane: false,
            target_track_id: Default::default(),
            target_row: 0,
            resolved_start: 0,
            duration_ticks: 0,
            has_snap: false,
            snap_line_tick: 0,
            is_noop: true,
        }
    }
}

impl ClipTrimResolution {
    fn invalid() -> Self {
        Self {
            valid: false,
            new_start: 0,
            new_duration: 0,
            has_snap: false,
            snap_line_tick: 0,
            is_noop: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, Rational, RationalTime, Sequence, TimeRange, Track, TrackKind};
    use slint::{ModelRc, SharedString, VecModel};
    use std::rc::Rc;

    fn rt(value: i32) -> RationalTime {
        RationalTime {
            value,
            rate: Rational { num: 24, den: 1 },
        }
    }

    /// A clip with effectively unbounded source headroom on both edges.
    fn clip(id: &str, start: i32, dur: i32) -> Clip {
        clip_with_rooms(id, start, dur, 1_000_000, 1_000_000)
    }

    fn clip_with_rooms(id: &str, start: i32, dur: i32, head_room: i32, tail_room: i32) -> Clip {
        Clip {
            id: SharedString::from(id),
            name: SharedString::from(id),
            timeline_start: rt(start),
            source_range: TimeRange {
                start: rt(0),
                duration: rt(dur),
            },
            text_content: Default::default(),
            head_room_ticks: head_room,
            tail_room_ticks: tail_room,
        }
    }

    fn track(id: &str, kind: TrackKind, clips: Vec<Clip>) -> Track {
        Track {
            id: SharedString::from(id),
            name: SharedString::from(id),
            kind,
            color: slint::Color::from_rgb_u8(0x4A, 0x6F, 0xA5),
            clips: ModelRc::from(Rc::new(VecModel::from(clips))),
        }
    }

    fn sequence(tracks: Vec<Track>) -> Sequence {
        Sequence {
            id: SharedString::from("1"),
            name: SharedString::from("Sequence 1"),
            fps: Rational { num: 24, den: 1 },
            drop_frame: false,
            tracks: ModelRc::from(Rc::new(VecModel::from(tracks))),
            width: 1920.0,
            height: 1080.0,
        }
    }

    /// Rows (top-first): 0 = V2 video, 1 = V1 video, 2 = A1 audio.
    fn sample_sequence() -> Sequence {
        sequence(vec![
            track("2", TrackKind::Video, vec![clip("2", 0, 80), clip("3", 200, 60)]),
            track("1", TrackKind::Video, vec![clip("1", 10, 100)]),
            track("9", TrackKind::Audio, vec![]),
        ])
    }

    // --- compute_drag_snap --------------------------------------------------

    #[test]
    fn snaps_to_zero_origin_when_dragging_near_start() {
        let seq = sample_sequence();
        let r = compute_drag_snap(&seq, "1", "1", 3, 100, 5, 0);
        assert!(r.has_snap);
        assert_eq!(r.snapped_start_value, 0);
    }

    #[test]
    fn snaps_to_playhead() {
        let seq = sample_sequence();
        let r = compute_drag_snap(&seq, "1", "1", 202, 100, 5, 200);
        assert!(r.has_snap);
        assert_eq!(r.snapped_start_value, 200);
        assert_eq!(r.snap_line_tick, 200);
    }

    #[test]
    fn zero_threshold_disables_snapping() {
        let seq = sample_sequence();
        let r = compute_drag_snap(&seq, "1", "1", 3, 100, 0, 0);
        assert!(!r.has_snap);
        assert_eq!(r.snapped_start_value, 3);
    }

    // --- resolve_clip_drag --------------------------------------------------

    #[test]
    fn free_spot_on_same_lane_lands_there() {
        let seq = sample_sequence();
        // Clip "1" (row 1, start 10, dur 100) dragged right by 200 ticks.
        let r = resolve_clip_drag(&seq, "1", "1", 200, 1, 0, 5);
        assert!(r.valid && !r.is_new_lane);
        assert_eq!(r.target_track_id, "1");
        assert_eq!(r.resolved_start, 210);
        assert!(!r.is_noop);
    }

    #[test]
    fn unmoved_drag_is_noop() {
        let seq = sample_sequence();
        let r = resolve_clip_drag(&seq, "1", "1", 0, 1, 0, 0);
        assert!(r.valid && !r.is_new_lane && r.is_noop);
    }

    #[test]
    fn conflict_on_hovered_lane_creates_new_lane_at_row() {
        let seq = sample_sequence();
        // Drag clip "1" up onto row 0 at a position overlapping clip "2" [0,80).
        let r = resolve_clip_drag(&seq, "1", "1", 20, 0, 0, 0);
        assert!(r.valid && r.is_new_lane);
        assert_eq!(r.target_row, 0);
        assert_eq!(r.resolved_start, 30);
    }

    #[test]
    fn edge_snap_resolves_conflict_into_abutment() {
        let seq = sample_sequence();
        // Dragging clip "1" so its start sits at 78 — overlaps "2" [0,80) by 2
        // ticks, but the magnet pulls it to abut at 80, which is free.
        let r = resolve_clip_drag(&seq, "1", "1", 68, 0, 0, 5);
        assert!(r.valid && !r.is_new_lane);
        assert_eq!(r.target_track_id, "2");
        assert_eq!(r.resolved_start, 80);
        assert!(r.has_snap);
    }

    #[test]
    fn foreign_kind_lane_creates_new_lane() {
        let seq = sample_sequence();
        // Video clip hovered over the audio lane (row 2).
        let r = resolve_clip_drag(&seq, "1", "1", 200, 2, 0, 0);
        assert!(r.valid && r.is_new_lane);
        assert_eq!(r.target_row, 2);
    }

    #[test]
    fn rows_outside_stack_clamp_to_top_and_bottom_insertion() {
        let seq = sample_sequence();
        let above = resolve_clip_drag(&seq, "1", "1", 200, -3, 0, 0);
        assert!(above.is_new_lane);
        assert_eq!(above.target_row, 0);

        let below = resolve_clip_drag(&seq, "1", "1", 200, 9, 0, 0);
        assert!(below.is_new_lane);
        assert_eq!(below.target_row, 3);
    }

    #[test]
    fn unknown_ids_resolve_invalid() {
        let seq = sample_sequence();
        let r = resolve_clip_drag(&seq, "1", "404", 0, 1, 0, 0);
        assert!(!r.valid);
    }

    // --- resolve_clip_trim ----------------------------------------------------

    #[test]
    fn tail_trim_extends_until_next_clip() {
        let seq = sample_sequence();
        // Clip "2" [0,80) on track "2"; clip "3" starts at 200 on the same lane.
        let r = resolve_clip_trim(&seq, "2", "2", false, 300, 0, 0);
        assert!(r.valid);
        assert_eq!(r.new_start, 0);
        assert_eq!(r.new_duration, 200);
        assert!(!r.is_noop);
    }

    #[test]
    fn head_trim_extends_until_previous_clip() {
        let seq = sample_sequence();
        // Clip "3" [200,260) on track "2"; clip "2" ends at 80 on the same lane.
        let r = resolve_clip_trim(&seq, "2", "3", true, -500, 0, 0);
        assert!(r.valid);
        assert_eq!(r.new_start, 80);
        assert_eq!(r.new_duration, 180);
    }

    #[test]
    fn head_trim_clamps_to_source_headroom() {
        let seq = sequence(vec![track(
            "1",
            TrackKind::Video,
            vec![clip_with_rooms("1", 10, 100, 5, 1_000_000)],
        )]);
        let r = resolve_clip_trim(&seq, "1", "1", true, -50, 0, 0);
        assert!(r.valid);
        assert_eq!(r.new_start, 5);
        assert_eq!(r.new_duration, 105);
    }

    #[test]
    fn tail_trim_clamps_to_source_tailroom() {
        let seq = sequence(vec![track(
            "1",
            TrackKind::Video,
            vec![clip_with_rooms("1", 10, 100, 0, 7)],
        )]);
        let r = resolve_clip_trim(&seq, "1", "1", false, 50, 0, 0);
        assert!(r.valid);
        assert_eq!(r.new_start, 10);
        assert_eq!(r.new_duration, 107);
    }

    #[test]
    fn trim_never_collapses_below_one_tick() {
        let seq = sample_sequence();
        let head = resolve_clip_trim(&seq, "2", "2", true, 1_000, 0, 0);
        assert_eq!(head.new_start, 79);
        assert_eq!(head.new_duration, 1);

        let tail = resolve_clip_trim(&seq, "2", "2", false, -1_000, 0, 0);
        assert_eq!(tail.new_start, 0);
        assert_eq!(tail.new_duration, 1);
    }

    #[test]
    fn trimmed_edge_snaps_to_playhead() {
        let seq = sample_sequence();
        // Clip "1" [10,110) on track "1": tail dragged to 148, playhead at 150.
        let r = resolve_clip_trim(&seq, "1", "1", false, 38, 150, 5);
        assert!(r.valid && r.has_snap);
        assert_eq!(r.new_duration, 140);
        assert_eq!(r.snap_line_tick, 150);
    }

    #[test]
    fn snap_beyond_clamp_is_dropped() {
        // Tail room of 2 caps the end at 102, but clip "B" at 105 is a magnet
        // candidate within threshold — the clamp wins and the guide hides.
        let seq = sequence(vec![
            track("1", TrackKind::Video, vec![clip_with_rooms("A", 0, 100, 0, 2)]),
            track("2", TrackKind::Video, vec![clip("B", 105, 50)]),
        ]);
        let r = resolve_clip_trim(&seq, "1", "A", false, 4, 0, 5);
        assert!(r.valid && !r.has_snap);
        assert_eq!(r.new_duration, 102);
    }

    #[test]
    fn unmoved_trim_is_noop() {
        let seq = sample_sequence();
        let r = resolve_clip_trim(&seq, "1", "1", true, 0, 0, 0);
        assert!(r.valid && r.is_noop);
    }

    #[test]
    fn unknown_trim_ids_resolve_invalid() {
        let seq = sample_sequence();
        let r = resolve_clip_trim(&seq, "1", "404", true, 0, 0, 0);
        assert!(!r.valid);
    }
}
