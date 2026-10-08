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
    if (lights.n_directional_lights == 0u) {
        return vec4(0.0);
    }
    let light = &lights.directional_lights[0];
    if (((*light).flags & DIRECTIONAL_LIGHT_FLAGS_VOLUMETRIC_BIT) == 0u) {
        return vec4(0.0);
    }

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

        let cascade = get_cascade_index(0u, rd_view_z * t);
        let local = world_to_directional_light_local(0u, cascade, vec4(p + depth_offset, 1.0));
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
    if (depth <= 0.0) {
        return vec4(color * 0.3, 0.0);
    }
    return vec4(color, 1.0 - transmittance);
}
