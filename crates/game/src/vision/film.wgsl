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
    // y: 1 when the glow is on; z: how much brighter than `w` is stretched
    // (this game's highlight gain, 0 for CoD4's film).
    glow_blur: vec4<f32>,
    // The Rich look: x: its S-curve, y: its saturation, z: the exponent
    // that takes its pivot to 0.5.
    grade: vec4<f32>,
    // The lens: x grain, y vignette, z lens dirt, w the frame.
    lens: vec4<f32>,
    // Split toning: the shadows' tint and amount, the highlights'.
    split_dark: vec4<f32>,
    split_light: vec4<f32>,
}

// Interleaved gradient noise (per pixel, turning each frame).
fn ign(p: vec2<f32>, frame: f32) -> f32 {
    let q = p + vec2<f32>(5.588238, 5.588238) * frame;
    return fract(52.9829189 * fract(dot(q, vec2<f32>(0.06711056, 0.00583715))));
}

// Lens dirt: smudges and specks, made from the screen position.
fn dirt(uv: vec2<f32>) -> f32 {
    var d = 0.0;
    for (var i = 0; i < 6; i++) {
        let c = vec2<f32>(fract(sin(f32(i) * 12.9898) * 43758.5453), fract(sin(f32(i) * 78.233) * 43758.5453));
        let r = 0.08 + 0.12 * fract(sin(f32(i) * 3.7) * 9631.1);
        d += smoothstep(r, 0.0, distance(uv, c)) * 0.6;
    }
    let g = floor(uv * vec2<f32>(48.0, 27.0));
    let speck = step(0.985, fract(sin(dot(g, vec2<f32>(12.9898, 78.233))) * 43758.5453));
    return saturate(d + speck * 0.8);
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

// Where highlights start rolling off towards white.
const KNEE: f32 = 0.8;

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
    // The Rich look: more colour about the luminance, and an S-curve
    // (steeper in the middle, black and white kept) about a pivot near a
    // frame's usual mean, so darker frames aren't sunk further than bright
    // ones are lifted.
    if film.grade.x > 0.0 {
        let l2 = dot(out, vec3<f32>(0.299, 0.587, 0.114));
        out = max(vec3<f32>(l2) + (out - vec3<f32>(l2)) * film.grade.y, vec3<f32>(0.0));
        let x = clamp(out, vec3<f32>(0.0), vec3<f32>(1.0));
        var t = pow(x, vec3<f32>(film.grade.z));
        t += film.grade.x * 4.0 * (t - 0.5) * t * (1.0 - t);
        out += pow(t, vec3<f32>(1.0 / film.grade.z)) - x;
    }
    out += max(out - vec3<f32>(film.glow_blur.w), vec3<f32>(0.0)) * film.glow_blur.z;
    // Rolled off into white rather than cut at it: past the knee the
    // stretched highlights ease towards 1 (same slope at the knee), so a
    // sunlit wall or a bright sky keeps its detail instead of blowing out.
    out = select(out, vec3<f32>(1.0) - (1.0 - KNEE) * exp(-(out - vec3<f32>(KNEE)) / (1.0 - KNEE)), out > vec3<f32>(KNEE));
    var glow_c = vec3<f32>(0.0);
    if film.glow_blur.y > 0.5 {
        glow_c = textureSampleLevel(glow, screen_sampler, in.uv, 0.0).rgb;
        out += glow_c * film.glow.w;
    }
    // Split toning: shadows toward one hue, highlights toward another
    // (luminance kept).
    if film.split_dark.w > 0.0 {
        let l3 = dot(out, vec3<f32>(0.299, 0.587, 0.114));
        let toward = mix(film.split_dark.rgb, film.split_light.rgb, smoothstep(0.15, 0.7, l3));
        let toned = toward * l3 / max(dot(toward, vec3<f32>(0.299, 0.587, 0.114)), 1e-3);
        out = mix(out, toned, film.split_dark.w * 0.35);
    }
    // The lens: dirt lit by bright light (the glow, or the frame's own
    // bright parts nearby), a vignette, then grain (most in the shadows,
    // none in the highlights).
    if film.lens.z > 0.0 {
        var bright = glow_c;
        if film.glow_blur.y < 0.5 {
            var s = vec3<f32>(0.0);
            for (var i = 0; i < 8; i++) {
                let a = f32(i) * 0.785398;
                s += textureSampleLevel(screen, screen_sampler, in.uv + vec2<f32>(cos(a), sin(a)) * 0.08, 0.0).rgb;
            }
            bright = max(s / 8.0 - vec3<f32>(0.7), vec3<f32>(0.0)) * 2.0;
        }
        out += bright * dirt(in.uv) * film.lens.z;
    }
    if film.lens.y > 0.0 {
        let v = in.uv - 0.5;
        out *= 1.0 - film.lens.y * smoothstep(0.25, 0.75, dot(v, v) * 2.0);
    }
    if film.lens.x > 0.0 {
        let l4 = dot(out, vec3<f32>(0.299, 0.587, 0.114));
        let amount = film.lens.x * mix(1.6, 0.0, smoothstep(0.2, 0.9, l4));
        out += (ign(in.position.xy, film.lens.w) - 0.5) * amount;
    }
    return vec4<f32>(to_linear(clamp(out, vec3<f32>(0.0), vec3<f32>(1.0))), source.a);
}
