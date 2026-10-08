// World surfaces: Bevy's standard PBR fragment with IW3's surface model on
// top (from the `lm_*_sm3` and `vertcol_simple_fog_foa` shaders):
// - normal maps storing tangent-space x/y slopes (z = 1) in alpha/green;
// - specular maps with the specular colour in rgb and gloss in alpha;
// - the directional lightmap `A * N.z + B * saturate(dot(N, L))`, evaluated
//   with the normal map and darkened by screen-space AO;
// - specular reflections of the surface's baked reflection probe, using
//   Bevy's split-sum environment BRDF;
// - view-angle falloff for `*_falloff_*` materials (fake light beams);
// - distance falloff for `*_distfalloff` ones (HDR portals multiplying what
//   is seen through doorways);
// - water (`wc_water`, from `water_l_sun`): animated waves reflecting the
//   probe through a fresnel term, tinted by the material's water colour,
//   with a sun glint.
// IW3 lit in gamma space, so its results are linearised with pow 2.2.

#import bevy_pbr::shadows::fetch_directional_shadow
#import bevy_pbr::mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT
#import bevy_pbr::{
    lighting::{F_AB, EnvBRDFApprox},
    mesh_view_bindings::{view, globals, lights},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types::{
        STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
        STANDARD_MATERIAL_FLAGS_ALPHA_MODE_RESERVED_BITS,
        STANDARD_MATERIAL_FLAGS_ALPHA_MODE_OPAQUE,
        STANDARD_MATERIAL_FLAGS_ALPHA_MODE_MASK,
    },
    forward_io::{VertexOutput, FragmentOutput},
}

struct WorldParams {
    falloff_parms: vec4<f32>,
    falloff_begin: vec4<f32>,
    falloff_end: vec4<f32>,
    // x: normal map, y: specular map, z: lightmap, w: lightmap exposure.
    flags: vec4<f32>,
    // x: reflection probe, y: its last mip level, z: the sky in its place
    // (the showcase's wet surfaces).
    probe: vec4<f32>,
    // Water: colour (w = 1 for water), envMapParms, sun direction, sun colour.
    water_color: vec4<f32>,
    water_env: vec4<f32>,
    sun_dir: vec4<f32>,
    sun_diffuse: vec4<f32>,
    // Distance falloff: metres scale, offset, the blend's scale, 1 if on.
    dist_falloff: vec4<f32>,
    // Ray-traced lighting's (`world_deferred.wgsl`).
    traced: vec4<f32>,
    // Detail map: tiling, 1 if on, its last mip.
    detail: vec4<f32>,
}

#import bevy_pbr::mesh_bindings::mesh
#import bevy_pbr::pbr_types::PbrInput

#ifdef BINDLESS
// Bindless (`WorldLighting`'s `#[bindless]`): one bind group for many
// materials, so their draws batch. Each slot's indices into Bevy's arrays,
// in binding order 100..110.
#import bevy_render::bindless::{bindless_samplers_filtering, bindless_textures_2d, bindless_textures_cube}
struct WorldIndices {
    params: u32,
    normal_map: u32,
    normal_sampler: u32,
    specular_map: u32,
    specular_sampler: u32,
    lightmap: u32,
    lightmap_sampler: u32,
    reflection_probe: u32,
    probe_sampler: u32,
    detail_map: u32,
    detail_sampler: u32,
    wet_map: u32,
    ssr_history: u32,
    ssr_sampler: u32,
    height_map: u32,
    height_sampler: u32,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(120) var<storage> world_indices: array<WorldIndices>;
@group(#{MATERIAL_BIND_GROUP}) @binding(121) var<storage> world_params: array<WorldParams>;
// This fragment's material (`load_world_material`).
var<private> params: WorldParams;
var<private> world: WorldIndices;

fn load_world_material(in: VertexOutput) {
    let slot = mesh[in.instance_index].material_and_lightmap_bind_group_slot & 0xffffu;
    world = world_indices[slot];
    params = world_params[world.params];
}
fn sample_normal(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(bindless_textures_2d[world.normal_map], bindless_samplers_filtering[world.normal_sampler], uv);
}
fn sample_specular(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(bindless_textures_2d[world.specular_map], bindless_samplers_filtering[world.specular_sampler], uv);
}
fn sample_lightmap(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(bindless_textures_2d[world.lightmap], bindless_samplers_filtering[world.lightmap_sampler], uv);
}
fn sample_probe(dir: vec3<f32>, lod: f32) -> vec4<f32> {
    return textureSampleLevel(bindless_textures_cube[world.reflection_probe], bindless_samplers_filtering[world.probe_sampler], dir, lod);
}
fn sample_detail(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(bindless_textures_2d[world.detail_map], bindless_samplers_filtering[world.detail_sampler], uv);
}
fn sample_detail_level(uv: vec2<f32>, lod: f32) -> vec4<f32> {
    return textureSampleLevel(bindless_textures_2d[world.detail_map], bindless_samplers_filtering[world.detail_sampler], uv, lod);
}
fn load_wet_height(texel: vec2<i32>) -> f32 {
    return textureLoad(bindless_textures_2d[world.wet_map], texel, 0).r;
}
fn sample_ssr_history(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(bindless_textures_2d[world.ssr_history], bindless_samplers_filtering[world.ssr_sampler], uv, 0.0).rgb;
}
fn sample_height(uv: vec2<f32>, lod: f32) -> f32 {
    return textureSampleLevel(bindless_textures_2d[world.height_map], bindless_samplers_filtering[world.height_sampler], uv, lod).r;
}
fn height_size() -> vec2<f32> {
    return vec2<f32>(textureDimensions(bindless_textures_2d[world.height_map]));
}
#else
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> params: WorldParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var normal_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var normal_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var specular_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var specular_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var lightmap: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var lightmap_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var reflection_probe: texture_cube<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var probe_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(109) var detail_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(110) var detail_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(111) var wet_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(112) var ssr_history: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(113) var ssr_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(114) var height_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(115) var height_sampler: sampler;

fn load_world_material(in: VertexOutput) {}
fn sample_normal(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(normal_map, normal_sampler, uv);
}
fn sample_specular(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(specular_map, specular_sampler, uv);
}
fn sample_lightmap(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(lightmap, lightmap_sampler, uv);
}
fn sample_probe(dir: vec3<f32>, lod: f32) -> vec4<f32> {
    return textureSampleLevel(reflection_probe, probe_sampler, dir, lod);
}
fn sample_detail(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(detail_map, detail_sampler, uv);
}
fn sample_detail_level(uv: vec2<f32>, lod: f32) -> vec4<f32> {
    return textureSampleLevel(detail_map, detail_sampler, uv, lod);
}
fn load_wet_height(texel: vec2<i32>) -> f32 {
    return textureLoad(wet_map, texel, 0).r;
}
fn sample_ssr_history(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(ssr_history, ssr_sampler, uv, 0.0).rgb;
}
fn sample_height(uv: vec2<f32>, lod: f32) -> f32 {
    return textureSampleLevel(height_map, height_sampler, uv, lod).r;
}
fn height_size() -> vec2<f32> {
    return vec2<f32>(textureDimensions(height_map));
}
#endif

// IW3's packing of a tangent-space vector's x and y (z = 1): normal maps use
// alpha and green, lightmap directions the two halves' alpha.
fn iw3_slope(x: f32, y: f32) -> vec3<f32> {
    return vec3(x * 4.08 - 2.08, y * 4.0645161 - 2.0645161, 1.0);
}

// The engine's water texture: a tiling height field of travelling waves
// (IW3 builds it each frame from the water's wave spectrum).
fn water_height(p: vec2<f32>, t: f32) -> f32 {
    let k = array<vec2<f32>, 6>(
        vec2(7.0, 3.0), vec2(-5.0, 8.0), vec2(11.0, -4.0), vec2(3.0, -13.0), vec2(-14.0, -6.0), vec2(9.0, 12.0),
    );
    var h = 0.0;
    for (var i = 0; i < 6; i++) {
        // Deep water: speed grows with the square root of the wave number.
        h += sin(6.2831853 * dot(p, k[i]) + 1.4 * sqrt(length(k[i])) * t + f32(i) * 1.7);
    }
    return 0.5 + h / 12.0;
}

// A lightmapped surface's light, baked plus the live sun (Bevy's), eased
// off where it passes what IW3's 8-bit sum could hold. A wall facing a low
// sun (Broadcast's office, through its windows) took the sun almost head
// on on top of its lightmap, past 2x that, and burned out white. `colour`
// is the lit result, `albedo` the diffuse colour, `unit` a lightmap value
// of 1 on screen. Light below the knee (shade, most sunlit ground) is
// untouched; above it the light rises ever slower towards the ceiling (IW3's
// own limit, 1), in gamma space as IW3 summed it.
const LIT_KNEE: f32 = 0.75;
const LIT_CEILING: f32 = 1.0;
fn lit_rolloff(colour: vec3<f32>, albedo: vec3<f32>, unit: f32) -> vec3<f32> {
    let a = max(albedo, vec3(0.02)) * unit;
    // The brightest channel's light, so the hue stays.
    let e = max(max(colour.r / a.r, colour.g / a.g), colour.b / a.b);
    let g = pow(max(e, 0.0), 1.0 / 2.2);
    if (g <= LIT_KNEE) {
        return colour;
    }
    let room = LIT_CEILING - LIT_KNEE;
    let eased = LIT_KNEE + room * (1.0 - exp(-(g - LIT_KNEE) / room));
    return colour * pow(eased / g, 2.2);
}

// Three octaves of the water texture, as `water_l_sun` sums them.
fn water_octaves(p: vec2<f32>, t: f32) -> f32 {
    var h = 0.0;
    var q = p;
    var w = 1.0;
    for (var i = 0; i < 3; i++) {
        h += water_height(q, t) * w;
        q *= 3.7;
        w *= 0.6;
    }
    return h;
}

// IW3's `water_l_sun`, in gamma space. Its normals and vectors are in CoD
// axes (z up); Bevy (x, y, z) is CoD (x, -z, y).
fn water(in: VertexOutput) -> vec4<f32> {
    let t = globals.time;
    var uv = vec2(0.0);
#ifdef VERTEX_UVS_A
    uv = in.uv;
#endif
    let vb = normalize(in.world_position.xyz - view.world_position);
    let v = vec3(vb.x, -vb.z, vb.y);
    // A little parallax from the coarse waves.
    let h0 = water_height(uv * 0.5, t);
    let p = uv + (0.5 - h0) * v.xy * 0.0234375;
    let hc = water_octaves(p, t);
    let hx = water_octaves(p + vec2(0.00390625, 0.0), t);
    let hy = water_octaves(p + vec2(0.0, 0.00390625), t);
    let n = normalize(vec3(hx - hc, hy - hc, 1.0));
    let r = v - 2.0 * dot(v, n) * n;
    var reflected = params.water_color.rgb * 2.0;
    if (params.probe.x > 0.5) {
        let pr = sample_probe(vec3(r.x, r.y, abs(r.z)), 0.0);
        reflected = saturate(pr.rgb * pr.a * 4.0);
    }
    let env = params.water_env;
    let fresnel = saturate(mix(env.x, env.y, pow(1.0 - abs(dot(v, n)), env.z)));
    let base = params.water_color.rgb * n.z;
    var colour = mix(base, reflected, fresnel);
    let s = vec3(params.sun_dir.x, -params.sun_dir.z, params.sun_dir.y);
    let glint = pow(fresnel * max(dot(r, s) + 0.00075, 0.0), 64.0);
    colour += glint * params.sun_diffuse.rgb * env.w;
    var alpha = 1.0;
#ifdef VERTEX_COLORS
    alpha = in.color.a;
#endif
    return vec4(colour, alpha);
}

// ---------------------------------------------------------------------------
// Wet surfaces (the showcase's rain, `crate::wet`): soaked and darker, a
// water film that smooths and reflects, puddles on flat ground with rain
// rippling them, streaks running down walls. The mesh's tag carries how
// wet and how hard it rains; the wet map, where rain reaches.

fn wet_hash(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn wet_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(wet_hash(i), wet_hash(i + vec2(1.0, 0.0)), u.x), mix(wet_hash(i + vec2(0.0, 1.0)), wet_hash(i + vec2(1.0, 1.0)), u.x), u.y);
}

// Rain reaches here (1) or not (0): the wet map's cells (0.5 m, 128 a
// side, wrapping round the camera), blended between neighbours. Beyond
// the map (30 m from the camera) it's taken as open.
const WET_CELL: f32 = 0.5;
const WET_CELLS: i32 = 128;
fn wet_exposure(p: vec3<f32>) -> f32 {
    let to = p.xz - view.world_position.xz;
    if (dot(to, to) > 30.0 * 30.0) {
        return 1.0;
    }
    let g = p.xz / WET_CELL - 0.5;
    let c = vec2<i32>(floor(g));
    let f = fract(g);
    let h00 = load_wet_height(((c % WET_CELLS) + WET_CELLS) % WET_CELLS);
    let h10 = load_wet_height(((c + vec2(1, 0)) % WET_CELLS + WET_CELLS) % WET_CELLS);
    let h01 = load_wet_height(((c + vec2(0, 1)) % WET_CELLS + WET_CELLS) % WET_CELLS);
    let h11 = load_wet_height(((c + vec2(1, 1)) % WET_CELLS + WET_CELLS) % WET_CELLS);
    let reach = p.y + 0.3;
    let e = vec4(select(0.0, 1.0, h00 < reach), select(0.0, 1.0, h10 < reach), select(0.0, 1.0, h01 < reach), select(0.0, 1.0, h11 < reach));
    return mix(mix(e.x, e.y, f.x), mix(e.z, e.w, f.x), f.y);
}

// Raindrops' rings on standing water: the slope they add (world x, z).
fn rain_ripples(p: vec2<f32>, t: f32) -> vec2<f32> {
    var g = vec2(0.0);
    for (var layer = 0; layer < 2; layer++) {
        let q = p * (3.0 + f32(layer) * 1.7) + f32(layer) * 17.31;
        let c = floor(q);
        let centre = (vec2(wet_hash(c + 11.7), wet_hash(c + 23.1)) - 0.5) * 0.5;
        let phase = fract(t * 1.4 + wet_hash(c));
        let d = fract(q) - 0.5 - centre;
        let dist = length(d);
        let x = (dist - phase * 0.45) * 25.0;
        let fade = (1.0 - phase) * (1.0 - phase);
        g += d / max(dist, 1e-3) * sin(x * 3.0) * exp(-x * x) * fade;
    }
    return g * 0.6;
}

// Makes the surface wet; returns how much of a water film it has (0 dry).
fn wet_surface(in: VertexOutput, pbr: ptr<function, PbrInput>) -> f32 {
    let tag = mesh[in.instance_index].tag;
    if ((tag & 0x10000u) == 0u) {
        return 0.0;
    }
    // Opaque and alpha-tested surfaces only: glass and other blended ones
    // stay as they are (a porthole's glass took the sky's and the screen's
    // reflections as a flat, flickering colour).
    let alpha_mode = (*pbr).material.flags & STANDARD_MATERIAL_FLAGS_ALPHA_MODE_RESERVED_BITS;
    if (alpha_mode != STANDARD_MATERIAL_FLAGS_ALPHA_MODE_OPAQUE && alpha_mode != STANDARD_MATERIAL_FLAGS_ALPHA_MODE_MASK) {
        return 0.0;
    }
    let wetness = f32(tag & 0xffu) / 255.0;
    let rain = f32((tag >> 8u) & 0xffu) / 255.0;
    if (wetness <= 0.0) {
        return 0.0;
    }
    let p = in.world_position.xyz;
    let ng = normalize((*pbr).world_normal);
    // Out in front of the surface: a wall is wet where rain falls before it.
    let w = wetness * wet_exposure(p + ng * 0.6);
    if (w <= 0.001) {
        return 0.0;
    }
    let up = smoothstep(0.75, 0.95, ng.y);
    let vertical = 1.0 - smoothstep(0.3, 0.6, abs(ng.y));
    let rough = (*pbr).material.perceptual_roughness;
    // Rough (porous) surfaces soak and darken more.
    let porosity = smoothstep(0.3, 0.9, rough);
    // Puddles where the ground is flat, spreading as it gets wetter.
    let n = wet_noise(p.xz * 0.35) * 0.65 + wet_noise(p.xz * 1.3) * 0.35;
    let puddle = up * smoothstep(0.98 - w * 0.5, 1.06 - w * 0.5, n + 0.25);
    // Water running down walls, in streaks.
    var streak = 0.0;
    if (vertical > 0.0 && rain > 0.0) {
        let along = dot(p.xz, normalize(vec2(-ng.z, ng.x))) * 6.0;
        let s = wet_noise(vec2(along, p.y * 0.8 + globals.time * 0.6 + wet_hash(vec2(floor(along), 3.0)) * 10.0));
        streak = vertical * smoothstep(0.55, 0.8, s) * rain * w;
    }
    let soak = w * mix(0.25, 0.55, porosity) + streak * 0.15;
    // Standing water hides the surface under it: mostly reflection there.
    let under_water = 1.0 - 0.65 * puddle;
    (*pbr).material.base_color = vec4((*pbr).material.base_color.rgb * (1.0 - soak) * under_water, (*pbr).material.base_color.a);
    // The film: smoother, with water's reflectance (F0 0.02).
    let film = clamp(w * mix(0.5, 0.8, up) + streak * 0.5, 0.0, 1.0);
    let water = max(film, puddle);
    (*pbr).material.perceptual_roughness = mix(rough, min(rough, mix(0.25, 0.04, puddle)), water);
    (*pbr).material.reflectance = mix((*pbr).material.reflectance, vec3(0.35), water);
    // Standing water lies flat over the surface's bumps, and rain rings it.
    var n_out = normalize(mix((*pbr).N, ng, puddle));
    if (rain > 0.0 && up > 0.0) {
        let r = rain_ripples(p.xz, globals.time) * rain * mix(0.15, 1.0, puddle) * up;
        n_out = normalize(n_out + vec3(r.x, 0.0, r.y));
    }
    (*pbr).N = n_out;
    return water;
}

// ---------------------------------------------------------------------------
// Screen-space reflections (the showcase, `crate::ssr`): the reflected ray
// marched through the depth prepass in screen space; where it passes
// behind a surface, that pixel of the world camera's last image. Returns
// the colour (exposed, as on screen) and how much to trust it (0: nothing
// found, keep the probe). Smooth surfaces only; fades at the screen's
// edges and toward the ray's end.

const SSR_REACH: f32 = 30.0;
// `crate::player::SKY_BRIGHTNESS`: the skybox's brightness (cd/m²).
const SKY_BRIGHTNESS: f32 = 800.0;
const SSR_MAX_STEPS: f32 = 40.0;

fn screen_reflection(origin: vec3<f32>, r: vec3<f32>, roughness: f32, frag: vec2<f32>) -> vec4<f32> {
#ifdef DEPTH_PREPASS
    let smooth_enough = 1.0 - smoothstep(0.2, 0.45, roughness);
    if (smooth_enough <= 0.0) {
        return vec4(0.0);
    }
    let near = view.clip_from_view[3][2];
    let start = view.clip_from_world * vec4(origin, 1.0);
    var end = view.clip_from_world * vec4(origin + r * SSR_REACH, 1.0);
    // Stop the ray at the near plane.
    if (end.w < near) {
        end = mix(start, end, (start.w - near) / max(start.w - end.w, 1e-5));
    }
    let s = start.xyz / start.w;
    let e = end.xyz / end.w;
    let size = view.viewport.zw;
    let s_uv = s.xy * vec2(0.5, -0.5) + 0.5;
    let e_uv = e.xy * vec2(0.5, -0.5) + 0.5;
    let steps = clamp(length((e_uv - s_uv) * size) / 6.0, 8.0, SSR_MAX_STEPS);
    let jitter = wet_hash(frag);
    var lo = 0.0;
    for (var i = 1.0; i <= steps; i += 1.0) {
        let t = (i - 1.0 + jitter) / steps;
        let uv = mix(s_uv, e_uv, t);
        if (any(uv < vec2(0.0)) || any(uv > vec2(1.0))) {
            return vec4(0.0);
        }
        // Depth (reverse Z) runs straight in screen space.
        let ray_z = mix(s.z, e.z, t);
        let scene_z = textureLoad(bevy_pbr::mesh_view_bindings::depth_prepass_texture, vec2<i32>(uv * size), 0);
        if (scene_z > ray_z) {
            // Behind something: within its assumed thickness, a hit.
            let ray_d = near / max(ray_z, 1e-7);
            let scene_d = near / max(scene_z, 1e-7);
            if (ray_d - scene_d < 0.4 + 0.03 * scene_d) {
                // Narrow it down between the last step and this one.
                var a = lo;
                var b = t;
                for (var k = 0; k < 4; k++) {
                    let m = (a + b) * 0.5;
                    let muv = mix(s_uv, e_uv, m);
                    let mz = textureLoad(bevy_pbr::mesh_view_bindings::depth_prepass_texture, vec2<i32>(muv * size), 0);
                    if (mz > mix(s.z, e.z, m)) {
                        b = m;
                    } else {
                        a = m;
                    }
                }
                let hit = mix(s_uv, e_uv, b);
                let edge = min(min(hit.x, 1.0 - hit.x), min(hit.y, 1.0 - hit.y));
                let fade = smoothstep(0.0, 0.08, edge) * (1.0 - smoothstep(0.7, 1.0, b)) * smooth_enough;
                return vec4(sample_ssr_history(hit), fade);
            }
        }
        lo = t;
    }
#endif
    return vec4(0.0);
}


// ---------------------------------------------------------------------------
// Surface depth (the showcase, `crate::pom`): parallax occlusion mapping
// on surfaces with a normal map, from heights worked out from it. Near the
// camera only (fading out by 12 m), its heights read at the mip the view
// needs (fading out where they're too small to see), fewer steps face on.

const RELIEF_DEPTH: f32 = 0.02;

fn relief_uv(v: VertexOutput) -> vec2<f32> {
#ifdef VERTEX_UVS_A
#ifdef VERTEX_TANGENTS
    // Height texels a pixel (before any branching: derivatives).
    let texels = max(length(dpdx(v.uv) * height_size()), length(dpdy(v.uv) * height_size()));
    if ((mesh[v.instance_index].tag & 0x10000u) == 0u || params.flags.x < 0.5) {
        return v.uv;
    }
    let to_eye = view.world_position - v.world_position.xyz;
    let dist = length(to_eye);
    let fade = (1.0 - smoothstep(6.0, 12.0, dist)) * (1.0 - smoothstep(6.0, 12.0, texels));
    let lod = log2(max(texels, 1.0));
    if (fade <= 0.0) {
        return v.uv;
    }
    let e = to_eye / dist;
    let n = normalize(v.world_normal);
    let t = normalize(v.world_tangent.xyz);
    let b = v.world_tangent.w * cross(n, t);
    let ts = vec3(dot(e, t), dot(e, b), dot(e, n));
    if (ts.z < 0.08) {
        return v.uv;
    }
    let layers = mix(24.0, 6.0, ts.z);
    let layer = 1.0 / layers;
    let delta = RELIEF_DEPTH * fade * layer * vec2(-ts.x, ts.y) / ts.z;
    var uv = v.uv;
    var depth = 0.0;
    var surface = 1.0 - sample_height(uv, lod);
    for (var i = 0.0; i < layers && surface > depth; i += 1.0) {
        depth += layer;
        uv += delta;
        surface = 1.0 - sample_height(uv, lod);
    }
    // Between the last two layers, where the ray crossed the surface.
    let before = uv - delta;
    let after_gap = surface - depth;
    let before_gap = (1.0 - sample_height(before, lod)) - depth + layer;
    return mix(uv, before, after_gap / min(after_gap - before_gap, -1e-5));
#else
    return v.uv;
#endif
#else
    return vec2(0.0);
#endif
}

@fragment
fn fragment(vertex: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    load_world_material(vertex);
    var in = vertex;
#ifdef VERTEX_UVS_A
    in.uv = relief_uv(vertex);
#endif
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    if (params.water_color.w > 0.5) {
        let w = water(in);
        var out: FragmentOutput;
        out.color = vec4(pow(w.rgb, vec3(2.2)) * params.flags.w * view.exposure, w.a);
        out.color = main_pass_post_lighting_processing(pbr_input, out.color);
        return out;
    }
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
#ifdef VERTEX_UVS_A
    if (params.detail.z > 0.5) {
        // IW3's detail map multiplies the colour in gamma space, centred on
        // its own average (its smallest mip), so it adds grit up close and
        // fades out with distance as its mips blur to that average.
        let d = sample_detail(in.uv * params.detail.xy).rgb;
        let mean = max(sample_detail_level(vec2(0.5), params.detail.w).rgb, vec3(0.02));
        let k = pow(clamp(d / mean, vec3(0.0), vec3(4.0)), vec3(2.2));
        pbr_input.material.base_color = vec4(pbr_input.material.base_color.rgb * k, pbr_input.material.base_color.a);
    }
#endif

    // Tangent-space normal, flat unless there's a normal map.
    var n_t = vec3(0.0, 0.0, 1.0);
    // Probe mip for reflections: IW3 used 6 - 8 * gloss.
    var probe_lod = pbr_input.material.perceptual_roughness * params.probe.y;
#ifdef VERTEX_UVS_A
#ifdef VERTEX_TANGENTS
    if (params.flags.x > 0.5) {
        let s = sample_normal(in.uv);
        n_t = normalize(iw3_slope(s.a, s.g));
        let n = pbr_input.world_normal;
        let t = normalize(in.world_tangent.xyz);
        let b = in.world_tangent.w * cross(n, t);
        pbr_input.N = normalize(t * n_t.x + b * n_t.y + n * n_t.z);
    }
#endif
    if (params.flags.y > 0.5) {
        // Gloss is a Blinn-Phong power of 2^(9.3775 g) + 7; convert to GGX.
        let s = sample_specular(in.uv);
        let power = exp2(s.a * 9.3775) + 7.0;
        probe_lod = clamp(6.0 - 8.0 * s.a, 0.0, params.probe.y);
        pbr_input.material.perceptual_roughness = sqrt(sqrt(2.0 / (power + 2.0)));
        // Bevy's dielectric F0 is 0.16 * reflectance^2.
        pbr_input.material.reflectance = sqrt(s.rgb / 0.16);
    }
#endif

    // Showcase rain.
    let wet = wet_surface(in, &pbr_input);
    if (wet > 0.0) {
        probe_lod = min(probe_lod, pbr_input.material.perceptual_roughness * params.probe.y);
    }

    // The baked light, added here (without Bevy's `Lightmap`, whose per-slab
    // bind groups the render thread rebuilt every frame) or, with it
    // (`LIGHTMAP`), through Bevy's lightmap path.
    var baked = vec3(0.0);
    let ao = pbr_input.diffuse_occlusion;
#ifdef VERTEX_UVS_B
    if (params.flags.z > 0.5) {
        let a = sample_lightmap(vec2(in.uv_b.x, in.uv_b.y * 0.5));
        let b = sample_lightmap(vec2(in.uv_b.x, in.uv_b.y * 0.5 + 0.5));
        var l = normalize(iw3_slope(a.a, b.a));
        // Re-baked (`crate::bake`): linear light, the direction's x and y.
        let rebaked = params.traced.z > 0.5;
        if (rebaked) {
            l = vec3(a.a, b.a, sqrt(max(1.0 - a.a * a.a - b.a * b.a, 0.0)));
        }
        var lit = a.rgb * n_t.z + b.rgb * saturate(dot(n_t, l));
        if (rebaked) {
            // To IW3's gamma space, so the rest (the live sun's sum and its
            // soft limit) runs as for CoD4's lightmaps.
            lit = pow(max(lit, vec3(0.0)), vec3(1.0 / 2.2));
        }
        if (params.traced.y > 0.5 && params.water_color.w < 0.5) {
            // CoD4's `lm_sm_sun`: the live sun added to the baked light in
            // gamma space, shadowed, before linearising.
            let ndl = saturate(dot(pbr_input.N, params.sun_dir.xyz));
            var shadow = 0.0;
            if (ndl > 0.0) {
                // The map's sun among the view's directional lights (the
                // viewmodels' suns come first in some frames): the one along
                // this sun's direction that casts shadows.
                let view_z = dot(vec4(view.view_from_world[0].z, view.view_from_world[1].z, view.view_from_world[2].z, view.view_from_world[3].z), in.world_position);
                shadow = 1.0;
                for (var i: u32 = 0u; i < lights.n_directional_lights; i = i + 1u) {
                    let light = &lights.directional_lights[i];
                    if (((*light).flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u
                        && dot((*light).direction_to_light, params.sun_dir.xyz) > 0.999) {
                        shadow = fetch_directional_shadow(i, in.world_position, in.world_normal, view_z, in.position.xy);
                        break;
                    }
                }
            }
            lit += params.sun_diffuse.rgb * ndl * shadow;
            // IW3 summed these into an 8-bit target, where a sun-facing,
            // sunlit wall stopped at full brightness; unclamped, it ran
            // to 1.8x in linear and burned out (Broadcast's walls). Past 1
            // the sum now rises at a third of the rate: the brightest walls
            // keep their texture, shade and lightmaps below 1 are untouched.
            lit = select(lit, vec3(1.0) + (lit - vec3(1.0)) * 0.35, lit > vec3(1.0));
        }
        baked = pow(lit, vec3(2.2)) * params.flags.w;
    }
#endif
#ifdef LIGHTMAP
    pbr_input.lightmap_light = baked * ao;
#else
    if (params.flags.z > 0.5) {
        // Lightmapped surfaces take no ambient or light grid light: their
        // lightmap has it.
        pbr_input.diffuse_occlusion = vec3(0.0);
    }
#endif

    var out: FragmentOutput;
    // As in Bevy's own pbr.wgsl: `apply_pbr_lighting` ignores the unlit flag.
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
#ifndef LIGHTMAP
        let diffuse_color = pbr_input.material.base_color.rgb * (1.0 - pbr_input.material.metallic);
        out.color = vec4(out.color.rgb + baked * ao * diffuse_color * view.exposure, out.color.a);
        if (params.flags.z > 0.5 && params.water_color.w < 0.5) {
            out.color = vec4(lit_rolloff(out.color.rgb, diffuse_color, params.flags.w * view.exposure), out.color.a);
        }
#endif
        // Only materials with a specular map reflect, as in IW3 (its
        // diffuse-only techniques have no probe term): matte plaster and
        // ceilings took a sky-blue sheen from the probe otherwise.
        if ((params.probe.x > 0.5 && params.flags.y > 0.5) || wet > 0.0) {
            let v = normalize(view.world_position - in.world_position.xyz);
            let n = pbr_input.N;
            let r = reflect(-v, n);
            var radiance = vec3(0.0);
            if (params.probe.x > 0.5) {
                // Probes are looked up with CoD axes: Bevy (x, y, z) is CoD (x, -z, y).
                let p = sample_probe(vec3(r.x, -r.z, r.y), probe_lod);
                // rgb * a peaks at ~0.5 (the sky), so alpha is a 2x overbright
                // scale; doubling it matches the probes to the skybox.
                radiance = pow(p.rgb * p.a * 2.0, vec3(2.2)) * params.flags.w;
            } else if (params.probe.z > 0.5) {
                // The sky, as the skybox draws it (`crate::player`'s brightness).
                radiance = sample_probe(vec3(r.x, -r.z, r.y), 0.0).rgb * SKY_BRIGHTNESS;
            }
            // The probes (and the sky) were baked at CoD4's hour: in the
            // showcase they follow its sky's light (mesh tag bits 24-31).
            let tag = mesh[in.instance_index].tag;
            radiance *= select(1.0, f32(tag >> 24u) / 255.0, (tag & 0x10000u) != 0u);
            // The showcase's screen-space reflections over the probe, on
            // wet and smooth surfaces.
            if (wet > 0.0) {
                let found = screen_reflection(in.world_position.xyz, r, pbr_input.material.perceptual_roughness, in.position.xy);
                radiance = mix(radiance, found.rgb / max(view.exposure, 1e-6), found.a);
            }
            let f0 = 0.16 * pbr_input.material.reflectance * pbr_input.material.reflectance;
            let f_ab = F_AB(pbr_input.material.perceptual_roughness, max(dot(n, v), 1e-4));
            // (`ao`: the occlusion before lightmapped surfaces' was zeroed
            // to keep the ambient light off them.)
            let ao = min(pbr_input.specular_occlusion, dot(ao, vec3(1.0 / 3.0)));
            let specular = radiance * EnvBRDFApprox(f0, f_ab) * ao;
            out.color = vec4(out.color.rgb + specular * view.exposure, out.color.a);
        }
    } else {
        out.color = pbr_input.material.base_color;
    }
    if (params.falloff_parms.z != 0.0) {
        let to_surface = normalize(in.world_position.xyz - view.world_position);
        let c = dot(to_surface, normalize(in.world_normal));
        let t = saturate(c * c * params.falloff_parms.z + params.falloff_parms.w);
        // IW3 applied this in gamma space.
        let fade = pow(t * mix(params.falloff_end.rgb, params.falloff_begin.rgb, t), vec3(2.2));
        out.color = vec4(out.color.rgb * fade, out.color.a);
    }
    if (params.dist_falloff.w > 0.5) {
        // The colour multiplies the scene (the material blends by
        // multiplying), in gamma space as IW3 did.
        let d = distance(in.world_position.xyz, view.world_position);
        let t = saturate(d * params.dist_falloff.x + params.dist_falloff.y);
        let c = mix(params.falloff_end.rgb, params.falloff_begin.rgb, t) * params.dist_falloff.z;
        out.color = vec4(pow(c, vec3(2.2)) * pbr_input.material.base_color.rgb, 1.0);
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
