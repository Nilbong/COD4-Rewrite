// God rays (`crate::atmos::godrays`), at half resolution:
// - mask: the frame's sky (depth at the far plane), brightest round the
//   sun; anything solid is black, so only the sky and openings give light;
// - blur: each pixel gathers the mask along the line to the sun, fading
//   with each step, so light streams out past what stands in front;
// - add: the rays in the sun's colour onto the frame, levelling off
//   softly at `cap`.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct GodRays {
    sun_uv: vec2<f32>,
    strength: f32,
    cap: f32,
    color: vec3<f32>,
    density: f32,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> rays: GodRays;

const SAMPLES: i32 = 40;
const DECAY: f32 = 0.975;

@group(0) @binding(3) var depth: texture_depth_2d;

@fragment
fn mask(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    // The share of the four full-resolution pixels under this one that are
    // sky (reverse z: the far plane is 0).
    let size = vec2<f32>(textureDimensions(depth));
    let base = vec2<i32>(in.uv * size - 0.5);
    let top = vec2<i32>(size) - 1;
    var sky = 0.0;
    for (var i = 0; i < 4; i += 1) {
        let p = clamp(base + vec2(i & 1, i >> 1), vec2(0), top);
        sky += select(0.0, 0.25, textureLoad(depth, p, 0) <= 0.0);
    }
    if (sky <= 0.0) {
        return vec4(0.0);
    }
    let c = textureSampleLevel(source, source_sampler, in.uv, 0.0).rgb;
    // Brightest near the sun: the sky's glow there, not the whole dome.
    let aspect = size.x / size.y;
    let d = length((in.uv - rays.sun_uv) * vec2(aspect, 1.0));
    // A tight core: what stands in front of it cuts streaks.
    let near_sun = pow(saturate(1.0 - d * 2.2), 2.0);
    // (The sun's disc is far brighter than the sky: clamped, so it doesn't
    // flood every ray.)
    return vec4(min(c, vec3(2.0)) * sky * near_sun, 1.0);
}

@fragment
fn blur(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let step = (in.uv - rays.sun_uv) * (rays.density / f32(SAMPLES));
    var uv = in.uv;
    var weight = 1.0;
    var sum = vec3(0.0);
    for (var i = 0; i < SAMPLES; i += 1) {
        sum += textureSampleLevel(source, source_sampler, uv, 0.0).rgb * weight;
        weight *= DECAY;
        uv -= step;
    }
    // A weighted average of the sky along the way to the sun.
    return vec4(sum * (1.0 - DECAY) / (1.0 - pow(DECAY, f32(SAMPLES))), 1.0);
}

@fragment
fn add(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let r = textureSampleLevel(source, source_sampler, in.uv, 0.0).rgb;
    // Strongest round the sun, gone a screen's height away.
    let size = vec2<f32>(textureDimensions(source));
    let d = length((in.uv - rays.sun_uv) * vec2(size.x / size.y, 1.0));
    let lum = dot(r, vec3(0.2126, 0.7152, 0.0722)) * rays.strength * saturate(1.0 - d);
    // Levels off softly: looking into the sun never hides anyone.
    let amount = rays.cap * (1.0 - exp(-lum / rays.cap));
    return vec4(rays.color * amount, 0.0);
}
