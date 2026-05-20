use models::{ClipId, Project, TrackId};

use crate::error::TimelineError;
use crate::util::{
    assert_no_overlap, locate_clip, recompute_sequence_duration, sort_track, at_timebase,
    track_index,
};

#[derive(Debug, Clone)]
pub struct AddClip {
    pub track_id: TrackId,
    pub clip: models::Clip,
}

#[derive(Debug, Clone)]
pub struct AddClipEffect {
    pub track_id: TrackId,
    pub clip_id: ClipId,
}

pub fn apply(project: &mut Project, cmd: &AddClip) -> Result<AddClipEffect, TimelineError> {
    let timebase = project.sequence.timebase;

    // --- Validate up front (no writes yet). -------------------------------
    if cmd.clip.track_id != cmd.track_id {
        return Err(TimelineError::ClipTrackMismatch {
            clip_track: cmd.clip.track_id,
            command_track: cmd.track_id,
        });
    }

    let start_num = at_timebase(cmd.clip.start, timebase)?;
    let duration_num = at_timebase(cmd.clip.duration, timebase)?;
    let source_in_num = at_timebase(cmd.clip.source_in, timebase)?;
    let source_out_num = at_timebase(cmd.clip.source_out, timebase)?;

    if start_num < 0 {
        return Err(TimelineError::InvalidTime {
            reason: "clip.start must be ≥ 0",
        });
    }
    if duration_num <= 0 {
        return Err(TimelineError::InvalidTime {
            reason: "clip.duration must be > 0",
        });
    }
    if source_in_num < 0 {
        return Err(TimelineError::InvalidTime {
            reason: "clip.source_in must be ≥ 0",
        });
    }
    if source_out_num <= source_in_num {
        return Err(TimelineError::InvalidTime {
            reason: "clip.source_out must be > clip.source_in",
        });
    }

    // Clip-id uniqueness across the *whole* sequence — we treat ClipId
    // as project-scoped, not track-scoped, so duplicates anywhere are an
    // agent bug worth surfacing.
    if locate_clip(&project.sequence, cmd.clip.id).is_some() {
        return Err(TimelineError::DuplicateId {
            kind: "clip",
            id: cmd.clip.id.to_string(),
        });
    }

    // Media id (if present) must exist in the bin. None is fine — that's
    // the placeholder for generators / titles / colour mattes.
    if let Some(media_id) = cmd.clip.media_id
        && !project.media_bin.iter().any(|m| m.id == media_id)
    {
        return Err(TimelineError::SourceNotFound(media_id));
    }

    let ti = track_index(&project.sequence, cmd.track_id)?;
    assert_no_overlap(
        &project.sequence.tracks[ti],
        timebase,
        start_num,
        duration_num,
        None,
    )?;

    // --- Commit. ----------------------------------------------------------
    let track = &mut project.sequence.tracks[ti];
    track.clips.push(cmd.clip.clone());
    sort_track(track);
    recompute_sequence_duration(&mut project.sequence);

    Ok(AddClipEffect {
        track_id: cmd.track_id,
        clip_id: cmd.clip.id,
    })
}
