//! Structured edit commands.
//!
//! Every domain mutation funnels through this enum — UI gestures and
//! the AI agent both speak the same vocabulary. The `#[serde(tag = "kind")]`
//! representation gives a tagged-JSON wire format that the agent can
//! emit directly:
//!
//! ```json
//! { "kind": "move_clip", "track_id": "1", "clip_id": "1", "new_start_value": 42 }
//! ```
//!
//! Variants here are intentionally minimal. `MoveClip` is the only
//! command implemented in this slice (cut/trim/split land later).

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::models::Project;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Command {
    /// Reposition `clip_id` inside `track_id` to start at
    /// `new_start_value` ticks at the clip's existing rate.
    ///
    /// Rate is intentionally **not** part of the command — a move
    /// never changes the clip's authoring rate, and inheriting it
    /// from the existing `timeline_start.rate` removes the only
    /// rounding step the hot drag path would otherwise need.
    MoveClip {
        track_id: String,
        clip_id: String,
        new_start_value: i32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    UnknownTrack { track_id: String },
    UnknownClip { track_id: String, clip_id: String },
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::UnknownTrack { track_id } => {
                write!(f, "unknown track {track_id:?}")
            }
            CommandError::UnknownClip { track_id, clip_id } => {
                write!(f, "unknown clip {clip_id:?} in track {track_id:?}")
            }
        }
    }
}

impl std::error::Error for CommandError {}

/// Apply `command` to `project` in place. Pure function over plain
/// Rust structs — no Slint dependency, no I/O — so this is where
/// command-level invariants are unit-tested.
///
/// Returns `Ok(Effect)` describing what changed so the projector
/// can update only the affected rows (instead of re-walking the
/// whole tree).
pub fn apply(project: &mut Project, command: &Command) -> Result<Effect, CommandError> {
    match command {
        Command::MoveClip {
            track_id,
            clip_id,
            new_start_value,
        } => {
            let track = project
                .sequence
                .tracks
                .get_mut(track_id.as_str())
                .ok_or_else(|| CommandError::UnknownTrack {
                    track_id: track_id.clone(),
                })?;
            let clip = track
                .clips
                .get_mut(clip_id.as_str())
                .ok_or_else(|| CommandError::UnknownClip {
                    track_id: track_id.clone(),
                    clip_id: clip_id.clone(),
                })?;
            clip.timeline_start.value = *new_start_value;
            Ok(Effect::ClipMoved {
                track_id: track_id.clone(),
                clip_id: clip_id.clone(),
                new_start_value: *new_start_value,
            })
        }
    }
}

/// What changed in the project as a result of applying a command.
///
/// Keeping this distinct from `Command` lets the projector update
/// only the surfaces that need it — a future structural command
/// (e.g. `SplitClip`) will produce an effect that mentions multiple
/// row indices without inflating the command vocabulary the agent
/// needs to learn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    ClipMoved {
        track_id: String,
        clip_id: String,
        new_start_value: i32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::sample_project;

    #[test]
    fn move_clip_updates_only_timeline_start_value() {
        let mut p = sample_project();

        // Snapshot the clip we're about to move + a neighbour to
        // assert other fields didn't drift.
        let target_before = p
            .sequence
            .tracks
            .get("1")
            .unwrap()
            .clips
            .get("1")
            .unwrap()
            .clone();
        let neighbour_before = p
            .sequence
            .tracks
            .get("2")
            .unwrap()
            .clips
            .get("2")
            .unwrap()
            .clone();

        let effect = apply(
            &mut p,
            &Command::MoveClip {
                track_id: "1".into(),
                clip_id: "1".into(),
                new_start_value: 999,
            },
        )
        .unwrap();

        assert_eq!(
            effect,
            Effect::ClipMoved {
                track_id: "1".into(),
                clip_id: "1".into(),
                new_start_value: 999,
            }
        );

        let after = p.sequence.tracks.get("1").unwrap().clips.get("1").unwrap();
        assert_eq!(after.timeline_start.value, 999);
        // Rate, source range, name, id all unchanged.
        assert_eq!(after.timeline_start.rate, target_before.timeline_start.rate);
        assert_eq!(after.source_range, target_before.source_range);
        assert_eq!(after.name, target_before.name);
        assert_eq!(after.id, target_before.id);

        // Neighbour on a different track is untouched.
        let neighbour_after = p.sequence.tracks.get("2").unwrap().clips.get("2").unwrap();
        assert_eq!(neighbour_after, &neighbour_before);
    }

    #[test]
    fn move_clip_accepts_negative_start() {
        // Domain allows negative timeline_start (clip ramps in from
        // before the sequence origin). Clamp/snap policy lives in
        // the gesture layer, not here.
        let mut p = sample_project();
        apply(
            &mut p,
            &Command::MoveClip {
                track_id: "1".into(),
                clip_id: "1".into(),
                new_start_value: -50,
            },
        )
        .unwrap();
        assert_eq!(
            p.sequence
                .tracks
                .get("1")
                .unwrap()
                .clips
                .get("1")
                .unwrap()
                .timeline_start
                .value,
            -50
        );
    }

    #[test]
    fn move_clip_rejects_unknown_track() {
        let mut p = sample_project();
        let err = apply(
            &mut p,
            &Command::MoveClip {
                track_id: "no-such".into(),
                clip_id: "1".into(),
                new_start_value: 0,
            },
        )
        .unwrap_err();
        assert!(matches!(err, CommandError::UnknownTrack { ref track_id } if track_id == "no-such"));
    }

    #[test]
    fn move_clip_rejects_unknown_clip() {
        let mut p = sample_project();
        let err = apply(
            &mut p,
            &Command::MoveClip {
                track_id: "1".into(),
                clip_id: "no-such".into(),
                new_start_value: 0,
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CommandError::UnknownClip { ref clip_id, .. } if clip_id == "no-such"
        ));
    }

    #[test]
    fn command_serde_round_trips_through_json() {
        // Locks the wire format the agent will speak.
        let cmd = Command::MoveClip {
            track_id: "trk".into(),
            clip_id: "clp".into(),
            new_start_value: 7,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"move_clip","track_id":"trk","clip_id":"clp","new_start_value":7}"#
        );
        let parsed: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, cmd);
    }
}
