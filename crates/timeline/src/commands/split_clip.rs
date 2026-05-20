use models::{Clip, ClipId, Project, RationalTime, TrackId};

use crate::error::TimelineError;
use crate::util::{
    locate_clip, recompute_sequence_duration, require_speed_one, sort_track, at_timebase, rt,
};

#[derive(Debug, Clone)]
pub struct SplitClip {
    pub clip_id: ClipId,
    pub at: RationalTime,
    pub right_clip_id: ClipId,
}

#[derive(Debug, Clone)]
pub struct SplitClipEffect {
    pub track_id: TrackId,
    pub left_clip_id: ClipId,
    pub right_clip_id: ClipId,
    pub prev_duration: RationalTime,
    pub prev_source_out: RationalTime,
}

pub fn apply(project: &mut Project, cmd: &SplitClip) -> Result<SplitClipEffect, TimelineError> {
    let timebase = project.sequence.timebase;
    let at_num = at_timebase(cmd.at, timebase)?;

    let (ti, ci) =
        locate_clip(&project.sequence, cmd.clip_id).ok_or(TimelineError::ClipNotFound(cmd.clip_id))?;

    if locate_clip(&project.sequence, cmd.right_clip_id).is_some() {
        return Err(TimelineError::DuplicateId {
            kind: "clip",
            id: cmd.right_clip_id.to_string(),
        });
    }

    let track = &project.sequence.tracks[ti];
    let track_id = track.id;
    let original = &track.clips[ci];

    require_speed_one(original, "split with speed != 1/1 is not supported yet")?;

    let start_num = original.start.num;
    let end_num = start_num + original.duration.num;

    // Must split *strictly* inside the clip — splitting at either edge
    // would produce a zero-duration piece.
    if at_num <= start_num || at_num >= end_num {
        return Err(TimelineError::InvalidSplit {
            reason: "at must lie strictly inside [start, start + duration)",
        });
    }

    let left_duration_num = at_num - start_num;
    let right_duration_num = end_num - at_num;
    let prev_duration = original.duration;
    let prev_source_out = original.source_out;
    let prev_source_in_num = original.source_in.num;
    let prev_source_out_num = original.source_out.num;

    // With speed == 1/1, timeline_duration == source_duration.
    let split_source_num = prev_source_in_num + left_duration_num;

    // Build the right piece from the original — same media, same id-less
    // metadata, freshly named for clarity.
    let right_clip = Clip {
        id: cmd.right_clip_id,
        media_id: original.media_id,
        track_id,
        name: original.name.clone(),
        start: rt(at_num, timebase),
        duration: rt(right_duration_num, timebase),
        source_in: rt(split_source_num, timebase),
        source_out: rt(prev_source_out_num, timebase),
        speed: original.speed,
        opacity: original.opacity,
        volume: original.volume,
        enabled: original.enabled,
        color: original.color,
    };

    let track = &mut project.sequence.tracks[ti];
    let left = &mut track.clips[ci];
    left.duration = rt(left_duration_num, timebase);
    left.source_out = rt(split_source_num, timebase);

    track.clips.push(right_clip);
    sort_track(track);
    // Split never moves either edge of the original clip, so duration of
    // the sequence is unchanged — but recompute defensively in case a
    // future variant introduces gaps/extensions.
    recompute_sequence_duration(&mut project.sequence);

    Ok(SplitClipEffect {
        track_id,
        left_clip_id: cmd.clip_id,
        right_clip_id: cmd.right_clip_id,
        prev_duration,
        prev_source_out,
    })
}
