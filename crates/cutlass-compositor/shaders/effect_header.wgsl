// effect_header.wgsl — shared preamble for per-layer effect passes.
//
// Every effect fragment shader is concatenated after this header
// (compositor.rs `effects::build_registry`). Effect passes are full-screen:
// a single oversized triangle covers the canvas and each pass samples the
// previous pass's result (`src_tex`) plus the untouched placed layer
// (`orig_tex`, for combine-style passes like glow). Scratch textures hold
// PREMULTIPLIED alpha (the placed layer is rendered over a transparent clear
// with src-over, which premultiplies), so filtering is correct and the final
// composite uses premultiplied src-over.

struct EffectParams {
    // Canvas size packed as (width, height, 1/width, 1/height) so passes can
    // step in texel units.
    resolution: vec4<f32>,
    // Effect parameters, slots 0..3 (named per effect in effects.rs).
    p0: vec4<f32>,
    // Effect parameters, slots 4..7.
    p1: vec4<f32>,
    // (pass_index, pass_count, _, _) for multi-pass effects (e.g. separable
    // blur uses pass_index to pick the axis).
    pass_info: vec4<f32>,
};

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var orig_tex: texture_2d<f32>;
@group(0) @binding(2) var fx_sampler: sampler;
@group(0) @binding(3) var<uniform> fx: EffectParams;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VsOut {
    // Fullscreen triangle: (-1,-1), (3,-1), (-1,3) covers clip space; the
    // [-1,1] visible region maps to uv [0,1] with v flipped (texture v=0 is
    // the top row, clip y=+1 is the top of screen).
    var corners = array<vec2<f32>, 3>(
        vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0),
    );
    let xy = corners[vi];
    var out: VsOut;
    out.position = vec4(xy, 0.0, 1.0);
    out.uv = vec2((xy.x + 1.0) * 0.5, (1.0 - xy.y) * 0.5);
    return out;
}
