//! Demux-only probing via FFmpeg (no decoder, no hwaccel).

use std::path::Path;
use std::sync::OnceLock;

use ffmpeg_next::codec::context::Context;
use ffmpeg_next::format;
use ffmpeg_next::format::stream::Disposition;
use ffmpeg_next::media::Type;
use ffmpeg_next::{Error as FfmpegError, Rational as FfRational};

use cutlass_models::Rational;

use crate::error::ProbeError;
use crate::media::MediaProbe;

static FFMPEG_INIT: OnceLock<Result<(), FfmpegError>> = OnceLock::new();

pub(crate) fn ensure_ffmpeg_init() -> Result<(), ProbeError> {
    match FFMPEG_INIT.get_or_init(ffmpeg_next::init) {
        Ok(()) => Ok(()),
        Err(e) => Err(ProbeError::Open(*e)),
    }
}

/// Tick rate for audio-only sources, which have no frame rate of their own.
/// Millisecond ticks keep durations sample-accurate enough for timeline math.
const AUDIO_TICK_RATE: Rational = Rational::new(1000, 1);

/// Inspect `path` for container and stream metadata (ffprobe-style).
///
/// Audio-only files (no real video stream — embedded cover art doesn't count)
/// probe with `width == 0 && height == 0` and a millisecond tick rate.
pub fn probe(path: &Path) -> Result<MediaProbe, ProbeError> {
    ensure_ffmpeg_init()?;

    let path_str = path.to_str().ok_or(ProbeError::InvalidPath)?;
    let input = format::input(path_str).map_err(ProbeError::Open)?;

    let has_audio = input.streams().any(|s| s.parameters().medium() == Type::Audio);

    // mp3/m4a cover art is muxed as a video stream flagged ATTACHED_PIC;
    // treat those files as audio, not as one-frame videos.
    let video_stream = input
        .streams()
        .best(Type::Video)
        .filter(|s| !s.disposition().contains(Disposition::ATTACHED_PIC));

    let Some(stream) = video_stream else {
        if !has_audio {
            return Err(ProbeError::unsupported("no video or audio stream found"));
        }
        let micros = input.duration();
        return Ok(MediaProbe {
            width: 0,
            height: 0,
            frame_rate: AUDIO_TICK_RATE,
            duration_ticks: duration_ticks_from_micros(AUDIO_TICK_RATE, micros.max(0) as u64),
            has_audio: true,
            video_codec: "none".into(),
        });
    };

    let par = stream.parameters();
    if par.medium() != Type::Video {
        return Err(ProbeError::unsupported("best stream is not video"));
    }

    let codec_id = par.id();
    let video = Context::from_parameters(par)
        .map_err(ProbeError::Open)?
        .decoder()
        .video()
        .map_err(ProbeError::Open)?;
    let width = video.width();
    let height = video.height();
    if width == 0 || height == 0 {
        return Err(ProbeError::unsupported("zero video dimensions"));
    }

    let frame_rate = normalize_frame_rate(stream.avg_frame_rate());
    let micros = input.duration();
    let duration_ticks = duration_ticks_from_micros(frame_rate, micros.max(0) as u64);

    let video_codec = ffmpeg_next::codec::decoder::find(codec_id)
        .map(|c| c.name().to_string())
        .unwrap_or_else(|| "unknown".into());

    Ok(MediaProbe {
        width,
        height,
        frame_rate,
        duration_ticks,
        has_audio,
        video_codec,
    })
}

fn normalize_frame_rate(rate: FfRational) -> Rational {
    let frame_rate = Rational::new(rate.numerator(), rate.denominator());
    if frame_rate.is_valid() {
        frame_rate
    } else {
        Rational::FPS_24
    }
}

/// Map container duration (microseconds) to frame ticks at `frame_rate`.
pub fn duration_ticks_from_micros(frame_rate: Rational, micros: u64) -> i64 {
    if !frame_rate.is_valid() || micros == 0 {
        return 0;
    }
    let ticks = (i128::from(micros) * i128::from(frame_rate.num))
        / (i128::from(frame_rate.den) * 1_000_000);
    ticks.clamp(0, i128::from(i64::MAX)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_ticks_from_micros_one_second_at_24fps() {
        assert_eq!(
            duration_ticks_from_micros(Rational::FPS_24, 1_000_000),
            24
        );
    }

    #[test]
    fn duration_ticks_from_micros_zero_micros_is_zero() {
        assert_eq!(duration_ticks_from_micros(Rational::FPS_24, 0), 0);
    }

    #[test]
    fn normalize_frame_rate_falls_back_to_24() {
        assert_eq!(
            normalize_frame_rate(FfRational::new(0, 1)),
            Rational::FPS_24
        );
    }

    #[test]
    fn audio_only_file_probes_with_zero_dimensions() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/baby.mp3");
        if !path.exists() {
            return;
        }
        let probed = probe(&path).expect("probe mp3");
        assert_eq!(probed.width, 0);
        assert_eq!(probed.height, 0);
        assert!(probed.has_audio);
        assert_eq!(probed.frame_rate, AUDIO_TICK_RATE);
        assert!(probed.duration_ticks > 0);
        assert_eq!(probed.video_codec, "none");
    }
}
