// World surfaces into the deferred G-buffer, for ray-traced lighting
// (`crate::rtgi`): the standard material's G-buffer with IW3's normal maps
// (as `world.wgsl` reads them) and the baked lightmap, scaled down to a
// floor, as the light Bevy carries in the G-buffer's emissive channel. Solari
// adds that to what it traces, so rooms the sun and sky don't reach keep
// their baked light.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
    prepass_io::{VertexOutput, FragmentOutput},
    pbr_deferred_functions::deferred_output,
    mesh_view_bindings::view,
}

// As `world.wgsl`'s; only the flags are read here.
struct WorldParams {
    falloff_parms: vec4<f32>,
    falloff_begin: vec4<f32>,
    falloff_end: vec4<f32>,
    // x: normal map, y: specular map, z: lightmap, w: lightmap exposure.
    flags: vec4<f32>,
    probe: vec4<f32>,
    water_color: vec4<f32>,
    water_env: vec4<f32>,
    sun_dir: vec4<f32>,
    sun_diffuse: vec4<f32>,
    dist_falloff: vec4<f32>,
    // x: how much of the lightmap the traced light sits on.
    traced: vec4<f32>,
    // Detail map: tiling, 1 if on, its last mip.
    detail: vec4<f32>,
}

#ifdef BINDLESS
// As `world.wgsl`'s bindless path.
#import bevy_pbr::mesh_bindings::mesh
#import bevy_render::bindless::{bindless_samplers_filtering, bindless_textures_2d}
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
fn sample_lightmap(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(bindless_textures_2d[world.lightmap], bindless_samplers_filtering[world.lightmap_sampler], uv);
}
#else
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> params: WorldParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var normal_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var normal_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var lightmap: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var lightmap_sampler: sampler;

fn load_world_material(in: VertexOutput) {}
fn sample_normal(uv: vec2<f32>) -> vec4<f32> {
    return sample_normal(uv);
}
fn sample_lightmap(uv: vec2<f32>) -> vec4<f32> {
    return sample_lightmap(uv);
}
#endif

fn iw3_slope(x: f32, y: f32) -> vec3<f32> {
    return vec3(x * 4.08 - 2.08, y * 4.0645161 - 2.0645161, 1.0);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    load_world_material(in);
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var n_t = vec3(0.0, 0.0, 1.0);
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
#endif

#ifdef VERTEX_UVS_B
    if (params.flags.z > 0.5) {
        let a = sample_lightmap(vec2(in.uv_b.x, in.uv_b.y * 0.5));
        let b = sample_lightmap(vec2(in.uv_b.x, in.uv_b.y * 0.5 + 0.5));
        let l = normalize(iw3_slope(a.a, b.a));
        let lit = a.rgb * n_t.z + b.rgb * saturate(dot(n_t, l));
        // Bevy packs this times the view's exposure, and Solari exposes what
        // it reads again: one exposure undone.
        pbr_input.lightmap_light = pow(lit, vec3(2.2)) * params.flags.w * params.traced.x / view.exposure;
    }
#endif
    return deferred_output(in, pbr_input);
}
