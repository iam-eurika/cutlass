//! File probing: inspect a container with ffmpeg and describe its streams.
//!
//! No frames are decoded here — we only read codec parameters and stream
//! metadata, which is fast and side-effect-free.

use std::path::Path;

use ffmpeg_next as ff;
use num_rational::Rational64;
use tracing::debug;

use crate::{AudioStreamInfo, MediaError, MediaInfo, Result, VideoStreamInfo};

pub fn probe(ictx: &ff::format::context::Input, path: &Path) -> Result<MediaInfo> {
    let format_name = ictx.format().name().to_string();
    let duration = av_time_to_rational(ictx.duration());

    let mut video = None;
    let mut audio = None;

    for stream in ictx.streams() {
        let params = stream.parameters();
        let codec_ctx = ff::codec::context::Context::from_parameters(params)?;
        match codec_ctx.medium() {
            ff::media::Type::Video if video.is_none() => {
                video = Some(describe_video(&stream, codec_ctx)?);
            }
            ff::media::Type::Audio if audio.is_none() => {
                audio = Some(describe_audio(&stream, codec_ctx)?);
            }
            _ => {}
        }
    }

    if video.is_none() && audio.is_none() {
        return Err(MediaError::NoStreams(path.to_path_buf()));
    }

    let info = MediaInfo {
        path: path.to_path_buf(),
        format_name,
        duration,
        video,
        audio,
    };
    debug!(?info, "probed");
    Ok(info)
}

fn describe_video(
    stream: &ff::format::stream::Stream,
    codec_ctx: ff::codec::context::Context,
) -> Result<VideoStreamInfo> {
    let codec_name = codec_ctx.id().name().to_string();
    let decoder = codec_ctx.decoder().video()?;

    let pix_fmt = format!("{:?}", decoder.format()).to_lowercase();
    let avg = stream.avg_frame_rate();
    let frame_rate = if avg.numerator() > 0 && avg.denominator() > 0 {
        Some(Rational64::new(
            avg.numerator() as i64,
            avg.denominator() as i64,
        ))
    } else {
        None
    };

    Ok(VideoStreamInfo {
        stream_index: stream.index(),
        codec: codec_name,
        width: decoder.width(),
        height: decoder.height(),
        pix_fmt,
        frame_rate,
        time_base: rational_from_ff(stream.time_base()),
        rotation: read_rotation(stream),
    })
}

fn describe_audio(
    stream: &ff::format::stream::Stream,
    codec_ctx: ff::codec::context::Context,
) -> Result<AudioStreamInfo> {
    let codec_name = codec_ctx.id().name().to_string();
    let decoder = codec_ctx.decoder().audio()?;
    let sample_fmt = format!("{:?}", decoder.format()).to_lowercase();

    Ok(AudioStreamInfo {
        stream_index: stream.index(),
        codec: codec_name,
        sample_rate: decoder.rate(),
        channels: decoder.channel_layout().channels() as u16,
        sample_fmt,
        time_base: rational_from_ff(stream.time_base()),
    })
}

fn read_rotation(stream: &ff::format::stream::Stream) -> i32 {
    if let Some(v) = stream.metadata().get("rotate")
        && let Ok(n) = v.parse::<i32>()
    {
        return ((n % 360) + 360) % 360;
    }
    0
}

pub(crate) fn rational_from_ff(r: ff::Rational) -> Rational64 {
    if r.denominator() == 0 {
        Rational64::new(0, 1)
    } else {
        Rational64::new(r.numerator() as i64, r.denominator() as i64)
    }
}

fn av_time_to_rational(d: i64) -> Rational64 {
    if d <= 0 {
        Rational64::new(0, 1)
    } else {
        Rational64::new(d, ff::ffi::AV_TIME_BASE as i64)
    }
}
