// Rain (`weather/rain.rs`): streaks and splashes, placed here from the
// clock, the camera and the rain map. Each quad's four corners come in as
// (corner x -1..1, corner y 0..1, quad number).

#import bevy_pbr::mesh_view_bindings::view

// xyz: the box's centre (the camera), w: the clock.
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> centre: vec4<f32>;
// xyz: half size, w: the share of quads drawn.
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> extent: vec4<f32>;
// xyz: the drops' velocity, w: streak length per m/s.
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<uniform> velocity: vec4<f32>;
// rgb: light, w: lightning.
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> light: vec4<f32>;
// x: 0 near streaks, 1 far streaks, 2 splashes, 3 drips, 4 curtains,
// 5 near snowflakes, 6 far; y: map cell size;
// z: cells.
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var<uniform> kind: vec4<f32>;
// The rain map: how high rain gets in each cell.
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var heights: texture_2d<f32>;
// Toward the key light (the moon by night), and its light on the drops.
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var<uniform> key_dir: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(7) var<uniform> key_light: vec4<f32>;
// Snow: lamps near the camera, two each: (where, reach), (colour, -).
@group(#{MATERIAL_BIND_GROUP}) @binding(8) var<uniform> lamps: array<vec4<f32>, 16>;

// The near box's half size: the far layer leaves it alone.
const NEAR_EXTENT: vec3<f32> = vec3<f32>(14.0, 9.0, 14.0);
// Splashes a second, per splash.
const SPLASH_RATE: f32 = 2.2;
// How bright a flake is against the light it catches.
const SNOW_STRENGTH: f32 = 0.03;
// A lamp's light on a flake beside it, against the air's.
const LAMP_GLOW: f32 = 40.0;
// The near snow's box (half size): the far flakes leave it alone.
const SNOW_NEAR: vec3<f32> = vec3<f32>(12.0, 8.0, 12.0);
const PI: f32 = 3.14159265;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fade: f32,
    // Where it is (for the light's scatter, the curtains' noise).
    @location(2) world: vec3<f32>,
    // Snow: the lamps' light on the flake.
    @location(3) glow: vec3<f32>,
}

fn hash(n: u32) -> u32 {
    var x = n * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    return (x >> 22u) ^ x;
}

fn rand(n: u32) -> f32 {
    return f32(hash(n)) / 4294967295.0;
}

fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

// Smooth value noise, 0..1.
fn noise2(p: vec2<f32>) -> f32 {
    let c = floor(p);
    let f = p - c;
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash2(c), hash2(c + vec2<f32>(1.0, 0.0)), u.x), mix(hash2(c + vec2<f32>(0.0, 1.0)), hash2(c + vec2<f32>(1.0, 1.0)), u.x), u.y);
}

// How high rain gets at `p` (very low where not yet known: open).
fn height_at(p: vec3<f32>) -> f32 {
    let n = i32(kind.z + 0.5);
    let c = vec2<i32>(floor(p.xz / kind.y));
    let s = ((c % vec2<i32>(n)) + vec2<i32>(n)) % vec2<i32>(n);
    return textureLoad(heights, s, 0).r;
}

fn hidden(out: Out) -> Out {
    var o = out;
    o.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    o.fade = 0.0;
    return o;
}

@vertex
fn vertex(@location(0) v: vec3<f32>) -> Out {
    let i = u32(v.z + 0.5);
    let corner = v.xy;
    let k = i32(kind.x + 0.5);
    var out: Out;
    out.uv = corner;
    out.fade = 1.0;
    out.clip = vec4<f32>(0.0);
    out.world = vec3<f32>(0.0);
    // Light rain: fewer drops.
    if rand(i * 9u + 3u) > extent.w {
        return hidden(out);
    }
    let t = centre.w;
    let eye = view.world_position;
    if k == 5 || k == 6 {
        // A snowflake: drifting down through the box round the camera (and
        // wrapping in it, so it stays put in the world as the box moves),
        // fluttering side to side as it goes; none under a roof.
        let size = extent.xyz * 2.0;
        let lo = centre.xyz - extent.xyz;
        let base = vec3<f32>(rand(i * 9u), rand(i * 9u + 1u), rand(i * 9u + 2u)) * size;
        let speed = 0.8 + 0.4 * rand(i * 9u + 4u);
        let ph = rand(i * 9u + 5u) * 2.0 * PI;
        let f = 0.6 + 0.8 * rand(i * 9u + 6u);
        let flutter = vec3<f32>(sin(t * f * 2.1 + ph), 0.0, cos(t * f * 1.7 + ph * 1.3)) * (0.18 / f);
        let p0 = lo + fract((base + velocity.xyz * speed * t - lo) / size) * size;
        let p = p0 + flutter;
        let off = p0 - centre.xyz;
        if k == 6 && all(abs(off) < SNOW_NEAR) {
            return hidden(out);
        }
        if p.y < height_at(p) {
            return hidden(out);
        }
        let to_eye = eye - p;
        let dist = length(to_eye);
        let fwd = to_eye / max(dist, 1e-3);
        let side = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), fwd));
        let up = cross(fwd, side);
        // 1-2.5 cm across; at least a pixel or so far off, fainter as it grows.
        // (Near ones smaller: 0.6-1.6 cm across, far 1-2.6.)
        let r = select(0.005 + 0.008 * rand(i * 9u + 7u), 0.003 + 0.005 * rand(i * 9u + 7u), k == 5);
        let width = max(r, dist * 0.0007);
        let world = p + (side * corner.x + up * (corner.y * 2.0 - 1.0)) * width;
        out.world = world;
        let edge = min(min(extent.x - abs(off.x), extent.y - abs(off.y)), extent.z - abs(off.z));
        // Shrinking past a pixel they fade; the far field less so (a faint
        // depth), the nearest dimmer (none reads as a dot on the lens).
        let shrink = select((r / width) * (r / width), r / width, k == 6);
        let close = select(1.0, mix(0.45, 1.0, smoothstep(1.5, 5.0, dist)), k == 5);
        out.fade = smoothstep(0.4, 1.4, dist) * smoothstep(0.0, 2.0, edge) * shrink * close * (0.6 + 0.4 * rand(i * 9u + 8u));
        // The lamps it's passing: their colour, by how close (inside their
        // reach, falling off as its square).
        var glow = vec3<f32>(0.0);
        for (var l = 0u; l < 8u; l++) {
            let lamp = lamps[l * 2u];
            if lamp.w <= 0.0 {
                break;
            }
            let x = 1.0 - min(distance(p, lamp.xyz) / lamp.w, 1.0);
            glow += lamps[l * 2u + 1u].rgb * x * x;
        }
        out.glow = glow;
        out.clip = view.clip_from_world * vec4<f32>(world, 1.0);
        return out;
    }
    if k == 4 {
        // A distant curtain of rain: a big soft sheet standing in a ring
        // 30-150 m out, turned to face the eye, drifting with the wind.
        let ang = rand(i * 9u + 6u) * 2.0 * PI + t * 0.004;
        let r = mix(extent.x, extent.y, pow(rand(i * 9u + 7u), 0.7));
        let at = centre.xyz + vec3<f32>(cos(ang), 0.0, sin(ang)) * r;
        let to_eye = eye - at;
        let side = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), to_eye));
        let w = 18.0 + 22.0 * rand(i * 9u + 8u);
        let h = 40.0;
        let base = centre.y - 26.0;
        let world = vec3<f32>(at.x, base, at.z) + side * corner.x * w + vec3<f32>(0.0, corner.y * h, 0.0);
        out.world = world;
        out.fade = smoothstep(extent.x + 25.0, extent.x + 70.0, r) * (1.0 - smoothstep(extent.y * 0.7, extent.y, r));
        out.clip = view.clip_from_world * vec4<f32>(world, 1.0);
        return out;
    }
    if k == 3 {
        // A drip: where the rain map steps down by more than a metre (a
        // roof's or a container's edge), water runs off and falls.
        let phase = t * 0.9 + rand(i * 9u + 5u);
        let life = fract(phase);
        let s = i * 9u + u32(floor(phase)) * 7919u;
        let spot = centre.xyz + vec3<f32>((rand(s) * 2.0 - 1.0) * extent.x, 0.0, (rand(s + 1u) * 2.0 - 1.0) * extent.z);
        let a = f32(hash(s + 2u) % 4u) * PI * 0.5;
        let step = vec3<f32>(cos(a), 0.0, sin(a)) * kind.y;
        let top = height_at(spot);
        let below = height_at(spot + step);
        if top < -1.0e5 || below < -1.0e5 || top - below < 1.0 || top > centre.y + 6.0 {
            return hidden(out);
        }
        let fall = 0.5 * 9.8 * pow(life * sqrt(2.0 * (top - below) / 9.8), 2.0);
        // (Just past the lip, and hanging from it: the old drips started
        // half a cell out and a streak above the edge.)
        let p = vec3<f32>(spot.x, top - fall, spot.z) + step * 0.12;
        let to_eye = eye - p;
        let dist = length(to_eye);
        let side = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), to_eye));
        let width = max(0.0025, dist * 0.0007);
        // Close to the eye a drip would fill the view as a bright bar: it
        // fades out within ~1.5 m, and is dimmer and shorter up to ~4 m.
        let near = smoothstep(1.0, 4.0, dist);
        let len = min(0.18 * mix(0.4, 1.0, near), fall + 0.02);
        let world = p - vec3<f32>(0.0, (1.0 - corner.y) * len, 0.0) + side * corner.x * width;
        out.fade = (1.0 - smoothstep(extent.x * 0.6, extent.x, dist)) * (0.0025 / width) * 1.6 * smoothstep(1.5, 2.5, dist) * mix(0.35, 1.0, near);
        out.clip = view.clip_from_world * vec4<f32>(world, 1.0);
        return out;
    }
    if k == 2 {
        // A splash: a ripple ring spreading flat on whatever the rain lands
        // on. Each one has a place in the world (wrapped into the box round
        // the camera as the streaks are), so none slide as the camera moves;
        // a new place each time round.
        let phase = t * SPLASH_RATE + rand(i * 9u + 5u);
        let life = fract(phase);
        let s = i * 9u + u32(floor(phase)) * 7919u;
        let size2 = extent.xz * 2.0;
        let lo = centre.xz - extent.xz;
        let base = vec2<f32>(rand(s), rand(s + 1u)) * 997.0;
        let xz = lo + fract((base - lo) / size2) * size2;
        let spot = vec3<f32>(xz.x, 0.0, xz.y);
        let h = height_at(spot);
        if h < -1.0e5 || h > centre.y + 4.0 {
            return hidden(out);
        }
        let p = vec3<f32>(spot.x, h + 0.012, spot.z);
        let dist = length(eye - p);
        let radius = 0.025 + 0.06 * sqrt(life);
        let world = p + vec3<f32>(corner.x * radius, 0.0, (corner.y * 2.0 - 1.0) * radius);
        let edge = min(extent.x - abs(xz.x - centre.x), extent.z - abs(xz.y - centre.z));
        out.fade = (1.0 - smoothstep(extent.x * 0.5, extent.x, dist)) * smoothstep(0.0, 1.5, edge);
        out.world = vec3<f32>(life, 0.0, 0.0);
        out.clip = view.clip_from_world * vec4<f32>(world, 1.0);
        return out;
    }
    // A streak: drops fall through the box and wrap round in it, so they
    // stay put in the world as the box follows the camera.
    let size = extent.xyz * 2.0;
    let lo = centre.xyz - extent.xyz;
    let base = vec3<f32>(rand(i * 9u), rand(i * 9u + 1u), rand(i * 9u + 2u)) * size;
    let speed = 0.85 + 0.3 * rand(i * 9u + 4u);
    let vel = velocity.xyz * speed;
    let p = lo + fract((base + vel * t - lo) / size) * size;
    let off = p - centre.xyz;
    if k == 1 && all(abs(off) < NEAR_EXTENT) {
        return hidden(out);
    }
    // None indoors, under cover or below a roof.
    if p.y < height_at(p) {
        return hidden(out);
    }
    let dir = normalize(vel);
    // Blurred along its fall, as by a camera's shutter; far ones longer.
    let len = length(vel) * velocity.w * select(1.0, 2.5, k == 1) + 0.12;
    let head = p;
    let tail = p - dir * len;
    let to_eye = eye - p;
    let dist = length(to_eye);
    let side = normalize(cross(dir, to_eye));
    // At least a pixel or so wide however far, dimmer as it widens.
    let width = max(0.0022, dist * 0.0008);
    let world = mix(head, tail, corner.y) + side * corner.x * width;
    out.world = world;
    // Fading in close to the eye and out towards the box's edges.
    let edge = min(min(extent.x - abs(off.x), extent.y - abs(off.y)), extent.z - abs(off.z));
    // Most drops faint, a few brighter.
    let bright = 0.35 + 0.65 * pow(rand(i * 9u + 8u), 3.0);
    out.fade = smoothstep(0.6, 2.5, dist) * smoothstep(0.0, 2.0, edge) * (0.0022 / width) * bright;
    out.clip = view.clip_from_world * vec4<f32>(world, 1.0);
    return out;
}

@fragment
fn fragment(in: Out) -> @location(0) vec4<f32> {
    let k = i32(kind.x + 0.5);
    var a = 0.0;
    var strength = 0.0;
    if k == 5 || k == 6 {
        // A soft round flake, white: the air's light, and the moon's (some
        // forward scatter, much less than rain's).
        let d = length(vec2<f32>(in.uv.x, in.uv.y * 2.0 - 1.0));
        a = smoothstep(1.0, 0.25, d);
        let v = normalize(in.world - view.world_position);
        let cs = dot(v, normalize(key_dir.xyz));
        let g = 0.35;
        let hg = (1.0 - g * g) / pow(1.0 + g * g - 2.0 * g * cs, 1.5) / (4.0 * PI);
        let lit = light.rgb + key_light.rgb * (0.4 + 2.0 * hg);
        let air = dot(light.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        let white = vec3<f32>(dot(lit, vec3<f32>(0.2126, 0.7152, 0.0722))) * vec3<f32>(0.96, 0.98, 1.0) + in.glow * air * LAMP_GLOW;
        return vec4<f32>(white * max(a * in.fade, 0.0) * SNOW_STRENGTH, 0.0);
    }
    if k == 2 {
        // A ripple: a thin ring spreading out and fading (flat on the
        // ground; uv -1..1 across it).
        let life = in.world.x;
        let r = length(vec2<f32>(in.uv.x, in.uv.y * 2.0 - 1.0));
        let ring = 0.55 + 0.4 * life;
        a = smoothstep(0.07, 0.0, abs(r - ring)) * (1.0 - life) * (1.0 - life);
        // Faint, lit by the air's light only (no moon glow): dark at night.
        strength = 0.0012;
        let lum = dot(light.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        return vec4<f32>(vec3<f32>(lum) * max(a * in.fade, 0.0) * strength, 0.0);
    } else if k == 3 {
        let across = 1.0 - abs(in.uv.x);
        a = across * across * sin(in.uv.y * PI);
        // (Thin and clear: water, not white paint.)
        strength = 0.006;
    } else if k == 4 {
        // Streaky noise falling down the sheet, densest low (mist over the
        // sea), soft at the sides and top. Faint: a soldier 40 m off stays
        // plain.
        let t = centre.w;
        let q = vec2<f32>(in.world.x * 0.35 + in.world.z * 0.35, in.world.y * 0.05 + t * 0.9);
        let streaks = noise2(vec2<f32>(q.x * 0.6, q.y * 0.4)) * 0.75 + noise2(q * vec2<f32>(2.5, 1.2)) * 0.25;
        // Soft all round: sides, top, and the bottom (below the sea line).
        let edge = (1.0 - abs(in.uv.x)) * smoothstep(1.0, 0.6, in.uv.y) * smoothstep(0.0, 0.3, in.uv.y);
        let low = mix(1.0, 0.5, in.uv.y);
        a = mix(0.5, 1.0, smoothstep(0.2, 0.9, streaks)) * edge * edge * low;
        strength = 0.0005 * kind.w;
        // Grey: rain haze takes the light's brightness more than its hue.
        let lit = light.rgb + key_light.rgb * 0.15;
        let tint = vec3<f32>(dot(lit, vec3<f32>(0.2126, 0.7152, 0.0722)));
        return vec4<f32>(tint * max(a * in.fade, 0.0) * strength, 0.0);
    } else {
        let across = 1.0 - abs(in.uv.x);
        a = across * across * sin(in.uv.y * PI);
        strength = select(0.0045, 0.0018, k == 1);
        // Drops light up looking toward the key light (forward scatter,
        // Henyey-Greenstein, g 0.7) and catch some of it from any side;
        // grey-white, not tinted.
        let v = normalize(in.world - view.world_position);
        let cs = dot(v, normalize(key_dir.xyz));
        let g = 0.7;
        let hg = (1.0 - g * g) / pow(1.0 + g * g - 2.0 * g * cs, 1.5) / (4.0 * PI);
        let tinted = light.rgb + key_light.rgb * (0.25 + 3.0 * hg);
        let lit = vec3<f32>(dot(tinted, vec3<f32>(0.2126, 0.7152, 0.0722))) * vec3<f32>(0.95, 0.97, 1.0);
        return vec4<f32>(lit * max(a * in.fade, 0.0) * strength, 0.0);
    }
    // Alpha 0: added to what's behind, never covering it.
    return vec4<f32>(light.rgb * max(a * in.fade, 0.0) * strength, 0.0);
}
