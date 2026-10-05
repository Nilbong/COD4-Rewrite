// CoD4's glow (`glow_consistent_setup`, `filter_symmetric_*`): the bright
// parts of the frame, graded with the map's film, desaturated and blurred,
// for the film pass to add (`glow_apply_bloom`).

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct Film {
    tint_base: vec4<f32>,
    tint_delta: vec4<f32>,
    bias: vec4<f32>,
    // x: cutoff, y: 1 / (1 - cutoff), z: desaturation, w: intensity.
    glow: vec4<f32>,
    // x: blur radius (pixels at 640x480), y: 1 when on.
    glow_blur: vec4<f32>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> film: Film;

const LUMINANCE = vec3<f32>(0.299, 0.587, 0.114);

fn to_display(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

// The bright pass at a quarter of the frame's size: four taps a texel off
// the middle of each 4x4 block (each averaging 2x2), each graded with the
// film and weighted by how far its luminance is over the cutoff.
@fragment
fn setup(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(source));
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        let offset = vec2<f32>(f32(i & 1) * 2.0 - 1.0, f32(i >> 1u) * 2.0 - 1.0);
        let c = to_display(clamp(textureSampleLevel(source, source_sampler, in.uv + offset * texel, 0.0).rgb, vec3<f32>(0.0), vec3<f32>(1.0)));
        let lum = dot(c, LUMINANCE);
        let graded = mix(c, vec3<f32>(lum), film.bias.w) * (film.tint_base.rgb + film.tint_delta.rgb * lum) + film.bias.rgb;
        sum += graded * saturate(lum - film.glow.x) * film.glow.y;
    }
    let average = sum * 0.25;
    return vec4<f32>(mix(average, vec3<f32>(dot(average, LUMINANCE)), film.glow.z), 1.0);
}

// A Gaussian across `dir`, its radius given at 640x480.
fn blur(uv: vec2<f32>, dir: vec2<f32>) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(source));
    let sigma = max(film.glow_blur.x * size.y / 480.0 * 0.5, 0.5);
    let reach = i32(min(ceil(sigma * 3.0), 48.0));
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var i = -reach; i <= reach; i++) {
        let w = exp(-f32(i * i) / (2.0 * sigma * sigma));
        sum += textureSampleLevel(source, source_sampler, uv + dir * f32(i) / size, 0.0).rgb * w;
        total += w;
    }
    return vec4<f32>(sum / total, 1.0);
}

@fragment
fn blur_across(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return blur(in.uv, vec2<f32>(1.0, 0.0));
}

@fragment
fn blur_down(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return blur(in.uv, vec2<f32>(0.0, 1.0));
}
