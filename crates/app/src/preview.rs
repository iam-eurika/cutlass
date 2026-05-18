//! Timeline-driven preview: playhead → engine seek → GPU render → RGBA readback.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use decoder::Rational;
use engine::{Engine, EngineEvent, EventReceiver, RequestId, SourceId};
use renderer::{Layer, RenderTarget, Renderer, Transform};
use timeline::{
    AddClip, AddSource, Clip, ClipId, MediaSourceId, Project, SetSourceProbed, TimelineError,
    TrackId,
};

use crate::playhead::{default_video_track, plan_playhead, PlayheadPlan};

const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

/// Longer timeout for 4K exact seeks (decode + GPU readback).
const EVENT_TIMEOUT_HEAVY: Duration = Duration::from_secs(60);

/// Max width/height for preview readback (4K → ~1280px long edge).
pub const PREVIEW_MAX_EDGE_PX: u32 = 1280;

/// How a preview frame is requested from the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewSeek {
    /// Interactive scrub (keyframe snap, coalesced in the worker).
    Scrub,
    /// Presentation-accurate frame for the timeline time.
    Exact,
}

/// Fit `width`×`height` inside a `max_edge`×`max_edge` box, preserving aspect ratio.
pub fn preview_target_dimensions(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let max_edge = max_edge.max(1);
    let w = width.max(1);
    let h = height.max(1);
    if w <= max_edge && h <= max_edge {
        return (w, h);
    }
    let scale = max_edge as f32 / w.max(h) as f32;
    let tw = ((w as f32 * scale).round() as u32).max(1);
    let th = ((h as f32 * scale).round() as u32).max(1);
    (tw, th)
}

/// Failures from the preview pipeline.
#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error("timeline: {0}")]
    Timeline(#[from] TimelineError),

    #[error("engine: {0}")]
    Engine(#[from] engine::EngineError),

    #[error("renderer: {0}")]
    Renderer(#[from] renderer::RendererError),

    #[error("event channel: {0}")]
    EventChannel(#[from] crossbeam_channel::RecvTimeoutError),

    #[error("no video track in project")]
    NoVideoTrack,

    #[error("media source {0} missing from project")]
    UnknownSource(MediaSourceId),

    #[error("engine event: {0}")]
    EngineEvent(&'static str),

    #[error("no GPU adapter available")]
    NoGpu,
}

/// Render result without copying RGBA (pixels live in [`PreviewSession::rgba_buf`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewRender {
    Gap,
    Frame {
        clip_id: ClipId,
        media_time: Rational,
        width: u32,
        height: u32,
    },
}

/// Result of sampling the timeline at one playhead time.
#[derive(Debug)]
pub enum PreviewOutcome {
    /// No clip on the video track at this time.
    Gap,
    /// Decoded, rendered RGBA8 buffer (row-major, 4 bytes per pixel).
    Frame {
        clip_id: ClipId,
        media_time: Rational,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}

/// Owns project state plus engine/renderer handles and `MediaSourceId` → `SourceId` mapping.
pub struct PreviewSession {
    pub project: Project,
    video_track: TrackId,
    engine: Engine,
    events: EventReceiver,
    renderer: Renderer,
    /// Timeline source → engine decoder instance.
    source_map: HashMap<MediaSourceId, SourceId>,
    /// Open requests awaiting `Opened`.
    pending_opens: HashMap<RequestId, MediaSourceId>,
    /// Engine allows one open decoder in MVP; which timeline source owns it.
    active_media: Option<MediaSourceId>,
    /// Reused RGBA render target (recreated when preview dimensions change).
    render_target: Option<RenderTarget>,
    render_target_dims: (u32, u32),
    /// Scratch for GPU readback; valid after [`Self::preview_render`].
    pub(crate) rgba_buf: Vec<u8>,
}

impl PreviewSession {
    /// New empty session with default video track (no clips yet).
    pub fn new() -> Result<Self, PreviewError> {
        let project = Project::new().with_default_video_track();
        let video_track = default_video_track(&project)?;
        Self::from_project(project, video_track)
    }

    pub fn from_project(project: Project, video_track: TrackId) -> Result<Self, PreviewError> {
        let (engine, events) = Engine::new();
        let renderer = Renderer::new().map_err(|_| PreviewError::NoGpu)?;
        Ok(Self {
            project,
            video_track,
            engine,
            events,
            renderer,
            source_map: HashMap::new(),
            pending_opens: HashMap::new(),
            active_media: None,
            render_target: None,
            render_target_dims: (0, 0),
            rgba_buf: Vec::new(),
        })
    }

    /// Build a single-clip project pointing at `media_path` (timeline only; does not open engine).
    pub fn single_clip_project(
        media_path: impl Into<PathBuf>,
    ) -> Result<(Project, TrackId, MediaSourceId, ClipId), TimelineError> {
        let mut project = Project::new().with_default_video_track();
        let track_id = project.tracks[0].id;
        project.apply(Box::new(AddSource::new(media_path)), true)?;
        let source_id = *project.sources.keys().next().unwrap();
        let clip_id = project.alloc_clip_id();
        let clip = Clip {
            id: clip_id,
            source_id,
            source_in: Rational::new_raw(0, 1),
            source_out: Rational::new_raw(60, 1),
            timeline_position: Rational::new_raw(0, 1),
        };
        project.apply(Box::new(AddClip::new(track_id, clip)), true)?;
        Ok((project, track_id, source_id, clip_id))
    }

    /// Convenience: one H.264 clip on the default test fixture.
    pub fn with_h264_fixture() -> Result<Self, PreviewError> {
        let path = h264_fixture_path();
        let (project, track_id, _, _) =
            Self::single_clip_project(path).map_err(PreviewError::Timeline)?;
        Self::from_project(project, track_id)
    }

    pub fn video_track(&self) -> TrackId {
        self.video_track
    }

    pub fn plan(&self, timeline_time: Rational) -> Result<Option<PlayheadPlan>, TimelineError> {
        plan_playhead(&self.project, self.video_track, timeline_time)
    }

    /// Open the media file for `source_id` in the engine (closes any other open source).
    pub fn ensure_source_open(&mut self, source_id: MediaSourceId) -> Result<SourceId, PreviewError> {
        if let Some(&sid) = self.source_map.get(&source_id) {
            if self.active_media == Some(source_id) {
                return Ok(sid);
            }
        }

        if let Some(prev_media) = self.active_media {
            if let Some(&prev_sid) = self.source_map.get(&prev_media) {
                self.engine.close(prev_sid);
                self.drain_events(Duration::from_millis(200))?;
            }
            self.active_media = None;
        }

        let path = self
            .project
            .sources
            .get(&source_id)
            .map(|s| s.original_path.clone())
            .ok_or(PreviewError::UnknownSource(source_id))?;

        if !path.is_file() {
            return Err(PreviewError::EngineEvent("media path is not a file"));
        }

        let (sid, rid) = self.engine.open(path);
        self.pending_opens.insert(rid, source_id);
        self.wait_opened(rid)?;
        self.source_map.insert(source_id, sid);
        self.active_media = Some(source_id);
        Ok(sid)
    }

    /// Sample preview at `timeline_time` (defaults to [`PreviewSeek::Exact`]).
    pub fn preview_at(&mut self, timeline_time: Rational) -> Result<PreviewOutcome, PreviewError> {
        self.preview_at_with_mode(timeline_time, PreviewSeek::Exact)
    }

    /// Seek + render into [`Self::rgba_buf`]; use [`PreviewSeek::Scrub`] while dragging.
    pub fn preview_render(
        &mut self,
        timeline_time: Rational,
        mode: PreviewSeek,
    ) -> Result<PreviewRender, PreviewError> {
        let Some(plan) = self.plan(timeline_time)? else {
            return Ok(PreviewRender::Gap);
        };

        let sid = self.ensure_source_open(plan.media_source)?;
        let timeout = match mode {
            PreviewSeek::Scrub => EVENT_TIMEOUT,
            PreviewSeek::Exact => EVENT_TIMEOUT_HEAVY,
        };
        let frame = match mode {
            PreviewSeek::Exact => {
                let rid = self.engine.seek_exact(sid, plan.media_time);
                self.wait_frame(sid, rid, timeout)?
            }
            PreviewSeek::Scrub => {
                self.engine.seek_scrub(sid, plan.media_time);
                self.wait_scrub_frame(sid, timeout)?
            }
        };

        let (target_w, target_h) =
            preview_target_dimensions(frame.width, frame.height, PREVIEW_MAX_EDGE_PX);
        if self.render_target_dims != (target_w, target_h) {
            self.render_target = Some(RenderTarget::new(self.renderer.device(), target_w, target_h));
            self.render_target_dims = (target_w, target_h);
        }
        let target = self.render_target.as_ref().expect("render target");
        self.renderer.render(
            &[Layer {
                frame,
                transform: Transform::identity(),
                opacity: 1.0,
            }],
            target,
        )?;
        self.renderer
            .read_pixels_rgba8_into(target, &mut self.rgba_buf)?;

        Ok(PreviewRender::Frame {
            clip_id: plan.clip_id,
            media_time: plan.media_time,
            width: target_w,
            height: target_h,
        })
    }

    /// Seek + render; use [`PreviewSeek::Scrub`] while dragging, [`PreviewSeek::Exact`] on release.
    pub fn preview_at_with_mode(
        &mut self,
        timeline_time: Rational,
        mode: PreviewSeek,
    ) -> Result<PreviewOutcome, PreviewError> {
        match self.preview_render(timeline_time, mode)? {
            PreviewRender::Gap => Ok(PreviewOutcome::Gap),
            PreviewRender::Frame {
                clip_id,
                media_time,
                width,
                height,
            } => Ok(PreviewOutcome::Frame {
                clip_id,
                media_time,
                width,
                height,
                rgba: self.rgba_buf.clone(),
            }),
        }
    }

    /// Non-blocking-ish drain of pending engine events (probe metadata, etc.).
    pub fn drain_events(&mut self, max_wait: Duration) -> Result<(), PreviewError> {
        let deadline = std::time::Instant::now() + max_wait;
        while std::time::Instant::now() < deadline {
            match self.events.try_recv() {
                Ok(ev) => self.handle_event(ev)?,
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    return Err(PreviewError::EngineEvent("event channel disconnected"));
                }
            }
        }
        Ok(())
    }

    fn handle_event(&mut self, ev: EngineEvent) -> Result<(), PreviewError> {
        match ev {
            EngineEvent::Opened {
                source_id,
                info,
                request_id,
            } => {
                if let Some(media_id) = self.pending_opens.remove(&request_id) {
                    self.project.apply(
                        Box::new(SetSourceProbed::new(media_id, info)),
                        false,
                    )?;
                    self.source_map.insert(media_id, source_id);
                }
            }
            EngineEvent::Closed { .. } | EngineEvent::Frame { .. } | EngineEvent::Eof { .. } => {}
            EngineEvent::Error { error, .. } => return Err(PreviewError::Engine(error)),
        }
        Ok(())
    }

    fn wait_opened(&mut self, expect_rid: RequestId) -> Result<SourceId, PreviewError> {
        loop {
            match self.events.recv_timeout(EVENT_TIMEOUT)? {
                EngineEvent::Opened {
                    source_id,
                    info,
                    request_id,
                } => {
                    if request_id != expect_rid {
                        self.handle_event(EngineEvent::Opened {
                            source_id,
                            info,
                            request_id,
                        })?;
                        continue;
                    }
                    if let Some(media_id) = self.pending_opens.remove(&request_id) {
                        self.project.apply(
                            Box::new(SetSourceProbed::new(media_id, info)),
                            false,
                        )?;
                        self.source_map.insert(media_id, source_id);
                    }
                    return Ok(source_id);
                }
                ev => self.handle_event(ev)?,
            }
        }
    }

    fn wait_scrub_frame(
        &mut self,
        expect_sid: SourceId,
        timeout: Duration,
    ) -> Result<decoder::DecodedVideoFrame, PreviewError> {
        loop {
            match self.events.recv_timeout(timeout)? {
                EngineEvent::Frame {
                    source_id,
                    frame,
                    request_id: None,
                } if source_id == expect_sid => return Ok(frame),
                EngineEvent::Eof {
                    source_id,
                    request_id: None,
                } if source_id == expect_sid => {
                    return Err(PreviewError::EngineEvent("scrub reached EOF"));
                }
                ev => self.handle_event(ev)?,
            }
        }
    }

    fn wait_frame(
        &mut self,
        expect_sid: SourceId,
        expect_rid: RequestId,
        timeout: Duration,
    ) -> Result<decoder::DecodedVideoFrame, PreviewError> {
        loop {
            match self.events.recv_timeout(timeout)? {
                EngineEvent::Frame {
                    source_id,
                    frame,
                    request_id: Some(rid),
                } if source_id == expect_sid && rid == expect_rid => return Ok(frame),
                EngineEvent::Eof {
                    source_id,
                    request_id: Some(rid),
                } if source_id == expect_sid && rid == expect_rid => {
                    return Err(PreviewError::EngineEvent("seek reached EOF"));
                }
                ev => self.handle_event(ev)?,
            }
        }
    }
}

/// Path to `testsrc_h264.mp4` from the decoder test assets.
pub fn h264_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../decoder/tests/assets/testsrc_h264.mp4")
}

/// Mean absolute deviation of RGBA bytes from their mean (test helper).
pub fn pixel_variance_rgba(buf: &[u8]) -> f64 {
    if buf.is_empty() {
        return 0.0;
    }
    let n = buf.len();
    let mean: f64 = buf.iter().map(|&b| f64::from(b)).sum::<f64>() / n as f64;
    buf.iter()
        .map(|&b| (f64::from(b) - mean).abs())
        .sum::<f64>()
        / n as f64
}

/// Stable fingerprint for comparing two RGBA buffers in tests.
pub fn rgba_fingerprint(buf: &[u8]) -> u64 {
    buf.iter().fold(0u64, |acc, &b| acc.wrapping_add(u64::from(b)))
}

/// Returns true if `path` exists (skip GPU tests in environments without fixtures).
pub fn fixture_available(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_target_dimensions_caps_4k() {
        let (w, h) = preview_target_dimensions(3840, 2160, PREVIEW_MAX_EDGE_PX);
        assert_eq!(w.max(h), PREVIEW_MAX_EDGE_PX);
        assert!(w <= 3840 && h <= 2160);
    }

    #[test]
    fn preview_target_dimensions_unchanged_when_small() {
        assert_eq!(preview_target_dimensions(640, 360, 1280), (640, 360));
    }
}
