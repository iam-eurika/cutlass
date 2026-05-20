//! Cutlass timeline brain.
//!
//! Pure data + transforms over [`models::Project`]. No I/O, no async, no
//! threads. Every editor mutation — whether typed by a user or emitted by
//! the AI agent layer — funnels through [`apply`] as a structured
//! [`Command`].
//!
//! ## Design tenets
//!
//! * **Plain-data commands.** [`Command`] is an `enum` of value structs,
//!   not a `Box<dyn Trait>`. That keeps it serializable end-to-end (when
//!   serde lands), inspectable in tests, and trivial for the agent layer
//!   to produce.
//! * **Caller-supplied IDs.** Anywhere a new entity is born we take the
//!   id from the command. Replays, agent-generated sequences, and the
//!   future redo stack all need this to be deterministic.
//! * **Atomic apply.** Validation runs to completion before any write;
//!   a returned [`TimelineError`] means the project is unchanged.
//! * **Inverse data, not derived.** [`CommandEffect`] carries the prior
//!   field values needed to undo — even fields that weren't command
//!   arguments. Re-deriving them later breaks if intervening edits
//!   touched the same clip.
//!
//! ## Out of scope (today)
//!
//! * Undo / redo history (the future `History` layer just wraps [`apply`]
//!   and stashes [`CommandEffect`]s).
//! * JSON serialization of commands.
//! * Snapping, ripple delete, drag coalescing.
//! * Varispeed clips — [`Command::SplitClip`] / [`Command::TrimClipIn`] /
//!   [`Command::TrimClipOut`] refuse `speed != 1/1` for now and the
//!   maths is documented in `docs/timeline/research.md`.

mod apply;
mod command;
mod commands;
mod error;
mod util;

pub use crate::apply::apply;
pub use crate::command::{Command, CommandEffect};
pub use crate::commands::{
    AddClip, AddTrack, MoveClip, RemoveClip, SplitClip, TrimClipIn, TrimClipOut,
};
pub use crate::error::TimelineError;
