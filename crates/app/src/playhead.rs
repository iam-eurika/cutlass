//! Pure timeline → engine seek plan (no I/O, no GPU).

use decoder::Rational;
use timeline::{ActiveClip, ClipId, MediaSourceId, Project, TimelineError, TrackId, TrackKind};

/// What the preview pipeline should request from the engine at a timeline time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayheadPlan {
    pub clip_id: ClipId,
    pub media_source: MediaSourceId,
    pub media_time: Rational,
}

/// First video track in the project, if any.
pub fn default_video_track(project: &Project) -> Result<TrackId, TimelineError> {
    project
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Video)
        .map(|t| t.id)
        .ok_or(TimelineError::TrackNotFound(TrackId(0)))
}

/// Latest exclusive end time on a track (for playhead slider max), if any clips exist.
pub fn max_timeline_end(project: &Project, video_track: TrackId) -> Option<Rational> {
    let track = project.track(video_track).ok()?;
    let mut max: Option<Rational> = None;
    for clip in &track.clips {
        let Some(end) = clip.timeline_end() else {
            continue;
        };
        max = Some(match max {
            None => end,
            Some(m) if end.ge(m) => end,
            Some(m) => m,
        });
    }
    max
}

/// Convert UI seconds (float) to timeline [`Rational`] with millisecond resolution.
pub fn seconds_to_rational(seconds: f32) -> Rational {
    let ms = (seconds * 1000.0).round().clamp(0.0, i64::MAX as f32) as i64;
    Rational::new_raw(ms, 1000)
}

/// Map timeline playhead → active clip → `(source, media_time)` for the engine.
pub fn plan_playhead(
    project: &Project,
    video_track: TrackId,
    timeline_time: Rational,
) -> Result<Option<PlayheadPlan>, TimelineError> {
    let Some(ActiveClip {
        clip_id,
        source_id,
        media_time,
    }) = project.active_clip_on_track(video_track, timeline_time)?
    else {
        return Ok(None);
    };
    Ok(Some(PlayheadPlan {
        clip_id,
        media_source: source_id,
        media_time,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    use timeline::{AddClip, AddSource, Clip, Project};

    fn sample_project() -> (Project, TrackId, MediaSourceId) {
        let mut p = Project::new().with_default_video_track();
        let track_id = p.tracks[0].id;
        p.apply(Box::new(AddSource::new("/media/a.mp4")), true).unwrap();
        let source_id = *p.sources.keys().next().unwrap();
        let clip = Clip {
            id: p.alloc_clip_id(),
            source_id,
            source_in: Rational::new_raw(0, 1),
            source_out: Rational::new_raw(10, 1),
            timeline_position: Rational::new_raw(0, 1),
        };
        p.apply(Box::new(AddClip::new(track_id, clip)), true).unwrap();
        (p, track_id, source_id)
    }

    #[test]
    fn plan_inside_clip() {
        let (p, track, sid) = sample_project();
        let plan = plan_playhead(&p, track, Rational::new_raw(3, 1))
            .unwrap()
            .expect("plan");
        assert_eq!(plan.media_source, sid);
        assert_eq!(plan.media_time.reduced(), Rational::new_raw(3, 1));
    }

    #[test]
    fn plan_gap_returns_none() {
        let (p, track, _) = sample_project();
        assert!(
            plan_playhead(&p, track, Rational::new_raw(99, 1))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn default_video_track_finds_track() {
        let (p, track, _) = sample_project();
        assert_eq!(default_video_track(&p).unwrap(), track);
    }

    #[test]
    fn default_video_track_missing_errors() {
        let p = Project::new();
        assert!(default_video_track(&p).is_err());
    }

    #[test]
    fn max_timeline_end_from_clips() {
        let (p, track, _) = sample_project();
        let end = max_timeline_end(&p, track).expect("end");
        assert_eq!(end.reduced(), Rational::new_raw(10, 1));
    }

    #[test]
    fn seconds_to_rational_millis() {
        let r = seconds_to_rational(2.5);
        assert_eq!(r, Rational::new_raw(2500, 1000));
    }
}
