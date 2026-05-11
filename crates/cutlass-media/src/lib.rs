//! Cutlass media: probe + decode pipeline backed by ffmpeg.
//!
//! The public surface is small on purpose: callers open a file with
//! [`MediaSource::open`], inspect [`MediaInfo`], and ask for frames at
//! specific times via [`MediaSource::decode_frame_at`]. The frame cache,
//! priority queue, and worker threads from the project plan plug in
//! around this surface in later phases.

use std::path::{Path, PathBuf};

use ffmpeg_next as ff;
use num_rational::Rational64;
use thiserror::Error;

pub mod decode;
pub mod probe;
pub mod time;

pub use decode::{DecodedFrame, HwAccel};

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("ffmpeg: {0}")]
    Ffmpeg(#[from] ff::Error),

    #[error("no decodable video or audio stream in {0}")]
    NoStreams(PathBuf),

    #[error("file has no video stream: {0}")]
    NoVideoStream(PathBuf),

    #[error("no frame produced for target {target_seconds}s in {path}")]
    NoFrame {
        path: PathBuf,
        target_seconds: f64,
    },

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, MediaError>;

/// Everything we learned about a media file at probe time. Times are
/// rationals in seconds; convert to `f64` only for display.
#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub path: PathBuf,
    pub format_name: String,
    pub duration: Rational64,
    pub video: Option<VideoStreamInfo>,
    pub audio: Option<AudioStreamInfo>,
}

#[derive(Debug, Clone)]
pub struct VideoStreamInfo {
    pub stream_index: usize,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub pix_fmt: String,
    /// Reported average frame rate. `None` when the source is VFR.
    pub frame_rate: Option<Rational64>,
    pub time_base: Rational64,
    /// Display rotation in degrees, 0/90/180/270.
    pub rotation: i32,
}

#[derive(Debug, Clone)]
pub struct AudioStreamInfo {
    pub stream_index: usize,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_fmt: String,
    pub time_base: Rational64,
}

/// An opened media source: format context + (optional) warm video decoder.
///
/// Holding the decoder across calls is what makes scrubbing tractable —
/// each `decode_frame_at` reuses internal codec state instead of paying
/// the full open cost per frame.
pub struct MediaSource {
    info: MediaInfo,
    ictx: ff::format::context::Input,
    video: Option<decode::VideoDecodeState>,
}

impl MediaSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        init()?;
        let path = path.as_ref();
        let ictx = ff::format::input(&path)?;
        let info = probe::probe(&ictx, path)?;
        let video = decode::VideoDecodeState::new(&ictx)?;
        Ok(Self { info, ictx, video })
    }

    pub fn info(&self) -> &MediaInfo {
        &self.info
    }

    /// Hardware acceleration backend the video decoder is using, or
    /// [`HwAccel::None`] for files with no video stream or when hwaccel
    /// setup fell back to software (e.g. unsupported codec).
    pub fn hw_accel(&self) -> HwAccel {
        self.video
            .as_ref()
            .map(|v| v.hw_accel())
            .unwrap_or(HwAccel::None)
    }

    /// Decode the video frame whose presentation interval contains
    /// `target_seconds`. Seeks backwards to a keyframe and decodes
    /// forward — see [`decode`] for the algorithm.
    pub fn decode_frame_at(&mut self, target_seconds: Rational64) -> Result<DecodedFrame> {
        let video = self
            .video
            .as_mut()
            .ok_or_else(|| MediaError::NoVideoStream(self.info.path.clone()))?;
        match decode::decode_frame_at(&mut self.ictx, video, target_seconds) {
            Ok(frame) => Ok(frame),
            Err(MediaError::NoFrame { .. }) => Err(MediaError::NoFrame {
                path: self.info.path.clone(),
                target_seconds: rational_to_f64(target_seconds),
            }),
            Err(e) => Err(e),
        }
    }
}

impl std::fmt::Debug for MediaSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaSource")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

/// One-time global ffmpeg init. Safe to call repeatedly.
pub fn init() -> Result<()> {
    ff::init()?;
    Ok(())
}

fn rational_to_f64(r: Rational64) -> f64 {
    *r.numer() as f64 / *r.denom() as f64
}
