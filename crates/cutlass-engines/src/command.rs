//! The editor's command layer: a closed set of structured, deterministic edits
//! plus an undo/redo history.
//!
//! This is the executable surface the project's two front-ends drive: a UI
//! turns gestures into [`EditCommand`]s, and the AI agent turns a prompt into
//! the same commands. Because every edit is an explicit, serializable value
//! (not an ad-hoc mutation), edits are auditable and replayable, and undo is a
//! property of the layer rather than something each call site must remember.
//!
//! Commands are applied through [`Engine::apply`](crate::Engine::apply), which
//! validates against the model (rejecting overlaps / out-of-bounds edits) and,
//! on success, records a snapshot so the edit can be undone.

use cutlass_models::{ClipId, Generator, MediaId, TimeRange, Timeline, TrackId};

/// A single structured edit against the project's timeline.
///
/// Every variant maps to one invariant-preserving model mutation. Validation
/// happens at apply time, so constructing a command never fails; an illegal
/// edit surfaces as an `Err` from [`Engine::apply`](crate::Engine::apply) and
/// leaves the project untouched.
#[derive(Debug, Clone, PartialEq)]
pub enum EditCommand {
    /// Place a trimmed range of imported media on a track.
    AddClip {
        track: TrackId,
        media: MediaId,
        source: TimeRange,
        start: i64,
    },
    /// Place a generated clip (text, solid, shape, ...) on a track.
    AddGenerated {
        track: TrackId,
        generator: Generator,
        timeline: TimeRange,
    },
    /// Split a clip at a timeline frame into two abutting clips.
    SplitClip { clip: ClipId, at: i64 },
    /// Re-place / trim a clip to occupy `timeline` (adjusting its source range).
    TrimClip { clip: ClipId, timeline: TimeRange },
    /// Move a clip to `to_track` starting at `start`, keeping its duration.
    MoveClip {
        clip: ClipId,
        to_track: TrackId,
        start: i64,
    },
    /// Remove a clip, leaving a gap where it sat.
    RemoveClip { clip: ClipId },
    /// Remove a clip and slide later clips on its track left to close the gap.
    RippleDelete { clip: ClipId },
}

/// What an applied [`EditCommand`] produced, for the caller to act on (e.g. a UI
/// selecting the new clip after a split).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    /// A new clip was created (its id).
    Created(ClipId),
    /// An existing clip was modified in place (its id).
    Updated(ClipId),
    /// A clip was removed (its id).
    Removed(ClipId),
}

/// Undo/redo over timeline snapshots.
///
/// Edits only ever touch the [`Timeline`], so a snapshot of it is the complete
/// pre-edit state. Cloning a timeline is cheap relative to an edit's cadence
/// (clip metadata, not media), and the model is the hot path during playback —
/// not editing — so snapshotting here is clarity over a fiddly inverse-command
/// scheme. The redo stack is cleared whenever a fresh edit is recorded.
pub(crate) struct EditHistory {
    undo: Vec<Timeline>,
    redo: Vec<Timeline>,
    /// Cap on retained undo snapshots; oldest are dropped past this.
    limit: usize,
}

impl EditHistory {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// Record `pre_edit` as the state to restore on undo, invalidating redo.
    pub(crate) fn record(&mut self, pre_edit: Timeline) {
        self.redo.clear();
        self.undo.push(pre_edit);
        if self.undo.len() > self.limit {
            let excess = self.undo.len() - self.limit;
            self.undo.drain(0..excess);
        }
    }

    pub(crate) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub(crate) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Pop the most recent pre-edit snapshot, banking `current` for redo.
    pub(crate) fn undo(&mut self, current: Timeline) -> Option<Timeline> {
        let previous = self.undo.pop()?;
        self.redo.push(current);
        Some(previous)
    }

    /// Pop the most recent redo snapshot, banking `current` for undo.
    pub(crate) fn redo(&mut self, current: Timeline) -> Option<Timeline> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }
}
