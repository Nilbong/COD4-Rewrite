// Sun shafts (`crate::atmos::volumetric`), in place of Bevy's volumetric
// fog shader: same bindings and uniform, our own raymarch.
//
// Each pixel marches from the eye to the nearest of: the surface it sees,
// or how far the shafts reach (the uniform's `density_texture_offset.x`,
// within the sun's shadow cascades). Steps are packed toward the eye
// (quadratic), where shafts through windows and doorways are; each looks
// the point up in the sun's shadow map. The air thins with height above
// the map's floor. Only the sun (the first directional light) scatters:
// Bevy's version also walked the clustered point lights at every step.
//
// Output: the sun's light scattered toward the eye, and how much of the
// scene behind the air hides (kept small: the distance fog does that).

#import bevy_pbr::mesh_view_bindings::{globals, lights, view}
#import bevy_pbr::mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_VOLUMETRIC_BIT
#import bevy_pbr::shadow_sampling::sample_shadow_map_hardware
#import bevy_pbr::shadows::{get_cascade_index, world_to_directional_light_local}
#import bevy_pbr::utils::interleaved_gradient_noise
#import bevy_pbr::view_transformations::{frag_coord_to_ndc, position_ndc_to_world, direction_world_to_view}
#import bevy_pbr::mesh_view_bindings as view_bindings
#import bevy_pbr::clustered_forward::{view_fragment_cluster_index, unpack_clusterable_object_index_ranges, get_clusterable_object_id}

// The local lights' glow in the air along the ray from `eye` (`rd`, out to
// `t_end`): each point and spot light near the eye (the clustered lights:
// fires, lamps, flares, flashes) scatters toward the eye in closed form,
// the 1/d² falloff integrated along the ray (no shadows), clipped to its
// range; a spot's only inside its cone. `gain` is `volumetric.local`.
fn local_glow(eye: vec3<f32>, rd: vec3<f32>, t_end: f32, frag: vec2<f32>, density: f32, gain: f32) -> vec3<f32> {
    if (gain <= 0.0) {
        return vec3(0.0);
    }
    let view_z = direction_world_to_view(rd).z * min(t_end, 30.0) * 0.5;
    let cluster = view_fragment_cluster_index(frag, view_z, false);
    let ranges = unpack_clusterable_object_index_ranges(cluster);
    var sum = vec3(0.0);
    // (Debug: `volumetric.local` 99 shows how many lights the pass sees,
    // a tenth red each.)
    if (gain > 98.0) {
        return vec3(f32(ranges.first_reflection_probe_index_offset - ranges.first_point_light_index_offset) * 0.1, 0.0, 0.05);
    }
    // (Hard-capped: the cluster's counts in this pass aren't trusted; an
    // unbounded walk timed the GPU out, 2026-10-20.)
    let first = ranges.first_point_light_index_offset;
    let last = min(ranges.first_reflection_probe_index_offset, first + 4u);
    if (last <= first) {
        return vec3(0.0);
    }
    for (var i = first; i < last; i = i + 1u) {
        let id = get_clusterable_object_id(i);
        let l = &view_bindings::clustered_lights.data[id];
        let pos = (*l).position_radius.xyz;
        let range = (*l).range;
        let to = pos - eye;
        let s0 = dot(to, rd);
        let h2 = max(dot(to, to) - s0 * s0, 0.04);
        let h = sqrt(h2);
        if (h > range) {
            continue;
        }
        // The ray's stretch inside the light's range.
        let half = sqrt(max(range * range - h2, 0.0));
        let a = clamp(s0 - half, 0.0, t_end);
        let b = clamp(s0 + half, 0.0, t_end);
        if (b <= a) {
            continue;
        }
        var k = (atan((b - s0) / h) - atan((a - s0) / h)) / h;
        if (i >= ranges.first_spot_light_index_offset) {
            // Inside the cone at the closest point (Bevy packs the spot's
            // direction x, z and its scale and offset).
            let c = (*l).light_custom_data;
            let y_sign = select(1.0, -1.0, ((*l).flags & 2u) != 0u);
            let dir = vec3(c.x, y_sign * sqrt(max(1.0 - c.x * c.x - c.y * c.y, 0.0)), c.y);
            let at = eye + rd * clamp(s0, a, b);
            let cd = dot(normalize(at - pos), dir);
            let cone = saturate(cd * c.z + c.w);
            k *= cone * cone;
        }
        sum += (*l).color_inverse_square_range.rgb * k;
    }
    // Soft cap: a lamp's cone glows, never a white wall.
    let v = sum * density * FRAC_4_PI * gain * view.exposure;
    return 0.06 * (vec3(1.0) - exp(-v / 0.06));
}

// The GPU version of Bevy's `VolumetricFog` and `FogVolume` (must match
// `bevy_pbr::volumetric_fog::render::VolumetricFogUniform`).
struct VolumetricFog {
    clip_from_local: mat4x4<f32>,
    uvw_from_world: mat4x4<f32>,
    far_planes: array<vec4<f32>, 6>,
    fog_color: vec3<f32>,
    light_tint: vec3<f32>,
    ambient_color: vec3<f32>,
    ambient_intensity: f32,
    step_count: u32,
    bounding_radius: f32,
    absorption: f32,
    scattering: f32,
    density_factor: f32,
    // Ours: x how far the shafts reach (m), y how fast the air thins with
    // height (per m), z the floor's height.
    density_texture_offset: vec3<f32>,
    scattering_asymmetry: f32,
    light_intensity: f32,
    jitter_strength: f32,
}

@group(1) @binding(0) var<uniform> volumetric_fog: VolumetricFog;

#ifdef MULTISAMPLED
@group(1) @binding(1) var depth_texture: texture_depth_multisampled_2d;
#else
@group(1) @binding(1) var depth_texture: texture_depth_2d;
#endif

#ifdef DENSITY_TEXTURE
@group(1) @binding(2) var density_texture: texture_3d<f32>;
@group(1) @binding(3) var density_sampler: sampler;
#endif

const FRAC_4_PI: f32 = 0.07957747154594767;
// Metres of sunlit air past which the shafts' light levels off.
const SATURATION: f32 = 2.0;
// The most the air may add, as a share of the live light (not lifted): about a quarter of
// what a sunlit wall sends back, so a shaft never hides anyone.
const HAZE_MAX: f32 = 0.045;
// Metres over which the air's share fades with distance from the eye.
const NEAR: f32 = 20.0;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
}

@vertex
fn vertex(vertex: Vertex) -> @builtin(position) vec4<f32> {
    return volumetric_fog.clip_from_local * vec4<f32>(vertex.position, 1.0);
}

fn henyey_greenstein(cos_theta: f32, g: f32) -> f32 {
    let denom = 1.0 + g * g - 2.0 * g * cos_theta;
    return FRAC_4_PI * (1.0 - g * g) / (denom * sqrt(denom));
}

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // The volumetric sun: the directional light flagged so (the viewmodel's
    // own sun may come first in the list). Without one (night, a held
    // clock), only the local lights glow.
    var sun_id = 0xffffffffu;
    for (var k = 0u; k < lights.n_directional_lights; k += 1u) {
        if ((lights.directional_lights[k].flags & DIRECTIONAL_LIGHT_FLAGS_VOLUMETRIC_BIT) != 0u) {
            sun_id = k;
            break;
        }
    }
    if (sun_id == 0xffffffffu) {
        let eye0 = view.world_position;
        let far0 = position_ndc_to_world(vec3(frag_coord_to_ndc(vec4(position.xy, 1.0, 1.0)).xy, 1.0));
        let rd0 = normalize(far0 - eye0);
        let depth0 = textureLoad(depth_texture, vec2<i32>(position.xy), 0);
        var t0 = 40.0;
        if (depth0 > 0.0) {
            let surface0 = position_ndc_to_world(frag_coord_to_ndc(vec4(position.xy, depth0, 1.0)));
            t0 = min(t0, distance(surface0, eye0));
        }
        return vec4(local_glow(eye0, rd0, t0, position.xy, volumetric_fog.density_factor, volumetric_fog.ambient_intensity), 0.0);
    }
    let light = &lights.directional_lights[sun_id];

    let reach = volumetric_fog.density_texture_offset.x;
    let thinning = volumetric_fog.density_texture_offset.y;
    let floor_height = volumetric_fog.density_texture_offset.z;

    // The ray: from the eye through this pixel, to the surface or `reach`.
    let eye = view.world_position;
    let far_world = position_ndc_to_world(vec3(frag_coord_to_ndc(vec4(position.xy, 1.0, 1.0)).xy, 1.0));
    let rd = normalize(far_world - eye);
    let depth = textureLoad(depth_texture, vec2<i32>(position.xy), 0);
    var t_end = reach;
    if (depth > 0.0) {
        let surface = position_ndc_to_world(frag_coord_to_ndc(vec4(position.xy, depth, 1.0)));
        t_end = min(t_end, distance(surface, eye));
    }
    // View-space depth per metre along the ray (for the shadow cascade).
    let rd_view_z = direction_world_to_view(rd).z;

    let L = (*light).direction_to_light.xyz;
    let cos_theta = dot(L, rd);
    // Mostly forward scattering, with some isotropic so shafts still show
    // looking away from the sun.
    let phase = mix(FRAC_4_PI, henyey_greenstein(cos_theta, volumetric_fog.scattering_asymmetry), 0.6);
    let depth_offset = (*light).shadow_depth_bias * L;

    // A fixed dither (no TAA to smooth a moving one), or a moving one if
    // `jitter_strength` is set.
    let frame = select(0u, globals.frame_count, volumetric_fog.jitter_strength > 0.0);
    let dither = interleaved_gradient_noise(position.xy, frame);

    let n = max(volumetric_fog.step_count, 1u);
    let inv_n = 1.0 / f32(n);
    let sigma = volumetric_fog.density_factor;
    // Extinction per metre (kept small: the distance fog hides the far).
    let extinction = sigma * volumetric_fog.absorption;
    var transmittance = 1.0;
    // Metres of sunlit air along the ray (thinned with height).
    var lit_length = 0.0;
    // ... and metres of air at all, weighted alike.
    var air_length = 0.0;
    for (var i = 0u; i < n; i += 1u) {
        let s0 = (f32(i) + dither) * inv_n;
        let s1 = min(s0 + inv_n, 1.0);
        let t = t_end * s0 * s0;
        let dt = t_end * (s1 * s1 - s0 * s0);
        let p = eye + rd * t;
        let thin = exp(-max(p.y - floor_height, 0.0) * thinning);

        let cascade = get_cascade_index(sun_id, rd_view_z * t);
        let local = world_to_directional_light_local(sun_id, cascade, vec4(p + depth_offset, 1.0));
        var lit = 1.0;
        if (local.w != 0.0) {
            lit = sample_shadow_map_hardware(local.xy, local.z, i32((*light).depth_texture_base_index + cascade));
        }
        // Near air counts most: shafts in the room or street you're in,
        // never a haze that hides someone at mid range.
        let near = exp(-t / NEAR);
        lit_length += transmittance * thin * near * dt * lit;
        air_length += transmittance * thin * near * dt;
        transmittance *= exp(-thin * extinction * dt);
    }
    // Light in the air builds up as it would over a shaft's few metres,
    // then levels off: a shaft through a window keeps its strength while
    // open sunlit air (outdoors, toward the sky) doesn't become a wall of
    // haze.
    let scattered = sigma * SATURATION * (1.0 - exp(-lit_length / SATURATION));

    // (`light_intensity` scales the sun up to full daylight: most of
    // CoD4's sun is baked into the lightmaps, leaving the live light weak.)
    let sun = (*light).color.rgb * volumetric_fog.fog_color * volumetric_fog.light_tint * volumetric_fog.light_intensity * view.exposure;
    // Shafts, not a veil: air lit all along the ray (open sunlit streets)
    // adds little, so the light gathers where shade breaks it up
    // (through windows, doorways, roof holes, under foliage).
    let lit_share = lit_length / max(air_length, 1e-4);
    let structure = 1.0 - 0.9 * smoothstep(0.25, 0.8, lit_share);
    // Levels off softly at `HAZE_MAX`.
    // Looking toward the sun the god rays (`super::godrays`) give the
    // light; the shadow map is too coarse there (crowns and edges in front
    // of the sun turned to grey veils), so the raymarch steps back.
    let toward_sun = 1.0 - smoothstep(0.45, 0.9, cos_theta);
    let v = scattered * volumetric_fog.scattering * phase * structure * toward_sun;
    // (The cap is the map's full daylight's, whatever lifts the light.)
    let color = HAZE_MAX * (1.0 - exp(-v * volumetric_fog.light_intensity / HAZE_MAX)) * sun / max(volumetric_fog.light_intensity, 1e-4);
    // Nothing solid in the depth buffer (the sky, or blended foliage that
    // writes no depth): a little light and no dimming, so palm crowns and
    // the sky behind them don't turn to haze (the god rays carry the
    // sun-facing light there, `super::godrays`).
    // Lamps, fires and flares glowing in the air (`volumetric.local`, in
    // the camera's otherwise unused `ambient_intensity`).
    let glow = local_glow(eye, rd, t_end, position.xy, sigma, volumetric_fog.ambient_intensity);
    if (depth <= 0.0) {
        return vec4(color * 0.3 + glow, 0.0);
    }
    return vec4(color + glow, 1.0 - transmittance);
}
