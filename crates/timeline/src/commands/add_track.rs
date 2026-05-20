use models::{Project, Track, TrackId, TrackKind};

use crate::error::TimelineError;
use crate::util::default_track_height;

#[derive(Debug, Clone)]
pub struct AddTrack {
    pub track_id: TrackId,
    pub kind: TrackKind,
    pub name: String,
    /// Optional override; falls back to a sensible default per kind
    /// when `None`.
    pub height_px: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct AddTrackEffect {
    pub track_id: TrackId,
}

pub fn apply(project: &mut Project, cmd: &AddTrack) -> Result<AddTrackEffect, TimelineError> {
    if project
        .sequence
        .tracks
        .iter()
        .any(|t| t.id == cmd.track_id)
    {
        return Err(TimelineError::DuplicateId {
            kind: "track",
            id: cmd.track_id.to_string(),
        });
    }

    let height_px = cmd.height_px.unwrap_or_else(|| default_track_height(cmd.kind));

    project.sequence.tracks.push(Track {
        id: cmd.track_id,
        name: cmd.name.clone(),
        kind: cmd.kind,
        height_px,
        muted: false,
        solo: false,
        locked: false,
        visible: matches!(cmd.kind, TrackKind::Video),
        clips: Vec::new(),
    });

    Ok(AddTrackEffect {
        track_id: cmd.track_id,
    })
}
