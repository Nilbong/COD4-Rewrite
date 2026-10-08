// The settings' Render Scale (`render_scale.rs`): the world, drawn into the
// frame's top left at `share` of its size, stretched over all of it,
// filtered and lightly sharpened (a cross of neighbours, by `sharpen`).

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
// xy: the share of the frame drawn; z: sharpening.
@group(0) @binding(2) var<uniform> params: vec4<f32>;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(source));
    // Inside the drawn part (half a texel in from its edges).
    let lo = texel * 0.5;
    let hi = params.xy - texel * 0.5;
    let uv = clamp(in.uv * params.xy, lo, hi);
    let c = textureSample(source, source_sampler, uv);
    if (params.z <= 0.0) {
        return c;
    }
    let n = textureSample(source, source_sampler, clamp(uv - vec2(0.0, texel.y), lo, hi)).rgb;
    let s = textureSample(source, source_sampler, clamp(uv + vec2(0.0, texel.y), lo, hi)).rgb;
    let e = textureSample(source, source_sampler, clamp(uv + vec2(texel.x, 0.0), lo, hi)).rgb;
    let w = textureSample(source, source_sampler, clamp(uv - vec2(texel.x, 0.0), lo, hi)).rgb;
    let sharp = c.rgb + params.z * (4.0 * c.rgb - n - s - e - w);
    // Little past its neighbours (no halos).
    let low = min(min(min(n, s), min(e, w)), c.rgb);
    let high = max(max(max(n, s), max(e, w)), c.rgb);
    let room = (high - low) * 0.25;
    return vec4(max(clamp(sharp, low - room, high + room), vec3(0.0)), c.a);
}
