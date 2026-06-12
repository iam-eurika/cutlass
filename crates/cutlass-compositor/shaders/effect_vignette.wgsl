// effect_vignette.wgsl — radial darkening toward the canvas corners.
//
// p0.x = amount (0 = off, 1 = corners fully black). Operates on premultiplied
// color: scaling rgb darkens while leaving coverage (alpha) intact.

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let px = textureSample(src_tex, fx_sampler, in.uv);
    let amount = clamp(fx.p0.x, 0.0, 1.0);
    // Distance from center, normalized so the corners sit at ~1.0.
    let d = distance(in.uv, vec2(0.5, 0.5)) * 1.4142135624;
    let v = 1.0 - amount * smoothstep(0.4, 1.0, d);
    return vec4(px.rgb * v, px.a);
}
