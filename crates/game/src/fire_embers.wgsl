// Embers (`fire.rs`): sparks rising from fires and drifting round the
// eye, each placed here from the clock (no CPU work per spark). Each quad's
// corners come in as (corner x -1..1, corner y -1..1, quad number).

#import bevy_pbr::mesh_view_bindings::view

// x: clock, y: share drawn, z: fires, w: the eye's nearness to fire.
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: vec4<f32>;
// xy: wind (m/s), z: rain, w: 1 for the sparks round the eye.
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> air: vec4<f32>;
// The fires: xyz where, w size (1..3).
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<uniform> emitters: array<vec4<f32>, 16>;

const PI: f32 = 3.14159265;
// How bright a hot spark is (HDR: it blooms).
const GLOW: f32 = 7.0;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) colour: vec3<f32>,
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

// Curling air: a cheap swirl that grows with height and time.
fn swirl(p: vec3<f32>, t: f32, seed: f32) -> vec3<f32> {
    let a = sin(p.y * 1.3 + t * 1.7 + seed) + 0.5 * sin(p.y * 3.1 - t * 2.3 + seed * 2.0);
    let b = cos(p.y * 1.1 - t * 1.4 + seed * 1.7) + 0.5 * cos(p.y * 2.7 + t * 2.9 + seed * 0.3);
    return vec3<f32>(a, 0.0, b);
}

// A spark from a fire at `age` (s) into its life: where it is.
fn fire_spark(i: u32, f: vec4<f32>, age: f32) -> vec3<f32> {
    let seed = rand(i * 7u + 5u) * 6.28;
    let r = sqrt(rand(i * 7u + 1u)) * 0.45 * f.w;
    let ang = rand(i * 7u + 2u) * 6.28;
    // (From the flames' top, not the ground.)
    let start = f.xyz + vec3<f32>(cos(ang) * r, 0.5 + 0.35 * f.w, sin(ang) * r);
    // Rising fast at first, slowing as it cools; the higher, the more the
    // air curls it and the wind takes it.
    // (Lazily: a user found them whipping up too fast.)
    let rise = (1.0 + 1.0 * rand(i * 7u + 3u)) * (1.0 - exp(-age * 0.35)) / 0.35;
    let up = vec3<f32>(0.0, rise, 0.0);
    let curl = swirl((start + up) * 0.5, params.x * 0.12 + seed, seed) * min(rise * 0.2, 0.8);
    let drift = vec3<f32>(air.x, 0.0, air.y) * age * 0.35;
    return start + up + curl + drift;
}

// A spark round the eye: drifting through a box about it, wrapping.
fn near_spark(i: u32, t: f32) -> vec3<f32> {
    let size = vec3<f32>(9.0, 5.0, 9.0);
    let eye = view.world_position;
    let lo = eye - size * vec3<f32>(0.5, 0.35, 0.5);
    let base = vec3<f32>(rand(i * 5u), rand(i * 5u + 1u), rand(i * 5u + 2u)) * size;
    let vel = vec3<f32>(air.x * 0.4, 0.35 + 0.3 * rand(i * 5u + 3u), air.y * 0.4);
    let seed = rand(i * 5u + 4u) * 6.28;
    let p = lo + fract((base + vel * t - lo) / size) * size;
    return p + swirl(p, t * 0.5, seed) * 0.25;
}

@vertex
fn vertex(@location(0) v: vec3<f32>) -> Out {
    let i = u32(v.z + 0.5);
    var out: Out;
    out.uv = v.xy;
    out.colour = vec3<f32>(0.0);
    out.clip = vec4<f32>(0.0);
    if rand(i * 3u + 11u) > params.y {
        return hidden(out);
    }
    let t = params.x;
    var p = vec3<f32>(0.0);
    var p_before = vec3<f32>(0.0);
    var heat = 1.0;
    var size = 0.015;
    if air.w > 0.5 {
        // Round the eye: only as near a fire as the eye is; dim, a few.
        if rand(i * 3u + 7u) > params.w * 0.35 {
            return hidden(out);
        }
        p = near_spark(i, t);
        p_before = near_spark(i, t - 0.04);
        heat = 0.35 + 0.4 * rand(i * 3u + 9u);
        size = 0.002 + 0.003 * rand(i * 3u + 8u);
        // (Twinkling as it turns.)
        heat *= 0.6 + 0.4 * sin(t * (3.0 + 4.0 * rand(i)) + rand(i * 9u) * 6.28);
    } else {
        let n = u32(max(params.z, 1.0));
        // (About 400 a fire: a few fires near shared all the sparks between
        // them in thick clouds.)
        if i >= n * 400u {
            return hidden(out);
        }
        // (By the fire's size: a barrel sends up a few dozen at a time, a
        // burning building hundreds.)
        let fsize = emitters[i % n].w;
        if rand(i * 3u + 13u) > fsize * fsize * 0.05 {
            return hidden(out);
        }
        let f = emitters[i % n];
        // Each lives 2-5 s (shorter in the rain), born again at its fire.
        let life = (2.5 + 2.5 * rand(i * 3u + 1u)) * (1.0 - 0.5 * air.z);
        let phase = fract(t / life + rand(i * 3u + 2u));
        let age = phase * life;
        // (A new cycle: a new place in the fire, so the sparks don't repeat.)
        let cycle = u32(floor(t / life + rand(i * 3u + 2u)));
        let id = i + cycle * 7919u;
        p = fire_spark(id, f, age);
        p_before = fire_spark(id, f, max(age - 0.04, 0.0));
        heat = (1.0 - phase) * (1.0 - phase);
        // (Gone within 10-20 m of rising.)
        heat *= 1.0 - smoothstep(2.5, 9.0, p.y - f.y);
        // (Fading in over the first instant, out as it cools.)
        heat *= smoothstep(0.0, 0.05, phase);
        size = (0.005 + 0.008 * rand(i * 3u + 4u)) * (0.7 + 0.3 * f.w / 2.0);
    }
    // At most a few pixels across, however near (a spark up close is a
    // small streak, not a blob), and at least one far off (a point).
    let dist = distance(view.world_position, p);
    let px = 2.0 * dist / (view.clip_from_view[1][1] * view.viewport.w);
    size = clamp(size, px * 0.6, px * 1.4);
    // Hot white-yellow cooling through orange to dull red.
    let colour = mix(vec3<f32>(0.9, 0.12, 0.02), mix(vec3<f32>(1.0, 0.25, 0.02), vec3<f32>(1.0, 0.5, 0.12), saturate(heat * 2.0 - 1.0)), saturate(heat * 2.0));
    out.colour = colour * heat * GLOW;
    // A quad facing the eye, stretched along the spark's motion on screen.
    let to_eye = normalize(view.world_position - p);
    let motion = p - p_before;
    var along = motion - to_eye * dot(motion, to_eye);
    let len = length(along);
    var side = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), to_eye));
    var dir = cross(to_eye, side);
    var stretch = 1.0;
    if len > 1e-4 {
        dir = along / len;
        side = normalize(cross(dir, to_eye));
        // (Capped: where a spark wraps round the eye's box its last place
        // is far off, and it drew a line across the view.)
        stretch = min(1.0 + len / size * 0.6, 8.0);
    }
    let world = p + side * v.x * size + dir * v.y * size * stretch;
    out.clip = view.clip_from_world * vec4<f32>(world, 1.0);
    // (Thin streaks get brighter per pixel less: the same light spread out.)
    out.colour /= sqrt(stretch);
    return out;
}

@fragment
fn fragment(in: Out) -> @location(0) vec4<f32> {
    // A soft round glow, hot in the middle.
    let d = length(in.uv);
    let a = exp(-d * d * 4.0) * (1.0 - smoothstep(0.85, 1.0, d));
    return vec4<f32>(in.colour * a, 0.0);
}
