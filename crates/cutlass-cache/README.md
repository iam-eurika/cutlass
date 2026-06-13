# cutlass-cache

`cutlass-cache` provides on-disk cache storage used by Cutlass preview paths. It stores decoded frame data with enough source metadata to avoid reusing stale frames after media or decode settings change.

The cache crate does not decode media and does not decide which frame should be rendered. It only provides storage primitives that higher-level crates can use safely.

## Responsibilities

- Define cache keys and source fingerprints.
- Store and retrieve decoded frame payloads on disk.
- Validate cached data against a `CacheSpec`.
- Memory-map cached frame files where useful.
- Keep cache-specific errors separate from engine and decoder errors.

## Main APIs

- `FrameCache`: on-disk frame cache.
- `CacheSpec`: cache format and frame layout description.
- `SourceFingerprint`: metadata used to decide whether cached data still belongs to a source.
- `SourceId`: cache-facing source identifier.
- `DiskCacheError`: errors from cache reads, writes, validation, and mapping.

## What Belongs Here

Add code here when the change is about durable cache layout, frame-cache lookup, source fingerprints, cache validation, or cache-specific errors.

Do not add media decoding, timeline selection, preview scheduling, or export behavior here. Those are owned by `cutlass-decoder`, `cutlass-engine`, and `cutlass-ui`.

## Testing

Run the cache tests with:

```bash
cargo test -p cutlass-cache
```

Cache tests should use temporary directories and should avoid relying on real media fixtures.
