// Airborne particles (`motes.rs`): dust motes lit in sun shafts, and
// tumbling leaves, each placed here from the clock. Each quad's corners come
// in as (corner x -1..1, corner y -1..1, quad number).

#import bevy_pbr::mesh_view_bindings::{view, lights}
#import bevy_pbr::shadows::fetch_directional_shadow
#import bevy_pbr::mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT

// x: clock, y: share drawn, z: kind (0 dust, 1 leaves), w: the sun's light.
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: vec4<f32>;
// xy: wind (m/s), z: rain.
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> air: vec4<f32>;
// xyz: toward the sun.
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<uniform> sun: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> sun_color: vec4<f32>;

const PI: f32 = 3.14159265;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>,
    @location(2) colour: vec3<f32>,
    // x: the leaf's turn (its shape's angle), y: kind.
    @location(3) extra: vec2<f32>,
}

fn hash(n: u32) -> u32 {
    var x = n * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    return (x >> 22u) ^ x;
}

fn rand(n: u32) -> f32 {
    return f32(hash(n)) / 4294967295.0;
}

fn hidden(o: Out) -> Out {
    var out = o;
    out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    return out;
}

@vertex
fn vertex(@location(0) v: vec3<f32>) -> Out {
    let i = u32(v.z + 0.5);
    var out: Out;
    out.uv = v.xy;
    out.colour = vec3<f32>(0.0);
    out.world = vec3<f32>(0.0);
    out.extra = vec2<f32>(0.0, params.z);
    out.clip = vec4<f32>(0.0);
    if rand(i * 3u + 11u) > params.y {
        return hidden(out);
    }
    let t = params.x;
    let eye = view.world_position;
    let leaf = params.z > 0.5;
    // A box round the eye the particles drift through and wrap in (so they
    // stay put in the world as the eye moves).
    let size = select(vec3<f32>(12.0, 7.0, 12.0), vec3<f32>(30.0, 12.0, 30.0), leaf);
    let lo = eye - size * vec3<f32>(0.5, 0.4, 0.5);
    let base = vec3<f32>(rand(i * 5u), rand(i * 5u + 1u), rand(i * 5u + 2u)) * size;
    let seed = rand(i * 5u + 4u) * 6.28;
    var vel: vec3<f32>;
    var wobble: vec3<f32>;
    if leaf {
        // Blown on the wind, sinking slowly, swinging as they fall.
        vel = vec3<f32>(air.x, -0.6 - 0.4 * rand(i * 5u + 3u), air.y) * vec3<f32>(0.9, 1.0, 0.9);
        wobble = vec3<f32>(sin(t * 1.3 + seed), sin(t * 2.1 + seed * 1.7) * 0.4, cos(t * 1.1 + seed)) * 0.6;
    } else {
        // Hanging all but still in the air (no wind: motes show in shafts,
        // indoors as often as not), drifting a few centimetres.
        vel = vec3<f32>(0.0, 0.004 * sin(seed), 0.0);
        wobble = vec3<f32>(sin(t * 0.06 + seed), sin(t * 0.05 + seed * 1.3), cos(t * 0.055 + seed * 0.7)) * 0.06;
    }
    let p = lo + fract((base + vel * t - lo) / size) * size + wobble;
    let to_eye = normalize(eye - p);
    let side = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), to_eye));
    let up = cross(to_eye, side);
    let dist = distance(eye, p);
    let px = 2.0 * dist / (view.clip_from_view[1][1] * view.viewport.w);
    var size_m: f32;
    if leaf {
        size_m = max(0.035 + 0.025 * rand(i * 5u + 6u), px * 1.2);
        out.extra.x = t * (1.5 + 2.0 * rand(i * 5u + 7u)) + seed;
        // A dry olive-brown, varied; lit by the sky and sun about evenly.
        let tone = rand(i * 5u + 8u);
        out.colour = mix(vec3<f32>(0.22, 0.2, 0.08), vec3<f32>(0.38, 0.24, 0.1), tone) * (0.3 + 0.7 * params.w) * 0.35;
    } else {
        // A speck: about a pixel, never a blob near the eye.
        size_m = clamp(0.002 + 0.002 * rand(i * 5u + 6u), px * 0.5, px * 1.2);
        // Brightest looking toward the sun (dust scatters forward).
        let cs = dot(-to_eye, normalize(sun.xyz));
        let g = 0.7;
        let hg = (1.0 - g * g) / pow(1.0 + g * g - 2.0 * g * cs, 1.5) / (4.0 * PI);
        out.colour = sun_color.rgb * params.w * (0.15 + hg * 2.5) * 3.0;
        // (Fewer far off: the near ones carry it.)
        out.colour *= 1.0 - smoothstep(4.0, 7.0, dist);
    }
    let world = p + side * v.x * size_m + up * v.y * size_m;
    out.world = world;
    out.clip = view.clip_from_world * vec4<f32>(world, 1.0);
    return out;
}

// Whether the sun reaches `p` (its shadow map).
fn sunlit(p: vec3<f32>, frag: vec2<f32>) -> f32 {
    for (var k: u32 = 0u; k < lights.n_directional_lights; k = k + 1u) {
        let light = &lights.directional_lights[k];
        if (((*light).flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u && dot((*light).direction_to_light, normalize(sun.xyz)) > 0.999) {
            let view_z = dot(vec4<f32>(view.view_from_world[0].z, view.view_from_world[1].z, view.view_from_world[2].z, view.view_from_world[3].z), vec4<f32>(p, 1.0));
            return fetch_directional_shadow(k, vec4<f32>(p, 1.0), normalize(sun.xyz), view_z, frag);
        }
    }
    return 1.0;
}

@fragment
fn fragment(in: Out) -> @location(0) vec4<f32> {
    if in.extra.y > 0.5 {
        // A leaf: an almond shape, turning as it tumbles.
        let c = cos(in.extra.x);
        let s = sin(in.extra.x);
        let q = vec2<f32>(in.uv.x * c - in.uv.y * s, (in.uv.x * s + in.uv.y * c) * 2.2);
        let d = length(q);
        let a = 1.0 - smoothstep(0.8, 1.0, d);
        // (Seen edge-on as it turns: thinner.)
        let face = abs(sin(in.extra.x * 0.7));
        return vec4<f32>(in.colour * a * (0.3 + 0.7 * face), 0.0);
    }
    // A speck in a shaft: only where the sun reaches it.
    let lit = sunlit(in.world, in.clip.xy);
    let d = length(in.uv);
    let a = exp(-d * d * 3.0);
    return vec4<f32>(in.colour * a * lit, 0.0);
}
