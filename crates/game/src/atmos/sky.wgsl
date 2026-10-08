// The dynamic sky (`crate::atmos::sky`): written into a cube map that the
// cameras' skybox draws, a face or all six at a time.
//
// - Its colours are the map's own skybox, blurred (its painted clouds and
//   sun smeared out), so each map keeps its mood.
// - Its horizon is the map's fog colour, so fogged distant scenery fades
//   into it.
// - Clouds: a layer 1.5-2.6 km up over a round earth, raymarched through
//   value-noise fbm and lit by the sun (Beer's law along the sun, a
//   two-lobed phase and some multiple-scattering brightening) and by the
//   sky; they drift with the wind and fade into the horizon far away.
// - The sun: a disc with limb darkening, and its glow, behind the clouds.
//
// Values are in the classic skybox's units (the skybox draws both at the
// same brightness).

struct SkyParams {
    // Toward the sun (Bevy space) and the disc's angular radius (radians).
    sun_dir: vec3<f32>,
    sun_size: f32,
    // The sun's colour, brightest channel 1.
    sun_color: vec3<f32>,
    // How much of the sky the clouds cover, 0..1 (below 0: from the skybox).
    coverage: f32,
    // The map's fog colour in the skybox's units.
    horizon: vec3<f32>,
    // 1 if the map has fog.
    fog: f32,
    // How far the clouds have drifted (m, x and z).
    wind: vec2<f32>,
    // Overcast: how much the clouds shade their own undersides (0..1).
    darkness: f32,
    // The first face and row this dispatch writes.
    face_base: u32,
    row_base: u32,
    // The showcase (`climate`): toward the moon, and 0 day .. 1 night.
    moon_dir: vec3<f32>,
    night: f32,
    // Toward a lightning bolt, and its flash now (0: none).
    bolt_dir: vec3<f32>,
    flash: f32,
    // The light the clouds take (the sun, or the moon at night).
    key_dir: vec3<f32>,
    // 1: a physical sky (Rayleigh and Mie scattering) in place of the
    // skybox's colours.
    physical: f32,
    // The key light's colour and strength (1: the sun at full).
    key_color: vec3<f32>,
    // How far the cloud tops rise past the usual (m): storm towers.
    cloud_extra: f32,
    // Seconds, for the stars' twinkle; the bolt's shape.
    time: f32,
    bolt_seed: f32,
}

@group(0) @binding(0) var<uniform> sky: SkyParams;
@group(0) @binding(1) var classic: texture_cube<f32>;
@group(0) @binding(2) var classic_sampler: sampler;
@group(0) @binding(3) var output: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(4) var noise: texture_3d<f32>;
@group(0) @binding(5) var noise_sampler: sampler;

const PI: f32 = 3.14159265;
const EARTH: f32 = 6360000.0;
const CLOUD_BASE: f32 = 1500.0;
const CLOUD_TOP: f32 = 2600.0;
const STEPS: u32 = 48u;
const LIGHT_STEPS: u32 = 4u;

// A cube map texel's direction in the world. The skybox looks the cube
// map up as it does the map's own (`crate::player`: CoD axes, the cube
// map's z flipped), at (x, z, y) for a world direction (x, y, z); this
// is that lookup undone, from Bevy's `sample_cube_dir`.
fn cube_dir(uv: vec2<f32>, face: u32) -> vec3<f32> {
    let c = 2.0 * uv - 1.0;
    var d: vec3<f32>;
    switch (face) {
        case 0u: { d = vec3( 1.0, -c.y, -c.x); }
        case 1u: { d = vec3(-1.0, -c.y,  c.x); }
        case 2u: { d = vec3( c.x,  1.0,  c.y); }
        case 3u: { d = vec3( c.x, -1.0, -c.y); }
        case 4u: { d = vec3( c.x, -c.y,  1.0); }
        default: { d = vec3(-c.x, -c.y, -1.0); }
    }
    return normalize(vec3(d.x, d.z, d.y));
}

// The classic skybox, blurred, toward `d` (Bevy space), looked up as the
// skybox does (see `cube_dir`).
fn classic_sky(d: vec3<f32>) -> vec3<f32> {
    let lod = max(f32(textureNumLevels(classic)) - 4.0, 0.0);
    return textureSampleLevel(classic, classic_sampler, vec3(d.x, d.z, d.y), lod).rgb;
}

// --- noise

fn hash(p: vec3<i32>) -> f32 {
    var v = bitcast<vec3<u32>>(p) * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> vec3(16u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return f32(v.x) * (1.0 / 4294967296.0);
}

// Smooth value noise, one lattice cell per unit: a tiling 3D texture of
// it (32 cells across, `sky.rs`).
fn value_noise(p: vec3<f32>) -> f32 {
    return textureSampleLevel(noise, noise_sampler, p * (1.0 / 32.0), 0.0).r;
}

fn fbm(p: vec3<f32>, octaves: u32) -> f32 {
    var sum = 0.0;
    var amp = 0.5;
    var q = p;
    var norm = 0.0;
    for (var i = 0u; i < octaves; i += 1u) {
        sum += amp * value_noise(q);
        norm += amp;
        amp *= 0.5;
        q = q * 2.03 + vec3(17.1, 3.7, 9.3);
    }
    return sum / norm;
}

// --- clouds

// Distance along `d` (from the ground) to a shell `h` metres up.
fn to_shell(dy: f32, h: f32) -> f32 {
    let k = h * (2.0 * EARTH + h);
    let b = EARTH * dy;
    return k / (b + sqrt(b * b + k));
}

fn coverage() -> f32 {
    if (sky.coverage >= 0.0) {
        return sky.coverage;
    }
    // From the skybox: a grey sky is an overcast one, a blue one clear.
    let z = classic_sky(vec3(0.0, 1.0, 0.0));
    let saturation = (max(z.r, max(z.g, z.b)) - min(z.r, min(z.g, z.b))) / max(max(z.r, max(z.g, z.b)), 1e-3);
    return mix(0.65, 0.35, smoothstep(0.1, 0.45, saturation));
}

// Cloud density (0..1) at `p` (m, from the eye), `h` its height through
// the layer (0..1); `detail` adds the wispy edges.
fn density(p: vec3<f32>, h: f32, cover: f32, detail: bool) -> f32 {
    let q = p + vec3(sky.wind.x, 0.0, sky.wind.y);
    let weather = fbm(vec3(q.x, 0.0, q.z) * (1.0 / 7000.0), 3u);
    // Billows: the noise folded about its middle (puffy cauliflower).
    // (fbm bunches round 0.5: spread first, or the fold is near 1 everywhere
    // and the deck goes flat.)
    let n = saturate((fbm(q * (1.0 / 1600.0), 5u) - 0.5) * 2.8 + 0.5);
    let shape = 1.0 - abs(n * 2.0 - 1.0);
    // (fbm bunches round 0.5: spread it over 0..1, so `cover` is about
    // the share of the sky covered.)
    let base = saturate((weather * 0.55 + shape * 0.45 - 0.5) * 3.5 + 0.5);
    let lo = 1.0 - cover;
    // Firm edges: cloud bodies against clear gaps.
    var d = smoothstep(lo - 0.05, lo + 0.18, base);
    // Flat bottoms, rounded tops (taller where the cloud is thicker).
    d *= smoothstep(0.0, 0.12, h) * smoothstep(1.0, mix(0.35, 0.8, d), h);
    if (detail && d > 0.0) {
        // Wispy edges.
        let wisps = fbm(q * (1.0 / 220.0), 3u);
        d = saturate(d - wisps * 0.45 * (1.0 - d));
    }
    return d;
}

fn hg(c: f32, g: f32) -> f32 {
    let denom = 1.0 + g * g - 2.0 * g * c;
    return (1.0 - g * g) / (4.0 * PI * denom * sqrt(denom));
}

// --- the physical sky (the showcase)

const ATMOSPHERE: f32 = 60000.0;
// The physical sky's noon zenith in the skybox's units (a CoD4 day sky's).
const DAY_ZENITH: f32 = 0.55;
// The night clouds' own faint light (share of the day's zenith).
const NIGHT_CLOUD: f32 = 0.15;
// The moon's light on the sky, against the sun's (more than real, so the
// night sky reads deep blue rather than black).
const MOONLIGHT: f32 = 0.02;
const RAYLEIGH: vec3<f32> = vec3(5.8e-6, 13.5e-6, 33.1e-6);
const MIE: f32 = 21e-6;
const RAYLEIGH_HEIGHT: f32 = 8000.0;
const MIE_HEIGHT: f32 = 1200.0;

// Light scattered toward the eye along `d` (from the ground) from a light
// toward `l` (single scattering, Rayleigh and Mie), for a light of
// radiance 1.
fn scatter(d: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let steps = 12;
    let dt = to_shell(max(d.y, 0.0), ATMOSPHERE) / f32(steps);
    var od_r = 0.0;
    var od_m = 0.0;
    var sum_r = vec3(0.0);
    var sum_m = vec3(0.0);
    for (var i = 0; i < steps; i += 1) {
        let p = d * ((f32(i) + 0.5) * dt);
        let height = max(length(vec2(length(p.xz), p.y + EARTH)) - EARTH, 0.0);
        let hr = exp(-height / RAYLEIGH_HEIGHT) * dt;
        let hm = exp(-height / MIE_HEIGHT) * dt;
        od_r += hr;
        od_m += hm;
        // Toward the light from here, unless the earth is in the way.
        let up = normalize(vec3(p.x, p.y + EARTH, p.z));
        let cos_l = dot(up, l);
        if (cos_l < -0.1) {
            continue;
        }
        let ldt = to_shell(max(cos_l, 0.0), ATMOSPHERE - height) / 4.0;
        var lod_r = 0.0;
        var lod_m = 0.0;
        for (var j = 0; j < 4; j += 1) {
            let q = p + l * ((f32(j) + 0.5) * ldt);
            let qh = max(length(vec2(length(q.xz), q.y + EARTH)) - EARTH, 0.0);
            lod_r += exp(-qh / RAYLEIGH_HEIGHT) * ldt;
            lod_m += exp(-qh / MIE_HEIGHT) * ldt;
        }
        let tau = RAYLEIGH * (od_r + lod_r) + MIE * 1.1 * (od_m + lod_m);
        let attenuation = exp(-tau) * smoothstep(-0.1, 0.02, cos_l);
        sum_r += attenuation * hr;
        sum_m += attenuation * hm;
    }
    let mu = dot(d, l);
    let phase_r = 3.0 / (16.0 * PI) * (1.0 + mu * mu);
    let g = 0.76;
    let phase_m = 3.0 / (8.0 * PI) * ((1.0 - g * g) * (1.0 + mu * mu)) / ((2.0 + g * g) * pow(1.0 + g * g - 2.0 * g * mu, 1.5));
    return sum_r * RAYLEIGH * phase_r + sum_m * MIE * phase_m;
}

// The night sky behind and under the clouds: the horizon colour at the
// horizon, a third of it overhead.
fn night_sky(d: vec3<f32>) -> vec3<f32> {
    return sky.horizon * mix(1.0, 0.33, pow(saturate(d.y), 0.6));
}

// Distant cloud banks over `behind` (see `main`).
fn cloud_banks(d: vec3<f32>, behind: vec3<f32>, horizon: vec3<f32>) -> vec3<f32> {
    let az = atan2(d.x, d.z);
    let e = asin(clamp(d.y, -1.0, 1.0));
    let storm = storm_of();
    var out = behind;
    for (var k = 0; k < 3; k += 1) {
        let kf = f32(k);
        // Round the horizon (no seam): low-frequency noise on a circle (a
        // few big masses), drifting; a finer octave makes cauliflower tops.
        let r = 1.6 + 0.9 * kf;
        let drift = sky.time * 0.003 * (1.0 + kf * 0.5);
        let p = vec3(cos(az + drift) * r, kf * 7.3, sin(az + drift) * r);
        let big = saturate((fbm(p, 3u) - 0.4) * 2.5);
        let fine = fbm(p * 11.0, 3u);
        let top = (0.015 + (0.10 + 0.05 * kf) * big * big * (3.0 - 2.0 * big) + 0.04 * (fine - 0.35) * sqrt(big)) * storm;
        let edge = top - e;
        if (edge <= 0.0 || big <= 0.0) {
            continue;
        }
        let alpha = smoothstep(0.0, 0.006, edge);
        // Up the mass: a dark flat base, the body, a bright rim at the top
        // lit from above (the moon or the day through the deck).
        let v = saturate(e / max(top, 1e-4));
        let puffs = fbm(vec3(cos(az) * 9.0, e * 40.0 + kf * 3.1, sin(az) * 9.0), 3u);
        // (Darker than the glowing horizon behind: masses against it.)
        var col = horizon * mix(0.25, 0.8, smoothstep(0.0, 1.0, v)) * mix(0.6, 1.3, puffs);
        col += horizon * 0.7 * exp(-edge / 0.008) * mix(0.3, 1.0, fine);
        // The nearest bank darkest, the farthest into the horizon's haze.
        col *= mix(1.0, 0.75, kf / 2.0);
        col = mix(col, horizon, 0.35 * (1.0 - kf / 2.0));
        out = mix(out, col, alpha);
    }
    return out;
}

// How stormy (0..1): the storm towers' height says.
fn storm_of() -> f32 {
    return saturate(sky.cloud_extra / 3400.0);
}

// Stars: points on a fine grid of directions, twinkling.
fn stars(d: vec3<f32>) -> vec3<f32> {
    let q = d * 300.0;
    let cell = vec3<i32>(floor(q));
    let r = hash(cell);
    if (r < 0.985) {
        return vec3(0.0);
    }
    let centre = vec3<f32>(cell) + 0.5 + (vec3(hash(cell + 7), hash(cell + 13), hash(cell + 29)) - 0.5) * 0.6;
    let fall = exp(-dot(q - centre, q - centre) * 18.0);
    let twinkle = 0.75 + 0.25 * sin(sky.time * (2.0 + 4.0 * hash(cell + 3)) + r * 50.0);
    let tint = mix(vec3(0.75, 0.85, 1.0), vec3(1.0, 0.9, 0.75), hash(cell + 5));
    return tint * fall * twinkle * (r - 0.985) * 120.0;
}

// The bolt: a jagged line from the cloud base down toward the horizon
// beside `bolt_dir`, brightest in its core.
fn bolt(d: vec3<f32>) -> f32 {
    let b = normalize(sky.bolt_dir);
    var da = atan2(d.x, d.z) - atan2(b.x, b.z);
    da = da - round(da / (2.0 * PI)) * 2.0 * PI;
    let top = max(b.y, 0.12);
    if (d.y > top || d.y < -0.02 || abs(da) > 0.2) {
        return 0.0;
    }
    let e = (top - d.y) / top;
    // Jagged: a few octaves of steps sideways as it falls.
    var x = 0.0;
    var amp = 0.03;
    var f = 6.0;
    for (var k = 0; k < 4; k += 1) {
        let s = e * f + sky.bolt_seed * 17.0 + f32(k) * 3.1;
        x += (fract(sin(floor(s) * 12.9898 + sky.bolt_seed) * 43758.5453) - 0.5) * amp * mix(1.0, fract(s), 0.5);
        amp *= 0.55;
        f *= 2.3;
    }
    let off = abs(da * max(cos(d.y), 0.2) - x);
    return exp(-off / 0.0016) + 0.15 * exp(-off / 0.012);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = vec3(gid.x, gid.y + sky.row_base, gid.z);
    let size = textureDimensions(output);
    let face = sky.face_base + id.z;
    if (id.x >= size.x || id.y >= size.y || face >= 6u) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    let d = cube_dir(uv, face);
    let sun = normalize(sky.sun_dir);
    let key = normalize(sky.key_dir);
    let cs = dot(d, sun);
    let ck = dot(d, key);
    let showcase = sky.physical > 0.5;

    // The sky behind the clouds: the map's colours (or, in the showcase,
    // the physical sky scaled to the skybox's own zenith at the map's
    // hour), its fog at the horizon.
    var base = classic_sky(d);
    let zenith = classic_sky(vec3(0.0, 1.0, 0.0));
    // In the showcase the map's own skybox may be night (Wet Work): the
    // physical sky is scaled to a fixed daylight zenith instead.
    let zenith_lum = select(dot(zenith, vec3(0.2126, 0.7152, 0.0722)), DAY_ZENITH, showcase);
    var horizon = sky.horizon;
    if (showcase) {
        let reference = scatter(vec3(0.0, 1.0, 0.0), normalize(vec3(0.3, 0.75, 0.2)));
        let scale = DAY_ZENITH / max(dot(reference, vec3(0.2126, 0.7152, 0.0722)), 1e-6);
        let moon = normalize(sky.moon_dir);
        let dir = vec3(d.x, max(d.y, 0.0), d.z);
        // (The moon's glow only through clear air: none under storm cloud.)
        let moonlight = MOONLIGHT * (1.0 - storm_of());
        base = scale * (scatter(dir, sun) + moonlight * scatter(dir, moon) * vec3(0.7, 0.8, 1.0));
        base += stars(d) * sky.night * smoothstep(0.0, 0.1, d.y) * zenith_lum;
        // By night: a glow toward the horizon (the clock's horizon colour,
        // shared with the fog and the ocean), darker overhead.
        base = max(base, night_sky(d) * sky.night);
    }
    if (sky.fog > 0.5) {
        base = mix(base, horizon, 0.8 * exp(-max(d.y, 0.0) / 0.05));
        if (d.y < 0.0) {
            base = horizon;
        }
    }
    // The sun's glow in the air round it.
    let glow = sky.sun_color * (0.04 * pow(max(cs, 0.0), 6.0) + 0.25 * pow(max(cs, 0.0), 120.0)) * (1.0 - sky.night);

    let top_height = CLOUD_TOP + sky.cloud_extra;
    var color = vec3(0.0);
    var transmittance = 1.0;
    let cover = coverage();
    if (d.y > 0.0 && cover > 0.0) {
        let t0 = to_shell(d.y, CLOUD_BASE);
        let t1 = to_shell(d.y, top_height);
        // (Half the steps while a flash redraws the whole sky each frame.)
        let steps = select(STEPS, STEPS / 2u, sky.flash > 0.0);
        let dt = (t1 - t0) / f32(steps);
        let jitter = hash(vec3<i32>(vec2<i32>(id.xy), i32(face)));
        // Sigma per metre for a density of 1: thicker when overcast.
        let sigma = mix(0.012, 0.03, cover);
        // (By night a storm deck hides the moon: its light is the deck's glow,
        // which carries the masses and rifts, not a flat moonlit sheet.)
        let key_light = sky.key_color * mix(6.0, 2.5, sky.darkness) * (1.0 - 0.6 * storm_of() * sky.night);
        // The skybox's average hue (Crash's green-grey storm, Bog's brown)
        // on the clouds, so each map keeps its mood.
        let average = (zenith + classic_sky(vec3(1.0, 0.25, 0.0)) + classic_sky(vec3(-1.0, 0.25, 0.0))
            + classic_sky(vec3(0.0, 0.25, 1.0)) + classic_sky(vec3(0.0, 0.25, -1.0))) * 0.2;
        let hue = mix(vec3(1.0), average / max(dot(average, vec3(0.2126, 0.7152, 0.0722)), 1e-3), select(0.85, 0.0, showcase));
        let phase = mix(hg(ck, 0.45), hg(ck, -0.2), 0.3);
        // Where the bolt leaves the cloud base: lightning lights the cloud
        // round it from inside.
        let bolt_dir = normalize(sky.bolt_dir);
        let bolt_at = bolt_dir * to_shell(max(bolt_dir.y, 0.12), CLOUD_BASE + 300.0);
        var t_sum = 0.0;
        var weight = 0.0;
        for (var i = 0u; i < steps; i += 1u) {
            if (transmittance < 0.01) {
                break;
            }
            let t = t0 + (f32(i) + jitter) * dt;
            let p = d * t;
            // Height through the layer, on the round earth.
            let r = length(vec2(length(p.xz), p.y + EARTH)) - EARTH;
            let h = saturate((r - CLOUD_BASE) / (top_height - CLOUD_BASE));
            let dens = density(p, h, cover, true);
            if (dens <= 0.0) {
                continue;
            }
            let s = sigma * dens;
            // Toward the key light through the cloud.
            var depth = 0.0;
            for (var j = 1u; j <= LIGHT_STEPS; j += 1u) {
                let lp = p + key * (f32(j) * 140.0);
                let lr = length(vec2(length(lp.xz), lp.y + EARTH)) - EARTH;
                let lh = saturate((lr - CLOUD_BASE) / (top_height - CLOUD_BASE));
                depth += density(lp, lh, cover, false) * 140.0;
            }
            let od = depth * sigma * (1.0 + sky.darkness);
            // Beer's law, with a brighter tail for light scattered more than once.
            // The "powder" term darkens thin edges facing the sun (a cloud's
            // rim isn't lit from inside), so grazing edges don't glow.
            let powder = 1.0 - exp(-2.0 * od - 0.2);
            let lit = exp(-od) * mix(1.0, powder, 0.7) + 0.3 * exp(-od * 0.25) * (1.0 - exp(-2.0 * s * 100.0));
            // The sky's colour that way, darker under the cloud (and by night).
            var sky_there = mix(classic_sky(d), zenith, 0.5);
            if (showcase) {
                sky_there = mix(base, zenith * (1.0 - sky.night), 0.3);
                // By night the deck's underside takes the horizon's glow:
                // thick masses darker, thin gaps lighter (structure, not a
                // flat grey).
                let thin = exp(-od * 0.6);
                // Big slow-drifting masses (darker) and breaks (lighter).
                let q = p + vec3(sky.wind.x, 0.0, sky.wind.y);
                let mass = smoothstep(0.38, 0.62, fbm(vec3(q.x, 0.0, q.z) * (1.0 / 6000.0), 3u));
                // (Stronger low down, where the eye looks out to sea.)
                let low = 1.0 - smoothstep(0.1, 0.5, d.y);
                // Rifts: thin bright seams along the masses' edges.
                let rift = 1.0 + 1.5 * low * (1.0 - smoothstep(0.0, 0.12, abs(mass - 0.5)));
                let shade = mix(mix(1.8, 2.6, low), mix(0.25, 0.08, low), mass) * mix(0.6, 1.4, thin) * rift;
                sky_there = mix(sky_there, night_sky(d) * shade, sky.night);
            }
            // By night storm cloud still reads dark slate grey (CoD4's Wet
            // Work sky), lit faintly from above: not black.
            let ambient = sky_there * mix(0.25, 1.0, h) * (1.0 - 0.6 * sky.darkness)
                + vec3(0.42, 0.46, 0.55) * zenith_lum * NIGHT_CLOUD * sky.night * mix(0.6, 1.0, h);
            // Thin wisps scatter evenly; only thick cloud gets the strong
            // forward lobe (no bright ribbon along edges near the sun).
            let ph = mix(1.0 / (4.0 * PI), phase, saturate(dens * 1.5));
            let flash = sky.flash * 5.0 * exp(-distance(p, bolt_at) / 1200.0) * vec3(0.8, 0.85, 1.0) * zenith_lum;
            let light = (key_light * lit * ph + ambient) * hue + flash;
            let step_t = exp(-s * dt);
            color += transmittance * light * (1.0 - step_t);
            t_sum += t * transmittance * (1.0 - step_t);
            weight += transmittance * (1.0 - step_t);
            transmittance *= step_t;
        }
        // The map's own mood and brightness: clouds as bright, on the
        // whole, as its skybox (a night map's clouds don't shine like noon's,
        // a storm's don't darken the frame so the exposure blows out the rest).
        if (!showcase) {
            let lum = vec3(0.2126, 0.7152, 0.0722);
            let typical = (key_light * 0.3 / (4.0 * PI) + mix(average, zenith, 0.5) * 0.6 * (1.0 - 0.6 * sky.darkness)) * hue;
            color *= clamp(dot(average, lum) / max(dot(typical, lum), 1e-4), 0.15, 3.0);
        }
        // Far clouds fade into the horizon's haze.
        if (weight > 0.0) {
            // (A storm's low deck keeps its bands out to the horizon.)
            let far = 1.0 - exp(-(t_sum / weight) / mix(30000.0, 90000.0, storm_of()));
            color = mix(color, horizon * (1.0 - transmittance) + glow * (1.0 - transmittance), far * sky.fog);
        }
        // Clouds thin out toward the horizon, where the haze is.
        let fade = smoothstep(0.0, mix(0.15, 0.04, storm_of()), d.y);
        color *= fade;
        transmittance = mix(1.0, transmittance, fade);
    }

    // The sun's disc, darker at its rim, behind the clouds.
    let angle = acos(clamp(cs, -1.0, 1.0));
    let disc = 1.0 - smoothstep(sky.sun_size * 0.85, sky.sun_size, angle);
    let limb = sqrt(max(1.0 - (angle / sky.sun_size) * (angle / sky.sun_size), 0.0));
    var discs = sky.sun_color * 60.0 * disc * mix(0.6, 1.0, limb) * step(-0.02, d.y) * (1.0 - sky.night);
    if (showcase) {
        // The moon: a pale disc with darker seas.
        let moon = normalize(sky.moon_dir);
        let ma = acos(clamp(dot(d, moon), -1.0, 1.0));
        let moon_disc = 1.0 - smoothstep(sky.sun_size * 0.8, sky.sun_size, ma);
        let seas = 0.75 + 0.25 * value_noise(d * 900.0);
        discs += vec3(0.75, 0.8, 0.9) * 24.0 * moon_disc * seas * sky.night * zenith_lum * (1.0 - storm_of());
    }
    var out = (base + glow + discs) * transmittance + color;
    // A storm's distant cloud: banks of big rounded masses along the
    // horizon, back to front, each darker at its base and lit at its top
    // edge, the nearer ones taller and darker, the farther fading into the
    // horizon (volume in perspective where the raymarch's far layer reads
    // flat).
    if (showcase && storm_of() > 0.0 && d.y > -0.02 && d.y < 0.3) {
        out = cloud_banks(d, out, horizon);
    }
    // The bolt itself, below the cloud base.
    if (sky.flash > 0.0) {
        out += vec3(0.85, 0.9, 1.0) * bolt(d) * sky.flash * 30.0 * zenith_lum;
    }
    textureStore(output, vec2<i32>(id.xy), i32(face), vec4(out, 1.0));
}
