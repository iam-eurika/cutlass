use std::path::{Path, PathBuf};

use cutlass_models::{MediaSource, Rational, RationalTime};

/// Metadata read from a media file without opening a decode pipeline.
///
/// Audio-only sources have `width == 0 && height == 0`, a millisecond
/// `frame_rate` (1000/1), and `video_codec == "none"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaProbe {
    pub width: u32,
    pub height: u32,
    /// Native frame rate of the primary video stream.
    pub frame_rate: Rational,
    /// Source length in ticks at [`frame_rate`](Self::frame_rate).
    pub duration_ticks: i64,
    pub has_audio: bool,
    /// FFmpeg codec name for the selected video stream (e.g. `h264`).
    pub video_codec: String,
}

impl MediaProbe {
    pub fn duration(&self) -> RationalTime {
        RationalTime::new(self.duration_ticks.max(0), self.frame_rate)
    }

    /// Build a [`MediaSource`] for the project media pool.
    pub fn into_media_source(self, path: impl Into<PathBuf>) -> MediaSource {
        MediaSource::new(
            path,
            self.width,
            self.height,
            self.frame_rate,
            self.duration_ticks,
            self.has_audio,
        )
    }

    /// Same as [`into_media_source`](Self::into_media_source) but keeps the path borrow.
    pub fn to_media_source(&self, path: &Path) -> MediaSource {
        MediaSource::new(
            path,
            self.width,
            self.height,
            self.frame_rate,
            self.duration_ticks,
            self.has_audio,
        )
    }
}
