//! Shared helpers for command apply functions.

use models::{Clip, ClipId, Rational, RationalTime, Sequence, Track, TrackId, TrackKind};

use crate::error::TimelineError;

/// Returns the numerator of `rt` at the sequence's timebase, or an error
/// if the rationals don't share a denominator. Refusing to silently
/// rescale is deliberate — an agent that emits the wrong unit will
/// otherwise produce silently-wrong edits.
#[inline]
pub(crate) fn at_timebase(rt: RationalTime, timebase: u32) -> Result<i64, TimelineError> {
    if rt.den == timebase {
        Ok(rt.num)
    } else {
        Err(TimelineError::TimebaseMismatch {
            expected_den: timebase,
            got_den: rt.den,
        })
    }
}

#[inline]
pub(crate) fn rt(num: i64, timebase: u32) -> RationalTime {
    RationalTime::new_raw(num, timebase)
}

pub(crate) fn default_track_height(kind: TrackKind) -> u32 {
    match kind {
        TrackKind::Video => 72,
        TrackKind::Audio => 48,
    }
}

/// Find the (track index, clip index) for a clip id. Linear scan across
/// all tracks — N ≤ low thousands of clips per project at the limits we
/// care about, and editing commands run at user-gesture cadence, not on
/// the per-frame hot path. Swap in a `HashMap<ClipId, (TrackId, usize)>`
/// index when profiling demands.
pub(crate) fn locate_clip(sequence: &Sequence, clip_id: ClipId) -> Option<(usize, usize)> {
    for (ti, track) in sequence.tracks.iter().enumerate() {
        for (ci, clip) in track.clips.iter().enumerate() {
            if clip.id == clip_id {
                return Some((ti, ci));
            }
        }
    }
    None
}

pub(crate) fn track_index(sequence: &Sequence, track_id: TrackId) -> Result<usize, TimelineError> {
    sequence
        .tracks
        .iter()
        .position(|t| t.id == track_id)
        .ok_or(TimelineError::TrackNotFound(track_id))
}

/// Check `[start, start + duration)` against every clip on the track
/// **except** the one at `exclude_index` (used by Move / Trim where the
/// clip being mutated still occupies its old slot during validation).
pub(crate) fn assert_no_overlap(
    track: &Track,
    timebase: u32,
    start_num: i64,
    duration_num: i64,
    exclude_index: Option<usize>,
) -> Result<(), TimelineError> {
    let end_num = start_num + duration_num;
    for (i, other) in track.clips.iter().enumerate() {
        if Some(i) == exclude_index {
            continue;
        }
        let o_start = other.start.num;
        let o_end = other.start.num + other.duration.num;
        // Half-open intervals: clips that touch at a boundary do NOT overlap.
        if start_num < o_end && o_start < end_num {
            return Err(TimelineError::ClipOverlap {
                existing_clip: other.id,
                attempted_start_num: start_num,
                attempted_end_num: end_num,
                timebase,
            });
        }
    }
    Ok(())
}

/// Re-sort a track's clips by `start.num`. Used after any operation that
/// can change a clip's start. Stable sort so siblings with equal starts
/// (shouldn't happen — overlap check forbids it — but defensive) preserve
/// order.
pub(crate) fn sort_track(track: &mut Track) {
    track.clips.sort_by_key(|c| c.start.num);
}

/// Cache the sequence duration as the max clip end across all tracks.
/// Cheap (single linear pass), and keeping it correct lets the rest of
/// the codebase trust `sequence.duration` without scanning clips.
pub(crate) fn recompute_sequence_duration(sequence: &mut Sequence) {
    let max_end = sequence
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .map(|c| c.start.num + c.duration.num)
        .max()
        .unwrap_or(0);
    sequence.duration = rt(max_end, sequence.timebase);
}

pub(crate) fn require_speed_one(clip: &Clip, reason: &'static str) -> Result<(), TimelineError> {
    if clip.speed == Rational::ONE {
        Ok(())
    } else {
        Err(TimelineError::InvalidTrim { reason })
    }
}
