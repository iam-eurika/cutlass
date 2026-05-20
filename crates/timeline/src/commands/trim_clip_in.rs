use models::{ClipId, Project, RationalTime, TrackId};

use crate::error::TimelineError;
use crate::util::{
    assert_no_overlap, locate_clip, recompute_sequence_duration, require_speed_one, sort_track,
    at_timebase, rt,
};

#[derive(Debug, Clone)]
pub struct TrimClipIn {
    pub clip_id: ClipId,
    pub new_source_in: RationalTime,
}

#[derive(Debug, Clone)]
pub struct TrimClipInEffect {
    pub track_id: TrackId,
    pub clip_id: ClipId,
    pub prev_start: RationalTime,
    pub prev_duration: RationalTime,
    pub prev_source_in: RationalTime,
}

pub fn apply(project: &mut Project, cmd: &TrimClipIn) -> Result<TrimClipInEffect, TimelineError> {
    let timebase = project.sequence.timebase;
    let new_in_num = at_timebase(cmd.new_source_in, timebase)?;

    let (ti, ci) =
        locate_clip(&project.sequence, cmd.clip_id).ok_or(TimelineError::ClipNotFound(cmd.clip_id))?;

    let track = &project.sequence.tracks[ti];
    let track_id = track.id;
    let clip = &track.clips[ci];

    require_speed_one(clip, "trim with speed != 1/1 is not supported yet")?;

    if new_in_num < 0 {
        return Err(TimelineError::InvalidTrim {
            reason: "new_source_in must be ≥ 0",
        });
    }
    if new_in_num >= clip.source_out.num {
        return Err(TimelineError::InvalidTrim {
            reason: "new_source_in must be < source_out",
        });
    }

    let delta = new_in_num - clip.source_in.num;
    let new_start_num = clip.start.num + delta;
    let new_duration_num = clip.duration.num - delta;

    if new_start_num < 0 {
        return Err(TimelineError::InvalidTrim {
            reason: "trim would push clip.start below 0",
        });
    }
    if new_duration_num <= 0 {
        return Err(TimelineError::InvalidTrim {
            reason: "trim would collapse clip duration to ≤ 0",
        });
    }

    // Trim-in can extend the clip leftward (when `delta < 0`), so check
    // overlap. When `delta > 0` the start moves right and can't overlap
    // anyone it wasn't already not overlapping, but `assert_no_overlap`
    // is cheap enough that we always run it.
    assert_no_overlap(track, timebase, new_start_num, new_duration_num, Some(ci))?;

    let prev_start = clip.start;
    let prev_duration = clip.duration;
    let prev_source_in = clip.source_in;

    let track = &mut project.sequence.tracks[ti];
    let clip = &mut track.clips[ci];
    clip.start = rt(new_start_num, timebase);
    clip.duration = rt(new_duration_num, timebase);
    clip.source_in = rt(new_in_num, timebase);
    // sort_track is a no-op here since trim-in moves start in a way that
    // can't cross a neighbour (overlap check above forbids it), but
    // calling it keeps the invariant explicit for future readers.
    sort_track(track);
    recompute_sequence_duration(&mut project.sequence);

    Ok(TrimClipInEffect {
        track_id,
        clip_id: cmd.clip_id,
        prev_start,
        prev_duration,
        prev_source_in,
    })
}
