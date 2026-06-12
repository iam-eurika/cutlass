// composite.wgsl — blit a finished effect-layer scratch onto the canvas.
//
// The scratch holds PREMULTIPLIED color (the placed layer rendered over a
// transparent clear, then run through the effect chain). This pass scales by
// the layer opacity and blends with premultiplied src-over (configured in
// Rust: src_factor One, dst OneMinusSrcAlpha), which is the exact straight-
// alpha over-operator for premultiplied sources.

struct Placement {
    linear: vec4<f32>,
    trans_opacity: vec4<f32>,
    uv_rect: vec4<f32>,
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> placement: Placement;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

fn quad_corner(vertex_index: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2(-0.5, -0.5), vec2(0.5, -0.5), vec2(-0.5, 0.5),
        vec2(-0.5, 0.5), vec2(0.5, -0.5), vec2(0.5, 0.5),
    );
    return corners[vertex_index];
}

@vertex
fn vs(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let c = quad_corner(vertex_index);
    let m = placement.linear;
    let t = placement.trans_opacity;
    var out: VertexOutput;
    out.position = vec4(
        m.x * c.x + m.z * c.y + t.x,
        m.y * c.x + m.w * c.y + t.y,
        0.0,
        1.0,
    );
    out.uv = mix(placement.uv_rect.xy, placement.uv_rect.zw, c + vec2(0.5, 0.5));
    return out;
}

@fragment
fn fs(in: VertexOutput) -> @location(0) vec4<f32> {
    let px = textureSample(tex, samp, in.uv);
    let o = placement.trans_opacity.z;
    return vec4(px.rgb * o, px.a * o);
}
