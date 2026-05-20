//! The single entry point: [`apply`].
//!
//! Every editor mutation routes through here. The function is **atomic** —
//! it validates before it writes, and a returned `Err` leaves the project
//! byte-identical to the call site.
//!
//! ## Time arithmetic
//!
//! All `RationalTime`s passed to commands must match the sequence's
//! canonical timebase (`project.sequence.timebase`). Once that's
//! confirmed, every comparison and offset is plain `i64` numerator math —
//! exact, no float drift, and cheap.
//!
//! ## Speed != 1/1
//!
//! `SplitClip` / `TrimClipIn` / `TrimClipOut` need to map between timeline
//! duration and source duration via the clip's `speed`. MVP supports
//! `speed == 1/1` only and refuses other values with a clear error. The
//! math for varispeed lives in the doc next to the relevant variants
//! (`docs/timeline/research.md`) and gets implemented when varispeed UI
//! lands.

use models::Project;

use crate::command::{Command, CommandEffect};
use crate::commands::{
    add_clip, add_track, move_clip, remove_clip, split_clip, trim_clip_in, trim_clip_out,
};
use crate::error::TimelineError;

/// Apply one structured command to the project. On `Ok`, returns the
/// [`CommandEffect`] describing what changed (input for a future undo
/// stack). On `Err`, the project is unchanged.
pub fn apply(project: &mut Project, command: &Command) -> Result<CommandEffect, TimelineError> {
    match command {
        Command::AddTrack(c) => add_track::apply(project, c).map(CommandEffect::AddTrack),
        Command::AddClip(c) => add_clip::apply(project, c).map(CommandEffect::AddClip),
        Command::RemoveClip(c) => remove_clip::apply(project, c).map(CommandEffect::RemoveClip),
        Command::MoveClip(c) => move_clip::apply(project, c).map(CommandEffect::MoveClip),
        Command::SplitClip(c) => split_clip::apply(project, c).map(CommandEffect::SplitClip),
        Command::TrimClipIn(c) => trim_clip_in::apply(project, c).map(CommandEffect::TrimClipIn),
        Command::TrimClipOut(c) => trim_clip_out::apply(project, c).map(CommandEffect::TrimClipOut),
    }
}
