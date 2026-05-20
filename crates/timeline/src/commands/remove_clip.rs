use models::{Clip, ClipId, Project, TrackId};

use crate::error::TimelineError;
use crate::util::{locate_clip, recompute_sequence_duration};

#[derive(Debug, Clone)]
pub struct RemoveClip {
    pub clip_id: ClipId,
}

/// Whole clip is stashed so undo can re-insert it intact. `Box` keeps
/// the effect enum's stack footprint small — `Clip` is the largest variant
/// by far.
#[derive(Debug, Clone)]
pub struct RemoveClipEffect {
    pub track_id: TrackId,
    pub clip: Box<Clip>,
}

pub fn apply(project: &mut Project, cmd: &RemoveClip) -> Result<RemoveClipEffect, TimelineError> {
    let (ti, ci) =
        locate_clip(&project.sequence, cmd.clip_id).ok_or(TimelineError::ClipNotFound(cmd.clip_id))?;

    let track = &mut project.sequence.tracks[ti];
    let track_id = track.id;
    let removed = track.clips.remove(ci);
    recompute_sequence_duration(&mut project.sequence);

    Ok(RemoveClipEffect {
        track_id,
        clip: Box::new(removed),
    })
}
