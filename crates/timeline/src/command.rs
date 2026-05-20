//! Structured edit commands and the effect record [`crate::apply`] returns.
//!
//! Each command variant lives in its own module under [`crate::commands`],
//! alongside the corresponding effect type and apply logic.

use crate::commands::{
    AddClip, AddClipEffect, AddTrack, AddTrackEffect, MoveClip, MoveClipEffect, RemoveClip,
    RemoveClipEffect, SplitClip, SplitClipEffect, TrimClipIn, TrimClipInEffect, TrimClipOut,
    TrimClipOutEffect,
};

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

/// One atomic edit. Failing commands leave the project unchanged.
///
/// The variants are intentionally small and orthogonal. Higher-level
/// gestures (e.g. "ripple delete") compose multiple `Command`s through
/// the future history layer.
#[derive(Debug, Clone)]
pub enum Command {
    /// Append a fresh track to the end of the sequence.
    ///
    /// `track_id` is caller-supplied to keep the command deterministic
    /// (agent replays, fixture tests, redo).
    AddTrack(AddTrack),

    /// Insert a clip on `track_id`. The `clip.track_id` field must match
    /// `track_id` — agents that build the clip and the command in two
    /// places typically forget to keep them in sync, so we surface that
    /// mismatch as [`crate::TimelineError::ClipTrackMismatch`] rather
    /// than silently rewriting the clip.
    AddClip(AddClip),

    /// Remove the clip with `clip_id` from whichever track holds it.
    RemoveClip(RemoveClip),

    /// Move the clip with `clip_id` so its `start` becomes `new_start`.
    /// Duration is unchanged. Fails on overlap with the moved clip's
    /// neighbours.
    MoveClip(MoveClip),

    /// Split the clip with `clip_id` at the timeline coordinate `at`.
    /// The left piece keeps the original id; the right piece gets
    /// `right_clip_id`. `at` must lie strictly inside the clip's
    /// `[start, start + duration)` interval.
    SplitClip(SplitClip),

    /// Shrink the clip from its left edge by changing `source_in` to
    /// `new_source_in`. The right edge on the timeline stays put;
    /// `start` and `duration` shift to keep that anchor.
    TrimClipIn(TrimClipIn),

    /// Extend or shrink the clip from its right edge by changing
    /// `source_out` to `new_source_out`. `start` is unchanged.
    TrimClipOut(TrimClipOut),
}

// ---------------------------------------------------------------------------
// CommandEffect
// ---------------------------------------------------------------------------

/// What [`crate::apply`] did. Carries the data needed to invert the
/// command — the future history layer turns this into `undo`.
///
/// Each variant pins down (a) which entities changed and (b) the *prior*
/// values of every field that moved. Storing the snapshot rather than the
/// `Command` is deliberate: redoing a TrimClipIn from the original command
/// is straightforward, but undoing it needs the old `(start, source_in,
/// duration)` triple even though only one of those was a command argument.
#[derive(Debug, Clone)]
pub enum CommandEffect {
    AddTrack(AddTrackEffect),
    AddClip(AddClipEffect),
    RemoveClip(RemoveClipEffect),
    MoveClip(MoveClipEffect),
    SplitClip(SplitClipEffect),
    TrimClipIn(TrimClipInEffect),
    TrimClipOut(TrimClipOutEffect),
}
