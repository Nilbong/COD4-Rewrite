// Gun surfaces: weapon camo, from IW3's `lp_*_r0c0d0s0` pixel shaders: the
// detail (camo) map, tiled by `detailScale`, is added to the colour map
// centred on grey (`color + detail - 0.5`), in gamma space, before lighting.
// Black Ops' `colorDetailMap` is the same, masked by the colour map's alpha.
// Then IW3's model specular (`lp_sun_r0c0s0`): the specular map's colour
// times a fresnel term (`envMapParms`: min, max, power) times the
// reflection probe, sampled sharper the glossier. (IW3 adds a sun glint
// where the sun reaches the gun; without its shadowing here, that would
// glint in the shade too, so it's left out.)
// Beyond CoD4 (`look`, off with COD4RW_GUNLOOK=cod4): the camo partly
// painted on rather than only added (CoD4's camos are detail centred on
// grey, so on dark gun metal they barely show), keeping the gun's own wear
// as the colour map against its local average; more contrast and colour;
// a stronger reflection, clamped without bleaching its hue; and polished
// metal (a high fresnel minimum, CoD4's gold guns) reflecting in its own
// colour, as metal does, so gold reads gold and not chrome.

#import bevy_pbr::{
    mesh_view_bindings::view,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
    ambient,
    clustered_forward as clustering,
    lighting::{EnvBRDFApprox, F_AB},
    pbr_functions::calculate_F0,
}
#ifdef IRRADIANCE_VOLUME
#import bevy_pbr::irradiance_volume
#endif

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
// x: camo contrast, y: camo saturation, z: how much camo is painted on,
// w: the reflection's strength (over 1: the improved look's reflection).
@group(#{MATERIAL_BIND_GROUP}) @binding(109) var<uniform> look: vec4<f32>;
// The colour map again, for its local average.
@group(#{MATERIAL_BIND_GROUP}) @binding(110) var color_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(111) var color_sampler: sampler;

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3(0.299, 0.587, 0.114));
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    // Black Ops' gold: where it covers the gun.
    var gold = 0.0;
    var diamond_stud = false;
#ifdef VERTEX_UVS_B
    // The appended facets carry a second UV marker; original gun vertices
    // carry zero. No texture alpha or weapon silhouette is faked.
    diamond_stud = abs(detail_scale.z - 5.0) < 0.5 && in.uv_b.x > 0.5;
#endif
#ifdef VERTEX_UVS_A
    if (detail_scale.w > 0.5 && detail_scale.z > 5.5) {
        // A custom camo (6; 7 only where the colour map's alpha says, as
        // Black Ops' camos): painted on, turned and scaled by `look.xy`,
        // shaded by the gun's own wear (the colour map against its local
        // average) so edges, scratches and engraving still read.
        let t = in.uv * detail_scale.xy;
        let uv = vec2(t.x * look.x - t.y * look.y, t.x * look.y + t.y * look.x);
        let paint = textureSample(detail_map, detail_sampler, uv).rgb;
        let base = pow(pbr_input.material.base_color.rgb, vec3(1.0 / 2.2));
        let around = pow(textureSampleLevel(color_map, color_sampler, in.uv, 5.0).rgb, vec3(1.0 / 2.2));
        let relief = clamp(luma(base) / max(luma(around), 0.05), 0.35, 1.4);
        let mask = select(1.0, pbr_input.material.base_color.a, detail_scale.z > 6.5);
        let painted = paint * mix(1.0, relief, 0.75);
        pbr_input.material.base_color = vec4(mix(pbr_input.material.base_color.rgb, painted, mask), pbr_input.material.base_color.a);
    } else if (detail_scale.w > 0.5) {
        let detail = textureSample(detail_map, detail_sampler, in.uv * detail_scale.xy).rgb;
        let base = pow(pbr_input.material.base_color.rgb, vec3(1.0 / 2.2));
        var camo = pow(detail, vec3(1.0 / 2.2));
        camo = mix(vec3(luma(camo)), camo, look.y);
        // Black Ops (`detail_scale.z` = 1): only where the colour map's alpha says.
        let mask = select(1.0, pbr_input.material.base_color.a, detail_scale.z > 0.5);
        var color = saturate(base + (camo - 0.5) * look.x * mask);
        if (look.z > 0.0) {
            // Painted: the camo, shaded by the colour map's detail (it
            // against its average over ~32 texels).
            let around = pow(textureSampleLevel(color_map, color_sampler, in.uv, 5.0).rgb, vec3(1.0 / 2.2));
            let wear = clamp(base / max(around, vec3(0.03)), vec3(0.3), vec3(1.7));
            color = mix(color, saturate(camo * wear), look.z * mask);
        }
        color = pow(color, vec3(2.2));
        if (detail_scale.z > 2.5) {
            // Mastery metal: neutral grain with the original gun's small
            // wear/engravings, without inheriting its painted colour.
            let around = pow(textureSampleLevel(color_map, color_sampler, in.uv, 5.0).rgb, vec3(1.0 / 2.2));
            let relief = clamp(luma(base) / max(luma(around), 0.05), 0.3, 1.35);
            color = detail * mix(1.0, relief, 0.7);
            if (detail_scale.z > 3.5) {
                color = color * vec3(0.78, 0.40, 0.065);
            } else {
                // Platinum reflects about 60% of the light (its F0), not
                // all of it: near white, it outshone everything around.
                color = color * vec3(0.62, 0.60, 0.57);
            }
        } else if (detail_scale.z > 1.5) {
            // Black Ops' own colour map is nearly black (its gold is all
            // reflection, from much brighter probes than ours): CoD4's gold
            // guns' average colour under the shine instead.
            gold = mask;
            color = mix(pbr_input.material.base_color.rgb, pow(vec3(0.30, 0.20, 0.13), vec3(2.2)), mask);
        }
        pbr_input.material.base_color = vec4(color, pbr_input.material.base_color.a);
    }
#endif
    if (abs(detail_scale.z - 5.0) < 0.5) {
        // Clearcoat is specialised on the CPU for Diamond, but only the
        // crystal crowns use it; the gold backing keeps its metal finish.
        pbr_input.material.clearcoat = 0.0;
    }
    if (diamond_stud) {
        // Real, flat-shaded crystal crowns. Ignore the gun's normal atlas
        // on them so each facet reflects from its own geometric normal.
        pbr_input.N = normalize(in.world_normal);
        pbr_input.world_normal = pbr_input.N;
        pbr_input.material.base_color = vec4(0.30, 0.34, 0.39, 1.0);
        pbr_input.material.metallic = 0.05;
        pbr_input.material.perceptual_roughness = 0.09;
        pbr_input.material.reflectance = vec3(0.17);
        pbr_input.material.clearcoat = 1.0;
        pbr_input.material.clearcoat_perceptual_roughness = 0.05;
        pbr_input.clearcoat_N = pbr_input.N;
    }
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
#ifdef IRRADIANCE_VOLUME
    if (detail_scale.z > 2.5 && detail_scale.z < 5.5) {
        // Polished metal (platinum, gold, diamond's gold) has no diffuse
        // colour, so Bevy's flat ambient reflection was nearly all its
        // light, the same in shade as in the open: the gun glowed white in
        // the shade while the arms (lit by the light grid) went dark. Its
        // reflection is taken from the light grid instead, round the
        // reflected direction, like the arms' light.
        let m = pbr_input.material;
        let f0 = calculate_F0(m.base_color.rgb, m.metallic, m.reflectance);
        let n_dot_v = max(dot(pbr_input.N, pbr_input.V), 1e-4);
        let flat = ambient::ambient_light(pbr_input.world_position, pbr_input.N, pbr_input.V, n_dot_v, m.base_color.rgb * (1.0 - m.metallic), f0, m.perceptual_roughness, pbr_input.diffuse_occlusion);
        let view_z = dot(vec4(view.view_from_world[0].z, view.view_from_world[1].z, view.view_from_world[2].z, view.view_from_world[3].z), pbr_input.world_position);
        var ranges = clustering::unpack_clusterable_object_index_ranges(clustering::view_fragment_cluster_index(pbr_input.frag_coord.xy, view_z, false));
        let r = reflect(-pbr_input.V, pbr_input.N);
        let grid = irradiance_volume::irradiance_volume_light(pbr_input.world_position.xyz, r, &ranges);
        let reflected = EnvBRDFApprox(f0, F_AB(m.perceptual_roughness, n_dot_v)) * grid * pbr_input.diffuse_occlusion;
        out.color = vec4(max(out.color.rgb + (reflected - flat) * view.exposure, vec3(0.0)), out.color.a);
    }
#endif
#ifdef VERTEX_UVS_A
    if (shine.x > 0.5) {
        var s = textureSample(specular_map, specular_sampler, in.uv);
        if (detail_scale.z > 5.5) {
            // Custom camo: the map's wear in grey, as glossy as its finish
            // (`look.z`: matte 0, satin 0.5, gloss 1).
            let silver = max(s.r, max(s.g, s.b));
            s = vec4(vec3(silver * mix(0.3, 1.0, look.z)), mix(0.15, 0.8, look.z));
        } else if (detail_scale.z > 2.5) {
            // Gold (its specular map is gold): as polished as CoD4's gold guns.
            // Keep specular wear while neutralising source material hues.
            let silver = max(s.r, max(s.g, s.b));
            s = vec4(vec3(silver), clamp(s.a * 0.8 + 0.14, 0.42, 0.78));
            if (detail_scale.z > 3.5) {
                s = vec4(s.rgb * vec3(0.95, 0.70, 0.28), 0.64);
            }
            if (diamond_stud) {
                s = vec4(vec3(0.72), 0.95);
            }
        } else {
            s = vec4(s.rgb * select(1.0, gold, detail_scale.z > 1.5), mix(s.a, 0.48, gold));
        }
        let n = normalize(pbr_input.N);
        let v = normalize(in.world_position.xyz - view.world_position);
        let r = reflect(v, n);
        // Without a map (the menus' previews): a plain studio, bright above
        // and dim below.
        let studio = vec3(mix(0.08, 0.45, saturate(r.y * 0.5 + 0.5)));
        var light = studio;
        if (shine.y > 0.5) {
            // Probes are looked up with CoD axes: Bevy (x, y, z) is CoD (x, -z, y).
            let p = textureSampleLevel(reflection_probe, probe_sampler, vec3(r.x, -r.z, r.y), clamp(6.0 - 8.0 * s.a, 0.0, shine.z));
            light = p.rgb * p.a;
            // Polished metal (gold, platinum, diamond: a fresnel minimum of
            // 3 and up, ordinary gun metal's is at most 2) never goes flat:
            // night maps' probes are near black (Wet Work's), which left
            // them dull brown; at least some of the studio's light.
            if (detail_scale.z > 2.5 && detail_scale.z < 5.5 || env.x > 2.9) {
                light = max(light, studio * 0.35);
            }
        }
        let fresnel = mix(env.x, env.y, pow(max(1.0 - abs(dot(v, n)), 0.0), env.z));
        var raw = s.rgb * fresnel * light;
        // IW3 wrote to an 8-bit target: never more than white (then
        // `look.w` brighter).
        var spec = saturate(raw);
        if (look.w > 1.0) {
            // Polished metal reflects in its colour (its hue, kept bright).
            // In gamma space, as IW3's specular is, and partly (gold's
            // brown colour map in full would make it copper).
            let base = pow(pbr_input.material.base_color.rgb, vec3(1.0 / 2.2));
            let tint = base / max(max(base.r, max(base.g, base.b)), 0.001);
            raw = raw * mix(vec3(1.0), tint, 0.6 * saturate((env.x - 1.0) / 3.0));
            // Down to white by the brightest channel, keeping the hue.
            spec = raw / max(1.0, max(raw.r, max(raw.g, raw.b)));
        }
        out.color = vec4(out.color.rgb + pow(spec, vec3(2.2)) * look.w * shine.w * view.exposure, out.color.a);
    }
#endif
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
