// Gun surfaces: weapon camo, from IW3's `lp_*_r0c0d0s0` pixel shaders: the
// detail (camo) map, tiled by `detailScale`, is added to the colour map
// centred on grey (`color + detail - 0.5`), in gamma space, before lighting.
// Black Ops' `colorDetailMap` is the same, masked by the colour map's alpha.
// Then IW3's model specular (`lp_sun_r0c0s0`): the specular map's colour
// times a fresnel term (`envMapParms`: min, max, power) times the
// reflection probe, sampled sharper the glossier. (IW3 adds a sun glint
// where the sun reaches the gun; without its shadowing here, that would
// glint in the shade too, so it's left out.)
// Beyond CoD4 (`look`, off with COD4RW_GUNLOOK=cod4): the camo layer with
// more contrast and colour, and the specular map's gloss and colour setting
// the lights' own highlights (shadowed, unlike IW3's glint), so painted
// metal catches the sun.

#import bevy_pbr::{
    mesh_view_bindings::view,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> detail_scale: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var detail_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var detail_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var specular_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var specular_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var reflection_probe: texture_cube<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var probe_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var<uniform> env: vec4<f32>;
// x: specular map, y: probe, z: probe's last mip, w: lightmap exposure.
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var<uniform> shine: vec4<f32>;
// x: camo contrast, y: camo saturation, z: gloss in the lights' highlights.
@group(#{MATERIAL_BIND_GROUP}) @binding(109) var<uniform> look: vec4<f32>;

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3(0.299, 0.587, 0.114));
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    // Black Ops' gold: where it covers the gun.
    var gold = 0.0;
#ifdef VERTEX_UVS_A
    if (detail_scale.w > 0.5) {
        let detail = textureSample(detail_map, detail_sampler, in.uv * detail_scale.xy).rgb;
        let base = pow(pbr_input.material.base_color.rgb, vec3(1.0 / 2.2));
        var camo = pow(detail, vec3(1.0 / 2.2));
        camo = mix(vec3(luma(camo)), camo, look.y);
        // Black Ops (`detail_scale.z` = 1): only where the colour map's alpha says.
        let mask = select(1.0, pbr_input.material.base_color.a, detail_scale.z > 0.5);
        var color = pow(saturate(base + (camo - 0.5) * look.x * mask), vec3(2.2));
        if (detail_scale.z > 1.5) {
            // Black Ops' own colour map is nearly black (its gold is all
            // reflection, from much brighter probes than ours): CoD4's gold
            // guns' average colour under the shine instead.
            gold = mask;
            color = mix(pbr_input.material.base_color.rgb, pow(vec3(0.30, 0.20, 0.13), vec3(2.2)), mask);
        }
        pbr_input.material.base_color = vec4(color, pbr_input.material.base_color.a);
    }
#endif
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
#ifdef VERTEX_UVS_A
    if (shine.x > 0.5 && look.z > 0.5) {
        // The lights' highlights: glossy where the map is (gold polished),
        // as bright as its colour (dielectric: Bevy's F0 is 0.16 r^2).
        let s = textureSample(specular_map, specular_sampler, in.uv);
        let gloss = mix(s.a, 0.8, gold);
        pbr_input.material.perceptual_roughness = mix(0.85, 0.3, gloss);
        pbr_input.material.reflectance = saturate(sqrt(s.rgb * select(1.0, gold, detail_scale.z > 1.5)) * 1.1);
    }
#endif
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
#ifdef VERTEX_UVS_A
    if (shine.x > 0.5) {
        var s = textureSample(specular_map, specular_sampler, in.uv);
        // Gold (its specular map is gold): as polished as CoD4's gold guns.
        s = vec4(s.rgb * select(1.0, gold, detail_scale.z > 1.5), mix(s.a, 0.48, gold));
        let n = normalize(pbr_input.N);
        let v = normalize(in.world_position.xyz - view.world_position);
        let r = reflect(v, n);
        // Without a map (the menus' previews): a plain studio, bright above
        // and dim below.
        var light = vec3(mix(0.08, 0.45, saturate(r.y * 0.5 + 0.5)));
        if (shine.y > 0.5) {
            // Probes are looked up with CoD axes: Bevy (x, y, z) is CoD (x, -z, y).
            let p = textureSampleLevel(reflection_probe, probe_sampler, vec3(r.x, -r.z, r.y), clamp(6.0 - 8.0 * s.a, 0.0, shine.z));
            light = p.rgb * p.a;
        }
        let fresnel = mix(env.x, env.y, pow(max(1.0 - abs(dot(v, n)), 0.0), env.z));
        // IW3 wrote to an 8-bit target: never more than white.
        let spec = saturate(s.rgb * fresnel * light);
        out.color = vec4(out.color.rgb + pow(spec, vec3(2.2)) * shine.w * view.exposure, out.color.a);
    }
#endif
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
