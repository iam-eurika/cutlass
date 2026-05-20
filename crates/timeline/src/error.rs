//! Errors surfaced by [`crate::apply`]. Each variant pins down which invariant
//! failed so the UI and the agent layer can produce useful diagnostics
//! (rather than a generic "edit refused").
//!
//! All variants are recoverable from the caller's perspective: nothing in
//! this crate panics, and `apply` is **all-or-nothing** — a returned error
//! leaves the project byte-identical to before the call.

use models::{ClipId, MediaId, TrackId};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TimelineError {
    #[error("track {0} not found")]
    TrackNotFound(TrackId),

    #[error("clip {0} not found")]
    ClipNotFound(ClipId),

    #[error("media source {0} not found in project bin")]
    SourceNotFound(MediaId),

    /// A clip's `[start, start + duration)` interval overlaps `existing_clip`
    /// on the same track. `attempted_start_num` / `attempted_end_num` are the
    /// numerators at the sequence's timebase so callers can render exact
    /// debug output without re-deriving the rational.
    #[error(
        "clip overlaps existing clip {existing_clip} on the same track (\
         attempted [{attempted_start_num}, {attempted_end_num}) at timebase {timebase})"
    )]
    ClipOverlap {
        existing_clip: ClipId,
        attempted_start_num: i64,
        attempted_end_num: i64,
        timebase: u32,
    },

    /// Trim left an invalid clip (duration ≤ 0, source range inverted, etc.).
    #[error("invalid trim: {reason}")]
    InvalidTrim { reason: &'static str },

    /// Split coordinate doesn't fall strictly inside the clip, or the
    /// resulting halves wouldn't be valid clips.
    #[error("invalid split: {reason}")]
    InvalidSplit { reason: &'static str },

    /// A time argument was structurally wrong (e.g. negative start).
    #[error("invalid time: {reason}")]
    InvalidTime { reason: &'static str },

    /// A `RationalTime` argument didn't match the sequence's canonical
    /// timebase. We refuse to rescale silently — see crate docs for the
    /// reasoning.
    #[error("timebase mismatch: sequence expects den={expected_den}, got den={got_den}")]
    TimebaseMismatch { expected_den: u32, got_den: u32 },

    /// Caller-supplied ID for a new entity collides with an existing one
    /// on the same track (or, for tracks, on the same sequence).
    #[error("duplicate {kind} id: {id}")]
    DuplicateId { kind: &'static str, id: String },

    /// `AddClip.clip.track_id` referenced a different track than the
    /// command's `track_id`. Refusing this catches agent bugs early.
    #[error("clip.track_id {clip_track} does not match command track_id {command_track}")]
    ClipTrackMismatch {
        clip_track: TrackId,
        command_track: TrackId,
    },
}
