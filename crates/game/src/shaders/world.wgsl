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
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
    forward_io::{VertexOutput, FragmentOutput},
}

struct WorldParams {
    falloff_parms: vec4<f32>,
    falloff_begin: vec4<f32>,
    falloff_end: vec4<f32>,
    // x: normal map, y: specular map, z: lightmap, w: lightmap exposure.
    flags: vec4<f32>,
    // x: reflection probe, y: its last mip level.
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
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> params: WorldParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var normal_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var normal_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var specular_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var specular_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var lightmap: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var lightmap_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var reflection_probe: texture_cube<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var probe_sampler: sampler;

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
        let pr = textureSampleLevel(reflection_probe, probe_sampler, vec3(r.x, r.y, abs(r.z)), 0.0);
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

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    if (params.water_color.w > 0.5) {
        let w = water(in);
        var out: FragmentOutput;
        out.color = vec4(pow(w.rgb, vec3(2.2)) * params.flags.w * view.exposure, w.a);
        out.color = main_pass_post_lighting_processing(pbr_input, out.color);
        return out;
    }
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    // Tangent-space normal, flat unless there's a normal map.
    var n_t = vec3(0.0, 0.0, 1.0);
    // Probe mip for reflections: IW3 used 6 - 8 * gloss.
    var probe_lod = pbr_input.material.perceptual_roughness * params.probe.y;
#ifdef VERTEX_UVS_A
#ifdef VERTEX_TANGENTS
    if (params.flags.x > 0.5) {
        let s = textureSample(normal_map, normal_sampler, in.uv);
        n_t = normalize(iw3_slope(s.a, s.g));
        let n = pbr_input.world_normal;
        let t = normalize(in.world_tangent.xyz);
        let b = in.world_tangent.w * cross(n, t);
        pbr_input.N = normalize(t * n_t.x + b * n_t.y + n * n_t.z);
    }
#endif
    if (params.flags.y > 0.5) {
        // Gloss is a Blinn-Phong power of 2^(9.3775 g) + 7; convert to GGX.
        let s = textureSample(specular_map, specular_sampler, in.uv);
        let power = exp2(s.a * 9.3775) + 7.0;
        probe_lod = clamp(6.0 - 8.0 * s.a, 0.0, params.probe.y);
        pbr_input.material.perceptual_roughness = sqrt(sqrt(2.0 / (power + 2.0)));
        // Bevy's dielectric F0 is 0.16 * reflectance^2.
        pbr_input.material.reflectance = sqrt(s.rgb / 0.16);
    }
#endif

    // The baked light, added here (without Bevy's `Lightmap`, whose per-slab
    // bind groups the render thread rebuilt every frame) or, with it
    // (`LIGHTMAP`), through Bevy's lightmap path.
    var baked = vec3(0.0);
    let ao = pbr_input.diffuse_occlusion;
#ifdef VERTEX_UVS_B
    if (params.flags.z > 0.5) {
        let a = textureSample(lightmap, lightmap_sampler, vec2(in.uv_b.x, in.uv_b.y * 0.5));
        let b = textureSample(lightmap, lightmap_sampler, vec2(in.uv_b.x, in.uv_b.y * 0.5 + 0.5));
        let l = normalize(iw3_slope(a.a, b.a));
        var lit = a.rgb * n_t.z + b.rgb * saturate(dot(n_t, l));
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
#endif
        if (params.probe.x > 0.5) {
            let v = normalize(view.world_position - in.world_position.xyz);
            let n = pbr_input.N;
            let r = reflect(-v, n);
            // Probes are looked up with CoD axes: Bevy (x, y, z) is CoD (x, -z, y).
            let p = textureSampleLevel(reflection_probe, probe_sampler, vec3(r.x, -r.z, r.y), probe_lod);
            // rgb * a peaks at ~0.5 (the sky), so alpha is a 2x overbright
            // scale; doubling it matches the probes to the skybox.
            let radiance = pow(p.rgb * p.a * 2.0, vec3(2.2)) * params.flags.w;
            let f0 = 0.16 * pbr_input.material.reflectance * pbr_input.material.reflectance;
            let f_ab = F_AB(pbr_input.material.perceptual_roughness, max(dot(n, v), 1e-4));
            let ao = min(pbr_input.specular_occlusion, dot(pbr_input.diffuse_occlusion, vec3(1.0 / 3.0)));
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
