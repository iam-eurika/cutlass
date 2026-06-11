//! Audio decode for waveform peak extraction.
//!
//! Decodes the best audio stream, downmixes to mono f32 via swresample, and
//! reduces the samples to per-bucket peak amplitudes — enough for a static
//! waveform image. Playback decode (clocked, seekable) is a separate concern
//! and will live here later.

use std::path::Path;

use ffmpeg_next::error::EAGAIN;
use ffmpeg_next::format;
use ffmpeg_next::media::Type;
use ffmpeg_next::software::resampling;
use ffmpeg_next::util::channel_layout::ChannelLayout;
use ffmpeg_next::util::format::sample::{Sample, Type as SampleType};
use ffmpeg_next::util::frame::audio::Audio;
use ffmpeg_next::{Error as FfmpegError, codec, packet::Packet};

use crate::error::DecodeError;
use crate::video::ensure_ffmpeg_init;

/// Samples folded into one coarse peak while streaming (final buckets are
/// reduced from these). ~23ms at 44.1kHz: fine enough for any waveform image.
const COARSE_CHUNK: usize = 1024;

/// Decode the whole audio stream of `path` and return `buckets` peak
/// amplitudes in `0.0..=1.0`, evenly spread across the stream's duration.
pub fn audio_peaks(path: &Path, buckets: usize) -> Result<Vec<f32>, DecodeError> {
    ensure_ffmpeg_init()?;
    if buckets == 0 {
        return Err(DecodeError::unsupported("zero waveform buckets"));
    }

    let path_str = path
        .to_str()
        .ok_or_else(|| DecodeError::unsupported("path is not valid UTF-8"))?;
    let mut input = format::input(path_str).map_err(DecodeError::Open)?;

    let stream = input
        .streams()
        .best(Type::Audio)
        .ok_or_else(|| DecodeError::unsupported("no audio stream found"))?;
    let stream_index = stream.index();

    let mut decoder = codec::Context::from_parameters(stream.parameters())
        .map_err(DecodeError::Open)?
        .decoder()
        .audio()
        .map_err(DecodeError::Open)?;

    let rate = decoder.rate();
    if rate == 0 {
        return Err(DecodeError::unsupported("audio stream reports zero sample rate"));
    }
    let layout = if decoder.channel_layout().channels() == 0 {
        ChannelLayout::default(i32::from(decoder.channels()))
    } else {
        decoder.channel_layout()
    };
    decoder.set_channel_layout(layout);

    let mut resampler = resampling::Context::get(
        decoder.format(),
        layout,
        rate,
        Sample::F32(SampleType::Packed),
        ChannelLayout::MONO,
        rate,
    )
    .map_err(DecodeError::Decode)?;

    let mut peaks = PeakAccumulator::new(COARSE_CHUNK);
    let mut decoded = Audio::empty();
    let mut demuxer_done = false;

    loop {
        match decoder.receive_frame(&mut decoded) {
            Ok(()) => {
                if decoded.channel_layout().channels() == 0 {
                    decoded.set_channel_layout(layout);
                }
                let mut mono = Audio::empty();
                resampler
                    .run(&decoded, &mut mono)
                    .map_err(DecodeError::Decode)?;
                peaks.push_frame(&mono);
            }
            Err(FfmpegError::Eof) => break,
            Err(FfmpegError::Other { errno }) if errno == EAGAIN => {
                if demuxer_done {
                    break;
                }
                let mut packet = Packet::empty();
                loop {
                    match packet.read(&mut input) {
                        Ok(()) if packet.stream() == stream_index => {
                            decoder.send_packet(&packet).map_err(DecodeError::Decode)?;
                            break;
                        }
                        Ok(()) => continue,
                        Err(FfmpegError::Eof) => {
                            demuxer_done = true;
                            decoder.send_eof().map_err(DecodeError::Decode)?;
                            break;
                        }
                        Err(e) => return Err(DecodeError::Io(e)),
                    }
                }
            }
            Err(e) => return Err(DecodeError::Decode(e)),
        }
    }

    // Drain whatever swresample still buffers.
    loop {
        let mut mono = Audio::new(Sample::F32(SampleType::Packed), COARSE_CHUNK, ChannelLayout::MONO);
        match resampler.flush(&mut mono) {
            Ok(_) if mono.samples() > 0 => peaks.push_frame(&mono),
            _ => break,
        }
    }

    let coarse = peaks.finish();
    if coarse.is_empty() {
        return Err(DecodeError::unsupported("audio stream has no samples"));
    }
    Ok(reduce_to_buckets(&coarse, buckets))
}

/// Streams mono f32 samples into fixed-size chunk peaks so memory stays
/// O(duration / COARSE_CHUNK) instead of O(samples).
struct PeakAccumulator {
    chunk: usize,
    coarse: Vec<f32>,
    current: f32,
    filled: usize,
}

impl PeakAccumulator {
    fn new(chunk: usize) -> Self {
        Self {
            chunk,
            coarse: Vec::new(),
            current: 0.0,
            filled: 0,
        }
    }

    fn push_frame(&mut self, mono: &Audio) {
        if mono.planes() == 0 || mono.samples() == 0 {
            return;
        }
        let samples = &mono.plane::<f32>(0)[..mono.samples().min(mono.plane::<f32>(0).len())];
        for &s in samples {
            self.current = self.current.max(s.abs().min(1.0));
            self.filled += 1;
            if self.filled == self.chunk {
                self.coarse.push(self.current);
                self.current = 0.0;
                self.filled = 0;
            }
        }
    }

    fn finish(mut self) -> Vec<f32> {
        if self.filled > 0 {
            self.coarse.push(self.current);
        }
        self.coarse
    }
}

/// Max-reduce `coarse` peaks into exactly `buckets` values.
fn reduce_to_buckets(coarse: &[f32], buckets: usize) -> Vec<f32> {
    let n = coarse.len();
    (0..buckets)
        .map(|b| {
            let start = b * n / buckets;
            let end = (((b + 1) * n).div_ceil(buckets)).clamp(start + 1, n);
            coarse[start..end].iter().copied().fold(0.0, f32::max)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn any_audio_asset() -> Option<PathBuf> {
        std::fs::read_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets"))
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "mp3"))
    }

    #[test]
    fn peaks_have_requested_bucket_count_and_range() {
        let Some(path) = any_audio_asset() else {
            return;
        };
        let peaks = audio_peaks(&path, 48).expect("peaks");
        assert_eq!(peaks.len(), 48);
        assert!(peaks.iter().all(|p| (0.0..=1.0).contains(p)));
        // Real audio is not pure silence.
        assert!(peaks.iter().any(|&p| p > 0.0));
    }

    #[test]
    fn reduce_to_buckets_takes_max_per_range() {
        let coarse = vec![0.1, 0.9, 0.2, 0.3, 0.8, 0.1];
        assert_eq!(reduce_to_buckets(&coarse, 3), vec![0.9, 0.3, 0.8]);
    }

    #[test]
    fn reduce_to_buckets_handles_fewer_coarse_than_buckets() {
        let coarse = vec![0.5, 1.0];
        let out = reduce_to_buckets(&coarse, 4);
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|p| *p == 0.5 || *p == 1.0));
    }

    #[test]
    fn zero_buckets_is_rejected() {
        let err = audio_peaks(Path::new("/nonexistent.mp3"), 0);
        assert!(matches!(err, Err(DecodeError::Unsupported { .. })));
    }
}
