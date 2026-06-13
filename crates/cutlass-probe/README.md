# cutlass-probe

`cutlass-probe` inspects media files before Cutlass imports them. It reads container, codec, stream, duration, frame-rate, resolution, and audio metadata without opening the full preview decode path.

The engine uses this crate at import and relink time to populate media-pool entries.

## Responsibilities

- Initialize FFmpeg probing as needed.
- Detect still-image paths supported by Cutlass.
- Probe media files for video, audio, and container metadata.
- Convert probed duration into Cutlass timeline ticks.
- Return model-facing metadata through `MediaProbe`.

## Main APIs

- `probe`: inspect a path and return metadata.
- `MediaProbe`: probed media description used to create or refresh `MediaSource` entries.
- `is_image_path`: helper for still-image imports.
- `duration_ticks_from_micros`: duration conversion helper.
- `ProbeError`: errors from unsupported, missing, or unreadable media.

## What Belongs Here

Add code here for import-time metadata discovery. Keep actual frame reads, thumbnail extraction, audio streaming, and waveform analysis in `cutlass-decoder`.

`cutlass-probe` should avoid depending on the engine. It should return data the engine can use, not apply project changes itself.

## Testing

Run tests with:

```bash
cargo test -p cutlass-probe
```

Tests that need real media should stay small and should make clear which fixture formats they cover.
