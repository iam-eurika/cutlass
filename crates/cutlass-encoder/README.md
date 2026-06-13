# cutlass-encoder

`cutlass-encoder` writes video deliverables for Cutlass. It wraps FFmpeg encode and muxing for exported MP4 files and includes proxy-building support for seek-friendly media workflows.

The encoder receives frames and audio prepared by higher-level code. It does not evaluate timelines, composite layers, or decide what should be visible at a given timestamp.

## Responsibilities

- Initialize FFmpeg encode support.
- Encode composited RGBA frames into H.264 video.
- Mux video and audio into MP4 output.
- Track export statistics such as frame count, resolution, and timing.
- Build all-intra H.264 proxy files for source media.
- Keep encode-specific configuration and errors isolated from engine code.

## Main APIs

- `VideoExport`: frame-oriented export writer.
- `ExportConfig`: output settings such as dimensions, frame rate, and quality.
- `ExportStats`: summary of a completed export.
- `AUDIO_CHANNELS`: audio channel count expected by the export path.
- `build_proxy` and `build_proxy_with`: proxy generation helpers.
- `ProxyConfig`, `ProxyBuildOptions`, and `ProxyStats`: proxy configuration and reporting.
- `EncodeError`: errors from encode, mux, and proxy operations.

## How It Fits

`cutlass-engine` prepares timeline frames, renders audio, and feeds the encoder. The encoder should remain reusable for any caller that can provide the same frame and audio data.

If a change needs clip timing, transitions, preview caching, or project commands, implement it in `cutlass-engine` or lower model crates rather than here.

## Testing

Run tests with:

```bash
cargo test -p cutlass-encoder
```

Export behavior that depends on timeline content should also have coverage in `cutlass-engine`.
