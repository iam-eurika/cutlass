use models::{ClipId, Project, RationalTime, TrackId};

use crate::error::TimelineError;
use crate::util::{
    assert_no_overlap, locate_clip, recompute_sequence_duration, require_speed_one, at_timebase,
    rt,
};

#[derive(Debug, Clone)]
pub struct TrimClipOut {
    pub clip_id: ClipId,
    pub new_source_out: RationalTime,
}

#[derive(Debug, Clone)]
pub struct TrimClipOutEffect {
    pub track_id: TrackId,
    pub clip_id: ClipId,
    pub prev_duration: RationalTime,
    pub prev_source_out: RationalTime,
}

pub fn apply(project: &mut Project, cmd: &TrimClipOut) -> Result<TrimClipOutEffect, TimelineError> {
    let timebase = project.sequence.timebase;
    let new_out_num = at_timebase(cmd.new_source_out, timebase)?;

    let (ti, ci) =
        locate_clip(&project.sequence, cmd.clip_id).ok_or(TimelineError::ClipNotFound(cmd.clip_id))?;

    let track = &project.sequence.tracks[ti];
    let track_id = track.id;
    let clip = &track.clips[ci];

    require_speed_one(clip, "trim with speed != 1/1 is not supported yet")?;

    if new_out_num <= clip.source_in.num {
        return Err(TimelineError::InvalidTrim {
            reason: "new_source_out must be > source_in",
        });
    }

    let delta = new_out_num - clip.source_out.num;
    let new_duration_num = clip.duration.num + delta;

    if new_duration_num <= 0 {
        return Err(TimelineError::InvalidTrim {
            reason: "trim would collapse clip duration to ≤ 0",
        });
    }

    assert_no_overlap(track, timebase, clip.start.num, new_duration_num, Some(ci))?;

    let prev_duration = clip.duration;
    let prev_source_out = clip.source_out;

    let track = &mut project.sequence.tracks[ti];
    let clip = &mut track.clips[ci];
    clip.duration = rt(new_duration_num, timebase);
    clip.source_out = rt(new_out_num, timebase);
    recompute_sequence_duration(&mut project.sequence);

    Ok(TrimClipOutEffect {
        track_id,
        clip_id: cmd.clip_id,
        prev_duration,
        prev_source_out,
    })
}
