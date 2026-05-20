use models::{ClipId, Project, RationalTime, TrackId};

use crate::error::TimelineError;
use crate::util::{
    assert_no_overlap, locate_clip, recompute_sequence_duration, sort_track, at_timebase, rt,
};

#[derive(Debug, Clone)]
pub struct MoveClip {
    pub clip_id: ClipId,
    pub new_start: RationalTime,
}

#[derive(Debug, Clone)]
pub struct MoveClipEffect {
    pub track_id: TrackId,
    pub clip_id: ClipId,
    pub prev_start: RationalTime,
}

pub fn apply(project: &mut Project, cmd: &MoveClip) -> Result<MoveClipEffect, TimelineError> {
    let timebase = project.sequence.timebase;
    let new_start_num = at_timebase(cmd.new_start, timebase)?;
    if new_start_num < 0 {
        return Err(TimelineError::InvalidTime {
            reason: "new_start must be ≥ 0",
        });
    }

    let (ti, ci) =
        locate_clip(&project.sequence, cmd.clip_id).ok_or(TimelineError::ClipNotFound(cmd.clip_id))?;

    let track = &project.sequence.tracks[ti];
    let track_id = track.id;
    let clip = &track.clips[ci];
    let prev_start = clip.start;
    let duration_num = clip.duration.num;

    if new_start_num == prev_start.num {
        // No-op — still emit a (trivial) effect so callers don't have to
        // special-case "command succeeded but nothing changed".
        return Ok(MoveClipEffect {
            track_id,
            clip_id: cmd.clip_id,
            prev_start,
        });
    }

    assert_no_overlap(track, timebase, new_start_num, duration_num, Some(ci))?;

    let track = &mut project.sequence.tracks[ti];
    track.clips[ci].start = rt(new_start_num, timebase);
    sort_track(track);
    recompute_sequence_duration(&mut project.sequence);

    Ok(MoveClipEffect {
        track_id,
        clip_id: cmd.clip_id,
        prev_start,
    })
}
