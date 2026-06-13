//! Offline varispeed render: time-stretch a clip's source window to a target
//! timeline length, preserving pitch (or shifting it for "chipmunk" mode).
//!
//! This is the backend the audio mixers use to make retimed clips audible
//! (audio roadmap M8 Phase 3). The realtime preview mixer and the export
//! mixer both call [`render_stretched`] with the *same* inputs, so what you
//! hear while scrubbing is what you ship.
//!
//! Backend: [Signalsmith Stretch](https://signalsmith-audio.co.uk/code/stretch/)
//! (MIT, header-only C++ with a thin Rust wrapper) — chosen over rubberband
//! (GPL) and FFmpeg's stretch filters so Cutlass keeps its MIT/Apache license
//! posture. The render is one-shot ([`Stretch::exact`]) so latency is
//! compensated internally and the whole span resolves deterministically; the
//! mixers cache the result like a proxy and serve it 1:1.

use std::path::Path;

use signalsmith_stretch::Stretch;

use crate::audio::playback::{AudioReader, CHANNELS};
use crate::error::DecodeError;

/// Render the source window `[src_start_frame, src_start_frame + src_frames)`
/// (output sample frames at `out_rate`) time-stretched to exactly `out_frames`
/// of interleaved stereo output.
///
/// - `reversed` walks the source window back-to-front (CapCut reverse).
/// - `transpose` is a frequency multiplier: `1.0` preserves pitch (the
///   varispeed default), `> 1.0` shifts up (the optional "chipmunk" mode where
///   pitch follows speed). For a clip sped up by `s×`, pass `s` to follow the
///   pitch or `1.0` to lock it.
///
/// Returns `out_frames * CHANNELS` samples (zero-padded if the source runs
/// short). A zero-length window or output yields silence.
pub fn render_stretched(
    path: &Path,
    out_rate: u32,
    src_start_frame: i64,
    src_frames: i64,
    out_frames: i64,
    reversed: bool,
    transpose: f32,
) -> Result<Vec<f32>, DecodeError> {
    let out_frames = out_frames.max(0) as usize;
    let output_len = out_frames * CHANNELS;
    if out_frames == 0 || src_frames <= 0 {
        return Ok(vec![0.0; output_len]);
    }

    let input = read_source_window(path, out_rate, src_start_frame, src_frames, reversed)?;

    let mut stretch = Stretch::preset_default(CHANNELS as u32, out_rate);
    if (transpose - 1.0).abs() > f32::EPSILON {
        stretch.set_transpose_factor(transpose, None);
    }
    let mut output = vec![0.0; output_len];
    stretch.exact(&input, &mut output);
    Ok(output)
}

/// Decode `src_frames` of interleaved stereo from `path` starting at output
/// frame `src_start_frame` (at `out_rate`), zero-padding a short tail or a
/// lead gap, and reversing the frame order when `reversed`.
fn read_source_window(
    path: &Path,
    out_rate: u32,
    src_start_frame: i64,
    src_frames: i64,
    reversed: bool,
) -> Result<Vec<f32>, DecodeError> {
    let frames = src_frames.max(0) as usize;
    let mut buf = vec![0.0f32; frames * CHANNELS];

    let mut reader = AudioReader::open(path, out_rate)?;
    reader.seek_to_frame(src_start_frame)?;
    // A stream that starts after the requested point leaves a lead gap; the
    // padding already in `buf` covers it (same handling as the mixers).
    let lead = reader
        .position()
        .map_or(0, |p| (p - src_start_frame).clamp(0, src_frames) as usize);

    let mut filled = lead;
    while filled < frames {
        let got = reader.read(&mut buf[filled * CHANNELS..])?;
        if got == 0 {
            break; // source exhausted: rest stays silent
        }
        filled += got;
    }

    if reversed {
        reverse_frames(&mut buf);
    }
    Ok(buf)
}

/// Reverse the frame order of an interleaved stereo buffer in place (each
/// `(L, R)` pair stays intact; only their order flips).
fn reverse_frames(buf: &mut [f32]) {
    let frames = buf.len() / CHANNELS;
    for i in 0..frames / 2 {
        let j = frames - 1 - i;
        for ch in 0..CHANNELS {
            buf.swap(i * CHANNELS + ch, j * CHANNELS + ch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const RATE: u32 = 48_000;

    fn audio_asset() -> Option<PathBuf> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        std::fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "mp3"))
    }

    #[test]
    fn reverse_frames_flips_pairs() {
        // Two stereo frames (L,R): [(1,2),(3,4)] reverses to [(3,4),(1,2)].
        let mut buf = vec![1.0, 2.0, 3.0, 4.0];
        reverse_frames(&mut buf);
        assert_eq!(buf, vec![3.0, 4.0, 1.0, 2.0]);
    }

    #[test]
    fn zero_output_is_silence() {
        let out = render_stretched(Path::new("/nope.mp3"), RATE, 0, 48_000, 0, false, 1.0)
            .expect("zero output never touches the file");
        assert!(out.is_empty());
    }

    #[test]
    fn render_fills_the_requested_output_length() {
        let Some(path) = audio_asset() else {
            return;
        };
        // 1s of source (48000 frames) slowed to 2s (96000 frames): a ½×
        // speed clip. Output must be exactly the requested length and audible.
        let out = render_stretched(&path, RATE, 0, 48_000, 96_000, false, 1.0).expect("render");
        assert_eq!(out.len(), 96_000 * CHANNELS);
        assert!(out.iter().any(|&s| s != 0.0), "stretched audio is not silent");
        assert!(out.iter().all(|&s| s.is_finite()), "no NaNs/infs");
    }

    #[test]
    fn reversed_render_is_audible_and_distinct() {
        let Some(path) = audio_asset() else {
            return;
        };
        let forward = render_stretched(&path, RATE, 0, 48_000, 48_000, false, 1.0).expect("fwd");
        let reversed = render_stretched(&path, RATE, 0, 48_000, 48_000, true, 1.0).expect("rev");
        assert_eq!(forward.len(), reversed.len());
        assert!(reversed.iter().any(|&s| s != 0.0), "reversed audio plays");
        // Reversing real audio changes the waveform.
        let differ = forward
            .iter()
            .zip(&reversed)
            .any(|(a, b)| (a - b).abs() > 1e-3);
        assert!(differ, "reversed output differs from forward");
    }
}
