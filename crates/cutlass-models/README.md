# cutlass-models

`cutlass-models` is the shared data model for Cutlass projects. It defines the in-memory project, media pool, timeline, tracks, clips, time types, animation curves, generators, effects, transitions, and project-file schema used by the rest of the workspace.

This crate is intentionally independent from UI, decoding, rendering, AI, and export code. It is the place for data shape and editing invariants that should hold no matter which part of Cutlass is driving the project.

## Responsibilities

- Represent a `Project` with one `Timeline` and a media pool.
- Model video, audio, image, generated, text, shape, and solid-color clip sources.
- Provide strongly typed IDs such as `ClipId`, `TrackId`, `MediaId`, and `ProjectId`.
- Represent exact timeline math with `RationalTime`, `Rational`, and `TimeRange`.
- Store track state, clip transforms, crop/flip settings, audio settings, clip speed, markers, canvas settings, effects, and transitions.
- Provide `Param<T>`, `Keyframe`, and `Easing` for animatable clip parameters.
- Define persisted project schema constants and serialization helpers for `.cutlass` files.

## What Belongs Here

Add code to this crate when the change is about project state itself: new fields, validation rules, time conversion helpers, persistent schema changes, or reusable model-level calculations.

Do not add file I/O, FFmpeg calls, GPU work, Slint UI bindings, LLM prompts, or undo/redo dispatch here. Those belong in higher-level crates that depend on this model.

## Important Types

- `Project`: top-level editable project state.
- `Timeline`: tracks, clips, markers, canvas settings, and timeline-level queries.
- `Track`: one lane of video or audio content.
- `Clip`: a placed timeline item with source, timing, transform, audio, speed, crop, effects, and transition state.
- `MediaSource`: metadata for an imported media item.
- `Generator`: generated content such as text, solids, and shapes.
- `Param<T>`: constant or keyframed parameter value.

## Testing

Run the model tests with:

```bash
cargo test -p cutlass-models
```

Model tests should focus on invariants, time math, persistence shape, and behavior that can be checked without decoding or rendering media.
