// The showcase ocean (`crate::ocean`): Gerstner waves moved in the vertex
// shader, lit per pixel with the waves' own normal plus finer ripples.
// Radiance in physical units times the view's exposure, as Bevy's lighting
// gives it, then the map's fog.

#import bevy_pbr::{
    mesh_functions,
    mesh_view_bindings::{view, lights},
    view_transformations::position_world_to_clip,
    shadows::fetch_directional_shadow,
    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
}
#ifdef DISTANCE_FOG
#import bevy_pbr::{mesh_view_bindings::fog, pbr_functions::apply_fog}
#endif

struct Ocean {
    // x, y: wind direction (x, z); z: seconds; w: wind 0..1.
    wind: vec4<f32>,
    // x: rain; y: rest height; z: skybox brightness; w: hull map present.
    misc: vec4<f32>,
    // Hull map corner (x, z) and size (x, z), metres.
    hull_rect: vec4<f32>,
    // The skybox's turn (quaternion).
    sky_rotation: vec4<f32>,
    // Towards the moon, and how much night it is.
    moon: vec4<f32>,
    // The moon's light (lux).
    moon_light: vec4<f32>,
    // rgb: the sky's colour at the horizon line, after exposure; w: 1 if set.
    horizon: vec4<f32>,
    // x: debug view (1 reflection, 2 body, 3 glints, 4 foam); y: cloud cover.
    debug: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> ocean: Ocean;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var sky: texture_cube<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var sky_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var hull: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var hull_sampler: sampler;

const PI: f32 = 3.14159265;
const G: f32 = 9.81;
// The hull map's range (metres for 1).
const HULL_RANGE: f32 = 32.0;
const WAVES: u32 = 6u;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
};

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) world: vec3<f32>,
    // The undisplaced spot (x, z), and how far the waves lifted it.
    @location(1) rest: vec3<f32>,
};

// A wave: direction (degrees from the wind), wavelength (m), amplitude at
// full wind (m).
fn wave(i: u32) -> vec3<f32> {
    switch i {
        case 0u: { return vec3(0.0, 64.0, 1.1); }
        case 1u: { return vec3(28.0, 33.0, 0.55); }
        case 2u: { return vec3(-17.0, 19.0, 0.3); }
        case 3u: { return vec3(51.0, 11.5, 0.16); }
        case 4u: { return vec3(-38.0, 7.0, 0.09); }
        default: { return vec3(12.0, 4.3, 0.05); }
    }
}

fn rotate(v: vec2<f32>, deg: f32) -> vec2<f32> {
    let a = radians(deg);
    let c = cos(a);
    let s = sin(a);
    return vec2(v.x * c - v.y * s, v.x * s + v.y * c);
}

// How far from the hull (metres), and 1 well clear of it.
fn hull_distance(p: vec2<f32>) -> f32 {
    if (ocean.misc.w < 0.5) {
        return HULL_RANGE;
    }
    let uv = (p - ocean.hull_rect.xy) / ocean.hull_rect.zw;
    return textureSampleLevel(hull, hull_sampler, uv, 0.0).r * HULL_RANGE;
}

// The waves' scale: calm to storm, and calmer close in to the hull.
fn wave_scale(p: vec2<f32>) -> f32 {
    let wind = ocean.wind.w;
    let lee = mix(0.35, 1.0, smoothstep(1.0, 14.0, hull_distance(p)));
    return (0.2 + 1.9 * pow(wind, 1.5) + 0.2 * wind) * lee;
}

// Gerstner displacement at `p` (rest x, z): offset (x, y, z), and the
// normal's x, z slopes and the fold (Jacobian) in the last.
struct Waves {
    offset: vec3<f32>,
    slope: vec2<f32>,
    fold: f32,
}

fn waves(p: vec2<f32>, t: f32, scale: f32, count: u32) -> Waves {
    var w: Waves;
    w.offset = vec3(0.0);
    w.slope = vec2(0.0);
    w.fold = 1.0;
    let wind = normalize(ocean.wind.xy);
    let steep = 0.5 + 0.45 * ocean.wind.w;
    for (var i = 0u; i < count; i = i + 1u) {
        let def = wave(i);
        let d = rotate(wind, def.x);
        let k = 2.0 * PI / def.y;
        let c = sqrt(G / k);
        let a = def.z * scale;
        let q = steep / (k * a * f32(WAVES) + 1e-4);
        let theta = k * (dot(d, p) - c * t);
        let cs = cos(theta);
        let sn = sin(theta);
        w.offset += vec3(q * a * d.x * cs, a * sn, q * a * d.y * cs);
        w.slope += d * (k * a * cs);
        w.fold -= q * k * a * sn;
    }
    return w;
}

@vertex
fn vertex(v: Vertex) -> Out {
    var out: Out;
    let world_from_local = mesh_functions::get_world_from_local(v.instance_index);
    let rest = mesh_functions::mesh_position_local_to_world(world_from_local, vec4(v.position, 1.0)).xyz;
    let t = ocean.wind.z;
    // Far out (the coarse ring to the horizon) the waves die away: there
    // they'd only alias, and the haze hides them.
    let far = 1.0 - smoothstep(350.0, 1200.0, distance(rest.xz, view.world_position.xz));
    let w = waves(rest.xz, t, wave_scale(rest.xz) * far, WAVES);
    let p = rest + w.offset;
    out.world = p;
    out.rest = vec3(rest.x, rest.z, w.offset.y);
    out.position = position_world_to_clip(p);
    return out;
}

fn hash2(p: vec2<f32>) -> vec2<f32> {
    let q = vec2(dot(p, vec2(127.1, 311.7)), dot(p, vec2(269.5, 183.3)));
    return fract(sin(q) * 43758.5453);
}

// Smooth value noise (0..1).
fn noise(p: vec2<f32>) -> f32 {
    let c = floor(p);
    let f = p - c;
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(c).x;
    let b = hash2(c + vec2(1.0, 0.0)).x;
    let d = hash2(c + vec2(0.0, 1.0)).x;
    let e = hash2(c + vec2(1.0, 1.0)).x;
    return mix(mix(a, b, u.x), mix(d, e, u.x), u.y);
}

// Fine ripples riding the swell: a few short travelling waves' slopes.
fn ripples(p: vec2<f32>, t: f32, strength: f32) -> vec2<f32> {
    var s = vec2(0.0);
    let wind = normalize(ocean.wind.xy);
    for (var i = 0u; i < 5u; i = i + 1u) {
        let fi = f32(i);
        let d = rotate(wind, -50.0 + 27.0 * fi);
        let len = 2.4 / (1.0 + fi * 0.8);
        let k = 2.0 * PI / len;
        let c = sqrt(G / k);
        let a = 0.012 * len;
        s += d * (k * a * cos(k * (dot(d, p) - c * t) + fi * 1.7));
    }
    return s * strength;
}

// Rain: rings spreading out from drops, one a cell now and then.
fn rain_rings(p: vec2<f32>, t: f32, amount: f32) -> vec2<f32> {
    if (amount <= 0.0) {
        return vec2(0.0);
    }
    var s = vec2(0.0);
    let size = 0.6;
    let cell = floor(p / size);
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let c = cell + vec2(f32(i), f32(j));
            let h = hash2(c);
            let phase = fract(t * 1.4 + h.x);
            let centre = (c + 0.2 + 0.6 * h) * size;
            let d = p - centre;
            let r = length(d);
            let ring = phase * 0.45;
            let x = (r - ring) * 30.0;
            let wave = sin(x * 2.0) * exp(-x * x * 0.25) * (1.0 - phase);
            s += select(vec2(0.0), d / max(r, 1e-3) * wave, r < 0.6);
        }
    }
    return s * 0.35 * amount;
}

fn sky_radiance(dir: vec3<f32>, lod: f32) -> vec3<f32> {
    // The world direction turned by the skybox's rotation (as it lands on
    // screen: checked against the sky drawn behind the sea).
    let q = ocean.sky_rotation;
    let t = 2.0 * cross(q.xyz, dir);
    let d = dir + q.w * t + cross(q.xyz, t);
    return textureSampleLevel(sky, sky_sampler, d, lod).rgb * ocean.misc.z;
}

fn d_ggx(n_h: f32, rough: f32) -> f32 {
    let a2 = rough * rough * rough * rough;
    let d = n_h * n_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

@fragment
fn fragment(in: Out) -> @location(0) vec4<f32> {
    let t = ocean.wind.z;
    let p = in.rest.xy;
    let to_eye = view.world_position - in.world;
    let dist = length(to_eye);
    let v = to_eye / max(dist, 1e-4);
    let hull_d = hull_distance(p);
    let scale = wave_scale(p);
    let w = waves(p, t, scale, WAVES);
    // Detail fades with distance (it would only shimmer).
    let near = 1.0 - smoothstep(25.0, 140.0, dist);
    var slope = w.slope;
    slope += ripples(p, t, (0.4 + 0.8 * ocean.wind.w) * near);
    slope += rain_rings(p, t, ocean.misc.x * (1.0 - smoothstep(10.0, 40.0, dist)));
    let n = normalize(vec3(-slope.x, 1.0, -slope.y));
    let n_v = max(dot(n, v), 1e-3);

    // The map's sun (the one that casts shadows; else the brightest).
    var sun_dir = vec3(0.0, 1.0, 0.0);
    var sun_col = vec3(0.0);
    var sun_i = 0u;
    var best = -1.0;
    for (var i = 0u; i < lights.n_directional_lights; i = i + 1u) {
        let l = &lights.directional_lights[i];
        let shadowed = ((*l).flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u;
        let score = dot((*l).color.rgb, vec3(0.3, 0.6, 0.1)) + select(0.0, 1e9, shadowed);
        if (score > best) {
            best = score;
            sun_dir = (*l).direction_to_light;
            sun_col = (*l).color.rgb;
            sun_i = i;
        }
    }
    var shadow = 1.0;
    if (lights.n_directional_lights > 0u && (lights.directional_lights[sun_i].flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
        let view_z = dot(vec4(view.view_from_world[0].z, view.view_from_world[1].z, view.view_from_world[2].z, view.view_from_world[3].z), vec4(in.world, 1.0));
        shadow = fetch_directional_shadow(sun_i, vec4(in.world, 1.0), vec3(0.0, 1.0, 0.0), view_z, in.position.xy);
    }
    // The ship's shadow on the water: soft and faint (rough water, light
    // through cloud), never the dark blotches of a crisp shadow map.
    shadow = mix(1.0, shadow, mix(0.6, 0.2, ocean.moon.w));
    let n_l = max(dot(n, sun_dir), 0.0);

    // Fresnel (water's F0 0.02) and the sky reflected (below the horizon,
    // mirrored up: the sea doesn't see under itself).
    let rough = mix(0.04, 0.16, ocean.wind.w);
    // (A rough sea never mirrors fully, even edge-on.)
    let fresnel = 0.02 + (0.82 - 0.3 * ocean.wind.w - 0.02) * pow(1.0 - n_v, 5.0);
    var r = reflect(-v, n);
    // Looked up a little above the horizon: the bright band right at it
    // (a low sun's glow) otherwise tinted the whole sea.
    r.y = max(abs(r.y), 0.14);
    r = normalize(r);
    var reflected = sky_radiance(r, 1.0 + rough * 6.0);
    // A stormy sea's reflection is greyer than the sky, and never brighter
    // than the sky well up (a warm band low down tinted the whole sea).
    let up_sky = sky_radiance(normalize(vec3(r.x, 0.7, r.z)), 3.0);
    let lum = dot(reflected, vec3(0.2126, 0.7152, 0.0722));
    // (The sun's glow over a storm sea: broken up by the chop, greyer and
    // weaker than a calm sea's mirror.)
    reflected = mix(vec3(lum), reflected, 0.75) * mix(1.0, 0.75, ocean.wind.w);
    reflected = min(reflected, up_sky * 1.5);
    // Light from the whole sky, for the water's body and foam.
    // (Around the sky, not straight up, where a high sun's glow is.)
    var ambient = (sky_radiance(normalize(vec3(1.0, 0.6, 0.0)), 6.0) + sky_radiance(normalize(vec3(-1.0, 0.6, 0.0)), 6.0)
        + sky_radiance(normalize(vec3(0.0, 0.6, 1.0)), 6.0) + sky_radiance(normalize(vec3(0.0, 0.6, -1.0)), 6.0)) * 0.25;
    ambient = mix(vec3(dot(ambient, vec3(0.2126, 0.7152, 0.0722))), ambient, 0.5);

    // The body: deep blue-green, lighter where crests thin out and the sun
    // shows through them (subsurface).
    // (Seawater sends back little: a few hundredths at most.)
    // Deep grey-green, showing where the sea is seen from above.
    let deep = vec3(0.016, 0.026, 0.024);
    let shallow = vec3(0.024, 0.048, 0.042);
    let crest = saturate(in.rest.z / max(scale * 1.4, 0.2) * 0.5 + 0.5);
    let body_col = mix(deep, shallow, crest * 0.6);
    var body = body_col * (sun_col * n_l * shadow / PI + ambient);
    let through = pow(saturate(dot(v, -sun_dir) * 0.6 + 0.4), 4.0) * crest * crest;
    body += vec3(0.004, 0.025, 0.02) * sun_col / PI * through * shadow;

    // The sun's (or moon's) glint.
    let h = normalize(v + sun_dir);
    let spec = d_ggx(max(dot(n, h), 0.0), rough) * fresnel * 0.25 / n_v;
    var glint = sun_col * spec * n_l * shadow;
    // The moon's, at night: a narrow path of light on the water (when the
    // map's light isn't the moon already).
    if (ocean.moon.w > 0.01 && dot(ocean.moon.xyz, sun_dir) < 0.999) {
        let hm = normalize(v + ocean.moon.xyz);
        let m_l = max(dot(n, ocean.moon.xyz), 0.0);
        glint += ocean.moon_light.rgb * d_ggx(max(dot(n, hm), 0.0), rough * 0.8) * fresnel * 0.25 / n_v * m_l * ocean.moon.w;
    }

    // Foam: where the waves fold over, and along the hull, streaming off
    // with the current (a broken band of noise).
    let fold_foam = smoothstep(0.55, 0.2, w.fold) * (0.3 + 0.7 * ocean.wind.w);
    let flow = normalize(ocean.wind.xy) * t * 0.6;
    let grain = noise((p - flow) * 1.6) * 0.55 + noise((p - flow * 0.7) * 4.3) * 0.3 + noise(p * 9.0 - flow * 2.0) * 0.15;
    let hull_band = (1.0 - smoothstep(0.5, 3.0 + 3.0 * ocean.wind.w, hull_d)) * smoothstep(0.35, 0.75, grain + 0.25);
    let streak = (1.0 - smoothstep(3.0, 12.0, hull_d)) * smoothstep(0.62, 0.9, grain) * 0.6;
    // Out on the open sea in a blow: whitecaps breaking on the crests, and
    // long streaks of spray and foam laid down along the wind.
    let gale = smoothstep(0.35, 0.9, ocean.wind.w);
    let caps = smoothstep(0.62, 0.92, crest) * smoothstep(0.45, 0.8, noise((p - flow * 1.5) * 0.3) * 0.6 + grain * 0.4) * gale;
    let wd = normalize(ocean.wind.xy);
    let along = vec2(dot(p, wd), dot(p, vec2(-wd.y, wd.x)));
    let streak_n = noise(vec2(along.x * 0.05 - t * 0.4, along.y * 0.8)) * 0.75 + noise(along * vec2(0.15, 1.6)) * 0.25;
    let streaks = smoothstep(0.66, 0.97, streak_n) * gale * 0.3 * (1.0 - smoothstep(25.0, 90.0, dist));
    let foam = saturate(max(max(fold_foam * smoothstep(0.3, 0.7, grain + 0.2), max(hull_band, streak)), max(caps, streaks)));
    // (Grey by night: moonlit foam doesn't glow.)
    let foam_col = vec3(mix(0.75, 0.3, ocean.moon.w)) * (sun_col * max(dot(vec3(0.0, 1.0, 0.0), sun_dir), 0.0) * shadow / PI + ambient);

    // No path of light on the water under heavy overcast.
    // (Some still gets through: a wet sea always shows a sheen.)
    glint *= 1.0 - 0.6 * smoothstep(0.5, 0.85, ocean.debug.y);
    // By night: a dark, glossy sea, the sky's reflection and the moon's
    // sheen carrying it; the water's own body and the foam dim (they
    // glowed under the night's light).
    let night = ocean.moon.w;
    let body_n = body * mix(1.0, 0.25, night);
    let foam_n = foam * mix(1.0, 0.5, night);
    // By night more of the sky's sheen shows on the wave faces (a rough
    // sea is never seen quite straight down): dark grey-blue, not black.
    let sheen = max(fresnel, mix(0.0, 0.18, night));
    var c = mix(body_n * (1.0 - sheen) + reflected * sheen * mix(1.0, 1.4, night), foam_col * mix(1.0, 0.25, night), foam_n)
        + glint * (1.0 - foam_n) * mix(1.0, 1.5, night);
    // Far off the sea fades into the sky at the horizon: no edge.
    // (Taken a few degrees up: right at the horizon the dynamic sky's
    // clouds thin out to the bare night sky, black, which made a black
    // band between a grey sky and the sea.)
    let toward = normalize(vec3(-v.x, 0.12, -v.z));
    c = mix(c, sky_radiance(toward, 3.0) * 0.9, smoothstep(400.0, 2000.0, dist));
    switch u32(ocean.debug.x) {
        case 1u: { c = reflected * fresnel; }
        case 2u: { c = body * (1.0 - fresnel); }
        case 3u: { c = glint; }
        case 4u: { c = vec3(foam) * ambient; }
        case 5u: { c = sky_radiance(-v, 0.0); }
        default: {}
    }
    var out = vec4(c * view.exposure, 1.0);
    // Far out, exactly the colour the sky (and its fog) has at the horizon
    // line: sea and sky meet with no seam.
    if (ocean.horizon.w > 0.5) {
        out = vec4(mix(out.rgb, ocean.horizon.rgb, smoothstep(500.0, 2500.0, dist)), 1.0);
    }
#ifdef DISTANCE_FOG
    out = apply_fog(fog, out, in.world, view.world_position, in.position.xy);
#endif
    return out;
}
