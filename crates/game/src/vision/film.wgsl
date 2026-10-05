// A map's film settings (`vision/<map>.vision`) over the finished frame, as
// CoD4's film shaders (`postfx_color`, `vertcol_film`) do it in display
// space: the colour desaturated towards its luminance, times a tint running
// from the dark tint to the light one with luminance (contrast folded in),
// plus the brightness. Then the map's glow (`glow.wgsl`) on top, as
// `glow_apply_bloom` adds it.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct Film {
    // rgb: dark tint * contrast; w: 1 to invert.
    tint_base: vec4<f32>,
    // rgb: (light tint - dark tint) * contrast.
    tint_delta: vec4<f32>,
    // rgb: brightness, with contrast's offset; w: desaturation.
    bias: vec4<f32>,
    // w: the glow's intensity.
    glow: vec4<f32>,
    // y: 1 when the glow is on.
    glow_blur: vec4<f32>,
}

@group(0) @binding(0) var screen: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;
@group(0) @binding(2) var<uniform> film: Film;
@group(0) @binding(3) var glow: texture_2d<f32>;

fn to_display(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((max(c, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let source = textureSample(screen, screen_sampler, in.uv);
    let c = to_display(clamp(source.rgb, vec3<f32>(0.0), vec3<f32>(1.0)));
    let lum = dot(c, vec3<f32>(0.299, 0.587, 0.114));
    let grey = mix(c, vec3<f32>(lum), film.bias.w);
    var out = grey * (film.tint_base.rgb + film.tint_delta.rgb * lum) + film.bias.rgb;
    if film.tint_base.w > 0.5 {
        out = vec3<f32>(1.0) - out;
    }
    if film.glow_blur.y > 0.5 {
        out += textureSampleLevel(glow, screen_sampler, in.uv, 0.0).rgb * film.glow.w;
    }
    return vec4<f32>(to_linear(clamp(out, vec3<f32>(0.0), vec3<f32>(1.0))), source.a);
}
