//! Command dispatch: undoable timeline edits and session project commands.

mod dispatch;
mod edit;
mod project;

pub use dispatch::{ApplyOutcome, dispatch};

use cutlass_cache::FrameCache;
use cutlass_models::Project;

use crate::error::EngineError;

/// Session surface passed to every command handler.
pub struct ApplyContext<'a> {
    pub project: &'a mut Project,
    pub cache: &'a FrameCache,
    pub project_path: &'a mut Option<std::path::PathBuf>,
    pub history: &'a mut History,
}

/// A runtime edit action. Consuming `apply` runs the edit and returns its inverse.
pub trait EditAction: Send {
    fn apply(self: Box<Self>, ctx: &mut ApplyContext<'_>) -> Result<Box<dyn EditAction>, EngineError>;
}

/// Inverse-command undo/redo stacks.
#[derive(Default)]
pub struct History {
    undo: Vec<Box<dyn EditAction>>,
    redo: Vec<Box<dyn EditAction>>,
    limit: usize,
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn record_do(&mut self, inverse: Box<dyn EditAction>) {
        if self.undo.len() >= self.limit {
            self.undo.remove(0);
        }
        self.undo.push(inverse);
        self.redo.clear();
    }

    pub fn pop_undo(&mut self) -> Option<Box<dyn EditAction>> {
        self.undo.pop()
    }

    pub fn push_redo(&mut self, inverse: Box<dyn EditAction>) {
        if self.redo.len() >= self.limit {
            self.redo.remove(0);
        }
        self.redo.push(inverse);
    }

    pub fn pop_redo(&mut self) -> Option<Box<dyn EditAction>> {
        self.redo.pop()
    }

    pub fn push_undo(&mut self, inverse: Box<dyn EditAction>) {
        if self.undo.len() >= self.limit {
            self.undo.remove(0);
        }
        self.undo.push(inverse);
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::edit::{
        add_clip, add_generated, move_clip, remove_media, ripple_delete, split_clip, trim_clip,
    };
    use cutlass_cache::FrameCache;
    use cutlass_models::{
        Clip, Generator, MediaSource, Rational, RationalTime, TimeRange, TrackKind,
    };

    fn setup() -> (tempfile::TempDir, Project, FrameCache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = FrameCache::new(dir.path().join("cache"), 1024 * 1024).unwrap();
        let project = Project::new("test", Rational::FPS_24);
        (dir, project, cache)
    }

    fn test_ctx<'a>(
        project: &'a mut Project,
        cache: &'a FrameCache,
        project_path: &'a mut Option<std::path::PathBuf>,
        history: &'a mut History,
    ) -> ApplyContext<'a> {
        ApplyContext {
            project,
            cache,
            project_path,
            history,
        }
    }

    #[test]
    fn add_clip_inverse_oscillates() {
        let (_dir, mut project, cache) = setup();
        let media_id = project.add_media(MediaSource::new(
            "/tmp/x.mp4",
            1920,
            1080,
            Rational::FPS_24,
            240,
            true,
        ));
        let track = project.add_track(TrackKind::Video, "V1");

        let mut project_path = None;
        let mut history = History::new(32);
        let mut ctx = test_ctx(&mut project, &cache, &mut project_path, &mut history);

        let (id, inv1) = add_clip::execute(
            &mut ctx,
            track,
            media_id,
            TimeRange::at_rate(0, 48, Rational::FPS_24),
            RationalTime::new(0, Rational::FPS_24),
        )
        .unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 1);

        let inv2 = inv1.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 0);

        let inv3 = inv2.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 1);
        assert!(ctx.project.clip(id).is_some());

        let _ = inv3.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 0);
    }

    #[test]
    fn remove_media_inverse_restores_snapshot() {
        let (_dir, mut project, cache) = setup();
        let media = MediaSource::new("/tmp/y.mp4", 1280, 720, Rational::FPS_24, 100, false);
        let id = project.add_media(media.clone());

        let mut project_path = None;
        let mut history = History::new(32);
        let mut ctx = test_ctx(&mut project, &cache, &mut project_path, &mut history);

        let inv1 = Box::new(remove_media::RemoveMediaAction { media: id });
        let inv2 = inv1.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.media_count(), 0);

        let inv3 = inv2.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.media_count(), 1);
        assert_eq!(ctx.project.media(id).unwrap(), &media);

        let _ = inv3.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.media_count(), 0);
    }

    #[test]
    fn split_clip_inverse_oscillates() {
        let (_dir, mut project, cache) = setup();
        let media_id = project.add_media(MediaSource::new(
            "/tmp/split.mp4",
            1920,
            1080,
            Rational::FPS_24,
            240,
            false,
        ));
        let track = project.add_track(TrackKind::Video, "V1");
        let clip_id = project
            .add_clip(
                track,
                media_id,
                TimeRange::at_rate(0, 48, Rational::FPS_24),
                RationalTime::new(0, Rational::FPS_24),
            )
            .unwrap();

        let mut project_path = None;
        let mut history = History::new(32);
        let mut ctx = test_ctx(&mut project, &cache, &mut project_path, &mut history);

        let (_tail, inv1) = split_clip::execute(&mut ctx, clip_id, rt(24)).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 2);
        assert_eq!(
            ctx.project.clip(clip_id).unwrap().timeline.duration.value,
            24
        );

        let inv2 = inv1.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 1);
        assert_eq!(
            ctx.project.clip(clip_id).unwrap().timeline.duration.value,
            48
        );

        let inv3 = inv2.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 2);
        assert_eq!(
            ctx.project.clip(clip_id).unwrap().timeline.duration.value,
            24
        );

        let _ = inv3.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 1);
    }

    #[test]
    fn ripple_delete_inverse_oscillates() {
        let (_dir, mut project, cache) = setup();
        let track = project.add_track(TrackKind::Adjustment, "FX");
        let first = project
            .timeline_mut()
            .add_clip(
                track,
                Clip::generated(Generator::Adjustment, tr(0, 10)),
            )
            .unwrap();
        let second = project
            .timeline_mut()
            .add_clip(
                track,
                Clip::generated(Generator::Adjustment, tr(20, 10)),
            )
            .unwrap();

        let mut project_path = None;
        let mut history = History::new(32);
        let mut ctx = test_ctx(&mut project, &cache, &mut project_path, &mut history);

        let inv1 = ripple_delete::execute(&mut ctx, first).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 1);
        assert_eq!(
            ctx.project.clip(second).unwrap().timeline.start.value,
            10
        );

        let inv2 = inv1.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 2);
        assert_eq!(
            ctx.project.clip(second).unwrap().timeline.start.value,
            20
        );

        let _ = inv2.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 1);
    }

    #[test]
    fn ripple_delete_middle_of_three_adjacent_oscillates() {
        let (_dir, mut project, cache) = setup();
        let track = project.add_track(TrackKind::Adjustment, "FX");
        let a = project
            .timeline_mut()
            .add_clip(track, Clip::generated(Generator::Adjustment, tr(0, 10)))
            .unwrap();
        let b = project
            .timeline_mut()
            .add_clip(track, Clip::generated(Generator::Adjustment, tr(10, 10)))
            .unwrap();
        let c = project
            .timeline_mut()
            .add_clip(track, Clip::generated(Generator::Adjustment, tr(20, 10)))
            .unwrap();

        let mut project_path = None;
        let mut history = History::new(32);
        let mut ctx = test_ctx(&mut project, &cache, &mut project_path, &mut history);

        let inv1 = ripple_delete::execute(&mut ctx, b).unwrap();
        assert_eq!(ctx.project.clip(c).unwrap().start().value, 10);

        let inv2 = inv1.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.clip(b).unwrap().start().value, 10);
        assert_eq!(ctx.project.clip(c).unwrap().start().value, 20);

        let _ = inv2.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.clip(c).unwrap().start().value, 10);
        assert!(ctx.project.clip(b).is_none());
        let _ = a;
    }

    #[test]
    fn trim_clip_inverse_restores_snapshot() {
        let (_dir, mut project, cache) = setup();
        let media_id = project.add_media(MediaSource::new(
            "/tmp/trim.mp4",
            1280,
            720,
            Rational::FPS_24,
            240,
            false,
        ));
        let track = project.add_track(TrackKind::Video, "V1");
        let clip_id = project
            .add_clip(
                track,
                media_id,
                TimeRange::at_rate(0, 48, Rational::FPS_24),
                RationalTime::new(0, Rational::FPS_24),
            )
            .unwrap();
        let before = ctx_project_clip(&project, clip_id);

        let mut project_path = None;
        let mut history = History::new(32);
        let mut ctx = test_ctx(&mut project, &cache, &mut project_path, &mut history);

        let inv1 = trim_clip::execute(&mut ctx, clip_id, tr(10, 28)).unwrap();
        assert_eq!(ctx.project.clip(clip_id).unwrap().timeline.start.value, 10);

        let inv2 = inv1.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.clip(clip_id).unwrap().timeline, before.timeline);
        assert_eq!(
            ctx.project.clip(clip_id).unwrap().source_range(),
            before.source_range()
        );

        let _ = inv2.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.clip(clip_id).unwrap().timeline.start.value, 10);
    }

    #[test]
    fn move_clip_inverse_oscillates() {
        let (_dir, mut project, cache) = setup();
        let v1 = project.add_track(TrackKind::Text, "T1");
        let v2 = project.add_track(TrackKind::Text, "T2");
        let clip_id = project
            .timeline_mut()
            .add_clip(
                v1,
                Clip::generated(Generator::Text { content: "x".into() }, tr(5, 15)),
            )
            .unwrap();

        let mut project_path = None;
        let mut history = History::new(32);
        let mut ctx = test_ctx(&mut project, &cache, &mut project_path, &mut history);

        let inv1 = move_clip::execute(&mut ctx, clip_id, v2, rt(40)).unwrap();
        assert_eq!(ctx.project.timeline().track_of(clip_id), Some(v2));

        let inv2 = inv1.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.timeline().track_of(clip_id), Some(v1));
        assert_eq!(ctx.project.clip(clip_id).unwrap().start().value, 5);

        let _ = inv2.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.timeline().track_of(clip_id), Some(v2));
    }

    #[test]
    fn add_generated_inverse_oscillates() {
        let (_dir, mut project, cache) = setup();
        let track = project.add_track(TrackKind::Sticker, "S1");

        let mut project_path = None;
        let mut history = History::new(32);
        let mut ctx = test_ctx(&mut project, &cache, &mut project_path, &mut history);

        let (id, inv1) = add_generated::execute(
            &mut ctx,
            track,
            Generator::SolidColor {
                rgba: [9, 8, 7, 6],
            },
            tr(0, 12),
        )
        .unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 1);

        let inv2 = inv1.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 0);

        let inv3 = inv2.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 1);
        assert!(ctx.project.clip(id).is_some());

        let _ = inv3.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 0);
    }

    #[test]
    fn split_generated_clip_inverse_oscillates() {
        let (_dir, mut project, cache) = setup();
        let track = project.add_track(TrackKind::Text, "T1");
        let clip_id = project
            .timeline_mut()
            .add_clip(
                track,
                Clip::generated(
                    Generator::Text {
                        content: "split me".into(),
                    },
                    tr(0, 20),
                ),
            )
            .unwrap();

        let mut project_path = None;
        let mut history = History::new(32);
        let mut ctx = test_ctx(&mut project, &cache, &mut project_path, &mut history);

        let (_tail, inv1) = split_clip::execute(&mut ctx, clip_id, rt(10)).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 2);

        let inv2 = inv1.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 1);
        assert_eq!(
            ctx.project.clip(clip_id).unwrap().timeline.duration.value,
            20
        );

        let _ = inv2.apply(&mut ctx).unwrap();
        assert_eq!(ctx.project.timeline().clip_count(), 2);
    }

    fn ctx_project_clip(project: &Project, id: cutlass_models::ClipId) -> cutlass_models::Clip {
        project.clip(id).unwrap().clone()
    }

    fn rt(value: i64) -> RationalTime {
        RationalTime::new(value, Rational::FPS_24)
    }

    fn tr(start: i64, duration: i64) -> TimeRange {
        TimeRange::at_rate(start, duration, Rational::FPS_24)
    }
}
