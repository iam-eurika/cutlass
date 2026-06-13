# cutlass-decoder

`cutlass-decoder` handles media reading for Cutlass. It wraps FFmpeg-backed video decode, still-image decode, thumbnail generation, waveform analysis, and audio playback/render helpers.

This crate reads media. It does not own project state, timeline edits, compositing decisions, or UI state.

## Responsibilities

- Open video sources and decode frames.
- Build keyframe indexes for seeking.
- Support hardware-accelerated decode where FFmpeg and the platform allow it.
- Decode still images such as PNG, JPEG, and WebP.
- Generate thumbnails and video strips.
- Extract audio waveform peaks.
- Stream audio for clocked preview playback.
- Render retimed audio buffers and ducking curves used by higher-level editing features.

## Main APIs

- `Decoder`: video decode session.
- `DecodeOptions`: decode configuration.
- `DecodedFrame`: decoded frame data returned by the video path.
- `KeyframeIndex`: seek support data.
- `SourceInfo`: stream and source properties discovered by decode.
- `video_thumbnail` and `video_strip`: visual summaries for UI use.
- `decode_image`: still-image decode.
- `AudioReader`: audio stream reader.
- `audio_peaks` and `audio_peaks_per_second`: waveform helpers.
- `render_stretched` and `render_stretched_curve`: retimed audio helpers.

## FFmpeg

`cutlass-decoder` uses the workspace `ffmpeg-next` dependency. Local builds require FFmpeg development libraries, as described in the root README.

Hardware decode behavior is platform and FFmpeg-build dependent. Callers should treat hardware acceleration as an optimization, not a required feature.

## Testing

Run tests with:

```bash
cargo test -p cutlass-decoder
```

Decoder tests should keep fixtures small and should separate media-format coverage from timeline behavior. Timeline behavior belongs in `cutlass-engine` tests.
