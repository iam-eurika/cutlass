// effect_blur.wgsl — separable gaussian blur (fragment only).
//
// Two passes share this shader: pass 0 blurs horizontally, pass 1 vertically.
// p0.x = radius in texels (scales the tap spacing). Premultiplied input, so a
// straight weighted sum is the correct blur.

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let radius = max(fx.p0.x, 0.0);
    var dir = vec2(fx.resolution.z, 0.0);
    if (fx.pass_info.x > 0.5) {
        dir = vec2(0.0, fx.resolution.w);
    }
    var weights = array<f32, 5>(
        0.2270270270, 0.1945945946, 0.1216216216, 0.0540540541, 0.0162162162,
    );
    var sum = textureSample(src_tex, fx_sampler, in.uv) * weights[0];
    for (var i = 1; i < 5; i = i + 1) {
        let off = dir * f32(i) * radius;
        sum = sum + textureSample(src_tex, fx_sampler, in.uv + off) * weights[i];
        sum = sum + textureSample(src_tex, fx_sampler, in.uv - off) * weights[i];
    }
    return sum;
}
