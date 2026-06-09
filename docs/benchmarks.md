# Benchmarks

Cutlass uses [Criterion](https://bheisler.github.io/criterion.rs/) for local performance
measurement on hot paths. Benches are **not run in CI** — run them on your machine
before optimizing compositor, preview, or export code.

HTML reports are written to `target/criterion/` after each run.

## Quick start

```bash
# GPU compositor only (no FFmpeg asset required)
cargo bench -p cutlass-compositor --bench composite

# Engine preview (solid always; media when an asset is available)
cargo bench -p cutlass-engine --bench preview

# Point at a specific file
CUTLASS_BENCH_ASSET=assets/foo.mp4 cargo bench -p cutlass-engine --bench preview

# Full short export (48-frame solid timeline → MP4)
cargo bench -p cutlass-engine --bench export
```

Pass Criterion flags after `--`, e.g. `-- --sample-size 50`.

## Environment

| Variable | Used by | Purpose |
|----------|---------|---------|
| `CUTLASS_BENCH_ASSET` | `preview` | Path to an MP4 for media cold/warm cases. Falls back to the first `assets/*.mp4` if unset. |

GPU benches skip silently when no adapter is available (common in headless VMs).

## `cutlass-compositor` — `composite`

**File:** `crates/cutlass-compositor/benches/composite.rs`

Isolates the WGPU compositor: alpha-over blend + GPU readback to RGBA8. No decode,
no timeline.

| Case | What it measures |
|------|------------------|
| `compositor/solid/1080p_readback` | Single `CompositeLayer::Solid` fill |
| `compositor/rgba/1080p_upload_blend_readback` | One full-canvas RGBA texture upload + blend + readback |
| `compositor/stack/solid_plus_rgba_1080p` | Two layers (solid under semi-transparent RGBA) |

Throughput is reported in bytes/sec (1080p RGBA ≈ 8 MiB/frame).

**Typical ranges (Apple Silicon, Metal):** solid ~1.5–2 ms; RGBA/stack ~2–3 ms per frame.

## `cutlass-engine` — `preview`

**File:** `crates/cutlass-engine/benches/preview.rs`

End-to-end `Engine::get_frame`: layer resolve → decode/cache → CPU resize → WGPU
composite → readback.

| Case | What it measures |
|------|------------------|
| `preview/get_frame/solid_1080p_warm` | Generated solid clip, repeated tick 0 (GPU only, no decode) |
| `preview/get_frame/media_cold_tick0` | Fresh engine + import + first `get_frame` (decode + cache miss) |
| `preview/get_frame/media_warm_tick0` | Same engine after priming; disk cache hit on YUV blob |

**How to read cold vs warm:** a large gap (e.g. 100 ms → 6 ms) means the frame
cache is doing its job. Warm `get_frame` still exceeds compositor-only time because
of YUV→RGBA conversion, bilinear resize, and texture upload.

## `cutlass-engine` — `export`

**File:** `crates/cutlass-engine/benches/export.rs`

Full `ProjectCommand::Export` on a 48-frame solid-color timeline at 1080p:
48× (`get_frame` + `VideoExport::push_rgba`) + encoder flush.

| Case | What it measures |
|------|------------------|
| `export/timeline/solid_48f_1080p_mp4` | Per-export wall time for a minimal timeline |

Use this to track export regressions before adding audio mux or multi-track complexity.

## Interpreting results

1. **Compare before/after** a single change — Criterion stores baselines under
   `target/criterion/` and shows regressions on re-runs.
2. **Compositor vs preview** — if preview solid ≈ compositor solid, GPU readback
   dominates; if preview media warm ≫ compositor, optimize decode/resize/upload path.
3. **Export** — divide wall time by frame count (~48) for per-frame export cost;
   compare to `get_frame` warm + encoder push overhead.

## Related

- `frames/bench_readback.py` — disk I/O micro-benches for cached YUV/RGBA layouts
  (separate from these Rust benches).
- `.cursor/rules/perf.mdc` — project performance guidelines.
