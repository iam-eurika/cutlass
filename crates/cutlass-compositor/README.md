# cutlass-compositor

`cutlass-compositor` is the GPU frame compositor for Cutlass preview and export. It combines visual layers into RGBA frames using WGPU.

The compositor receives already-resolved layer descriptions. It does not own timeline state, decode media files, or run UI interaction logic.

## Responsibilities

- Create and own WGPU compositor resources.
- Composite layers bottom-to-top with source-over alpha blending.
- Convert YUV420P media layers to RGB on the GPU.
- Render RGBA, solid-color, and frame-backed layers.
- Apply compositor-level placement, cropping, opacity, and layer effects.
- Provide RGBA output for preview and export readback.
- Provide helpers for RGBA to YUV420P conversion used by tests and fallback paths.

## Main APIs

- `GpuContext`: WGPU device, queue, and adapter context.
- `Compositor`: reusable compositor instance.
- `CompositeLayer`: a single layer to render.
- `LayerContent`: layer pixel source.
- `LayerPlacement`: layer transform and canvas placement.
- `LayerEffect`: effect data passed into the compositor.
- `RgbaImage`: owned RGBA image output.
- `Yuv420pImage` and `Yuv420pLayer`: planar YUV media input.

## Shader And GPU Notes

The crate uses WGSL shaders for GPU conversion and compositing. Keep per-frame allocations and GPU submissions bounded because preview scrubbing and playback are latency-sensitive paths.

If a feature needs timeline queries, media decode, text shaping, or UI state, implement those outside this crate and pass the compositor a resolved layer list.

## Testing And Benchmarks

Run tests with:

```bash
cargo test -p cutlass-compositor
```

Run local compositor benchmarks with:

```bash
cargo bench -p cutlass-compositor --bench composite
```

Benchmarks are intended for local performance checks and are not a replacement for visual correctness tests.
