// Soldiers' look (`character_look.rs`): CoD4's character material, Bevy's
// lighting, CoD4's own model specular (below), then per pixel by class
// - skin (found by its colour, where the material allows): softer, a
//   reddish wrap of the sun past the terminator (light scattered through
//   skin), a broad second highlight, its light lifted in the shade;
// - cloth: rough, with a sheen lobe (Charlie: Estevez & Kulla 2017) and a
//   little grazing sheen from the light grid;
// - hard gear (helmets, goggles, kit): smoother, reflecting the light
//   grid's light round its reflection (as the guns' polished metal does).
// CoD4's model specular (IW3's `lp_sun`/`lp_*` model techniques): the
// specular map's colour times a fresnel term (`envMapParms`: min, max,
// power) times the reflection probe (sharper the glossier), and the sun's
// glint shaped by the gloss. That shine on skin, eyes and sweat is much of
// what makes CoD4's soldiers read as detailed. Here the probe's light is
// dimmed by the light grid round the reflection (probes are captured in
// the open; a face in the shade went on gleaming with the sky).
// The sun's extra terms take its shadow (and contact shadows) as Bevy's do.

#import bevy_pbr::{
    mesh_view_bindings as view_bindings,
    mesh_view_bindings::view,
    mesh_view_types,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
    clustered_forward as clustering,
    lighting::{EnvBRDFApprox, F_AB},
    pbr_functions::calculate_F0,
    ambient,
    shadows,
    mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT,
}
#ifdef CONTACT_SHADOWS
#ifdef DEPTH_PREPASS
#import bevy_pbr::pbr_functions::calculate_contact_shadow
#endif
#endif
#ifdef IRRADIANCE_VOLUME
#import bevy_pbr::irradiance_volume
#endif

// x: skin allowed, y: gear allowed, z: has a specular map, w: debug view
// (classes as colours).
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> classes: vec4<f32>;
// rgb: scattered light's colour, w: its strength.
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var<uniform> skin: vec4<f32>;
// x: cloth sheen, y: its roughness, z: gear reflection, w: skin's second highlight.
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var<uniform> cloth: vec4<f32>;
// x: wrap, y: shade lift, z: where the A/B wipe falls (0..1 across), w:
// CoD4's specular's strength.
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var<uniform> extra: vec4<f32>;
// CoD4's specular map: specular colour (rgb, gamma), gloss (a).
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var specular_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var specular_sampler: sampler;
// `envMapParms`: fresnel min, max, power, sun glint strength.
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var<uniform> env: vec4<f32>;
// x: has a probe, y: the probe's last mip, z: the probe's exposure (IW3
// light to ours, the showcase's sky and rain), w: the open air's light (the
// light grid's, for dimming the probe in the shade).
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var<uniform> shine: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var reflection_probe: texture_cube<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(109) var probe_sampler: sampler;
// x: how much of Bevy's flat ambient light characters keep, y: 1 for an
// eye's highlight shell (`mtl_eye_highlight`): only CoD4's specular, added;
// z: the cap on CoD4's grazing fresnel.
@group(#{MATERIAL_BIND_GROUP}) @binding(110) var<uniform> more: vec4<f32>;

const PI: f32 = 3.14159265;

// How much a (gamma-space) colour looks like CoD4's skin: hue 11-28
// degrees, saturation 0.35-0.6, red over green 1.25+, not near black.
fn skin_likeness(c: vec3<f32>) -> f32 {
    let hi = max(c.r, max(c.g, c.b));
    let lo = min(c.r, min(c.g, c.b));
    let sat = (hi - lo) / max(hi, 1e-3);
    let hue = degrees(atan2(sqrt(3.0) * (c.g - c.b), 2.0 * c.r - c.g - c.b));
    let h = smoothstep(6.0, 11.0, hue) * (1.0 - smoothstep(27.0, 33.0, hue));
    let s = smoothstep(0.22, 0.32, sat) * (1.0 - smoothstep(0.64, 0.74, sat));
    let l = smoothstep(0.05, 0.12, hi);
    // Red over green 1.25-1.65 on faces and hands; brown cloth's ~1.17.
    let rg = smoothstep(1.18, 1.28, c.r / max(c.g, 1e-3));
    return h * s * l * rg;
}

fn d_ggx(roughness: f32, n_dot_h: f32) -> f32 {
    let a2 = roughness * roughness * roughness * roughness;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn d_charlie(roughness: f32, n_dot_h: f32) -> f32 {
    let inv = 1.0 / max(roughness * roughness, 1e-3);
    let sin2h = max(1.0 - n_dot_h * n_dot_h, 0.0078125);
    return (2.0 + inv) * pow(sin2h, inv * 0.5) / (2.0 * PI);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    // (Before any branch: derivatives need uniform control flow.)
    let dn = max(dot(dpdx(pbr_input.N), dpdx(pbr_input.N)), dot(dpdy(pbr_input.N), dpdy(pbr_input.N)));
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    var out: FragmentOutput;
    // A/B: CoD4's material as it was (2), or on the screen's left half (3),
    // or on alternate 6-pixel bands (4: measuring).
    let left = in.position.x < view.viewport.x + extra.z * view.viewport.z;
    let band = (u32(in.position.y) / 6u) % 2u == 0u && classes.w < 4.5;
    if (classes.w > 1.5 && (classes.w < 2.5 || (classes.w < 3.5 && left) || (classes.w > 3.5 && band))) {
        out.color = main_pass_post_lighting_processing(pbr_input, apply_pbr_lighting(pbr_input));
        if (more.y > 0.5) {
            // (An eye's shell is drawn added: nothing on the A side.)
            out.color = vec4(0.0);
        }
        return out;
    }
    let albedo = pbr_input.material.base_color.rgb;
    // Without a specular map: dull, a little glossy (CoD4's typical cloth
    // and skin levels).
    var spec = vec4(0.25, 0.25, 0.25, 0.1);
#ifdef VERTEX_UVS_A
    if (classes.z > 0.5) {
        spec = textureSample(specular_map, specular_sampler, in.uv);
    }
#endif
    let s = classes.x * skin_likeness(pow(albedo, vec3(1.0 / 2.2)));
    // Gear: allowed by name and glossy (helmets ~0.28, skin 0.14-0.21,
    // cloth under 0.1).
    let g = classes.y * smoothstep(0.2, 0.3, spec.a) * (1.0 - s);
    let c = max(1.0 - s - g, 0.0);
    // Cloth's sheen colour: its own hue (the sky's blue light made olive
    // gloves grey-blue with a plain sqrt), sqrt-bright.
    let hue = albedo / max(max(albedo.r, max(albedo.g, albedo.b)), 1e-3);
    let sheen_tint = sqrt(max(max(albedo.r, albedo.g), albedo.b)) * mix(vec3(1.0), hue, 0.7);
    // Roughness from the gloss (IW3 sampled its probe at mip 6 - 8 gloss of
    // ~7): skin ~0.55-0.65 (Bevy's lobe is skin's sharp one; the broad one is
    // added below), cloth near 1, helmets ~0.5. F0 from the specular colour
    // times about CoD4's fresnel minimum for characters (envMapParms.x
    // 0.4-0.6), kept low (below).
    let gloss = spec.a;
    let rough = clamp(1.0 - 1.6 * gloss, 0.35, 0.97);
    pbr_input.material.perceptual_roughness = s * min(rough, 0.62) + c * clamp(rough, 0.8, 0.85) + g * rough;
    let f0 = pow(max(max(spec.r, spec.g), spec.b), 2.2) * 0.5;
    // Kept low: Bevy's flat ambient light is reflected untinted at this F0
    // everywhere, the shade too (faces looked pale at 0.03); cloth stays
    // near CoD4's 0.0064, its shine is the sheen.
    let f0_class = s * clamp(f0, 0.015, 0.022) + c * min(f0, 0.03) * 0.25 + g * clamp(f0, 0.02, 0.04);
    // (Bevy: F0 = 0.16 reflectance^2.)
    pbr_input.material.reflectance = vec3(sqrt(f0_class / 0.16));
    out.color = apply_pbr_lighting(pbr_input);
    // CoD4 lit models by the light grid and the sun alone: Bevy's flat
    // ambient light on top (on every side alike) washed out the eye
    // sockets and the shade under a hat. `more.x` keeps that much of it.
    {
        let m = pbr_input.material;
        let nv = max(dot(pbr_input.N, pbr_input.V), 1e-4);
        let f0 = calculate_F0(m.base_color.rgb, m.metallic, m.reflectance);
        let flat = ambient::ambient_light(pbr_input.world_position, pbr_input.N, pbr_input.V, nv, m.base_color.rgb * (1.0 - m.metallic), f0, m.perceptual_roughness, pbr_input.diffuse_occlusion);
        out.color = vec4(max(out.color.rgb - flat * (1.0 - more.x) * view.exposure, vec3(0.0)), out.color.a);
    }

    let N = pbr_input.N;
    let V = pbr_input.V;
    let n_dot_v = max(dot(N, V), 1e-4);
    let eye = more.y > 0.5;
    // CoD4's specular alone (its glint and probe reflection), for the eyes.
    var cod4_added = vec3(0.0);
    // CoD4's specular colour times its fresnel, in gamma space as IW3's.
    // (Its grazing maximum capped, `more.z`: CoD4's heads set up to 8,
    // and our brighter sky probes then rimmed brows and hair in white.)
    let fresnel = mix(env.x, min(env.y, max(more.z, env.x)), pow(max(1.0 - n_dot_v, 0.0), env.z));
    let cod4_spec = spec.rgb * fresnel;
    // Its glint's spread: IW3's specular power from the gloss, as GGX,
    // widened where the normal turns fast across a pixel (specular
    // anti-aliasing, Kaplanyan & Hoffman: tiny sharp highlights on eyes,
    // brows and stubble sparkled from frame to frame).
    let widen = min(2.0 * dn, 0.25);
    let glint_rough = sqrt(min(pow(clamp(1.0 - gloss, 0.25, 0.9), 2.0) + widen, 0.81));
    let view_z = dot(vec4(view.view_from_world[0].z, view.view_from_world[1].z, view.view_from_world[2].z, view.view_from_world[3].z), pbr_input.world_position);
    var added = vec3(0.0);
    // How much the sun (or moon) reaches here, for the skin's shade lift.
    var sunlit = 0.0;
    // Debug (`classes.w` 6): the sun's shadow here (r), 3 m up (g), 30 m
    // toward the sun (b): what shades a soldier.
    var sun_debug = vec3(0.0);

    // The sun and moon.
    let lights = view_bindings::lights.n_directional_lights;
    for (var i: u32 = 0u; i < lights; i = i + 1u) {
        let light = &view_bindings::lights.directional_lights[i];
        let L = (*light).direction_to_light.xyz;
        var shadow = 1.0;
        if ((pbr_input.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u && ((*light).flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            shadow = shadows::fetch_directional_shadow(i, pbr_input.world_position, pbr_input.world_normal, view_z, pbr_input.frag_coord.xy);
        }
#ifdef CONTACT_SHADOWS
#ifdef DEPTH_PREPASS
        let steps = view_bindings::contact_shadows_settings.linear_steps;
        if (steps > 0u && (pbr_input.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u && shadow > 0.0 && ((*light).flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_CONTACT_SHADOWS_ENABLED_BIT) != 0u) {
            shadow *= calculate_contact_shadow(pbr_input.world_position.xyz, pbr_input.frag_coord.xy, L, steps);
        }
#endif
#endif
        let color = (*light).color.rgb * shadow;
        let n_dot_l = dot(N, L);
        let lit = saturate(n_dot_l);
        sunlit = max(sunlit, lit * shadow);
        if (i == 0u && classes.w > 5.5) {
            let up3 = pbr_input.world_position + vec4(0.0, 3.0, 0.0, 0.0);
            let up30 = pbr_input.world_position + vec4(L * 30.0, 0.0);
            sun_debug = vec3(shadow,
                shadows::fetch_directional_shadow(i, up3, pbr_input.world_normal, view_z, pbr_input.frag_coord.xy),
                shadows::fetch_directional_shadow(i, up30, pbr_input.world_normal, view_z, pbr_input.frag_coord.xy));
        }
        let H = normalize(L + V);
        let n_dot_h = saturate(dot(N, H));
        // CoD4's sun glint (its specular colour linear, as F0).
        let glint_vis = 0.5 / max(lit * (n_dot_v * (1.0 - glint_rough) + glint_rough) + n_dot_v * (lit * (1.0 - glint_rough) + glint_rough), 1e-4);
        let glint = pow(saturate(cod4_spec), vec3(2.2)) * extra.w * d_ggx(glint_rough, n_dot_h) * glint_vis * lit * color;
        added += glint;
        cod4_added += glint;
        if (s > 0.0) {
            // Past the terminator: wrapped light, reddened (the deep
            // scatter), rising where plain Lambert falls off.
            let w = extra.x;
            let wrapped = saturate((n_dot_l + w) / (1.0 + w));
            let scatter = skin.rgb * (wrapped - lit) + (skin.rgb - vec3(1.0)) * 0.15 * lit * (1.0 - lit);
            added += s * skin.w * albedo / PI * max(scatter, vec3(0.0)) * color;
            // A broad second lobe (skin's oily sheen over its rough layer).
            let f = 0.028 + 0.972 * pow(1.0 - saturate(dot(H, V)), 5.0);
            let vis = 0.5 / max(lit * (n_dot_v * 0.4 + 0.6) + n_dot_v * (lit * 0.4 + 0.6), 1e-4);
            added += s * cloth.w * d_ggx(0.75, n_dot_h) * vis * f * lit * color;
        }
        if (c > 0.0) {
            // Cloth sheen, tinted by the cloth.
            let sheen = cloth.x * sheen_tint;
            let vis = 1.0 / (4.0 * (lit + n_dot_v - lit * n_dot_v) + 1e-4);
            added += c * sheen * d_charlie(cloth.y, n_dot_h) * vis * lit * color;
        }
    }

#ifdef VERTEX_UVS_A
    if (shine.x > 0.5 && extra.w > 0.0) {
        // CoD4's probe reflection. Probes are looked up with CoD axes: Bevy
        // (x, y, z) is CoD (x, -z, y).
        let r = reflect(-V, N);
        let p = textureSampleLevel(reflection_probe, probe_sampler, vec3(r.x, -r.z, r.y), clamp(6.0 - 8.0 * gloss + 8.0 * sqrt(widen), 0.0, shine.y));
        var light = p.rgb * p.a;
#ifdef IRRADIANCE_VOLUME
        var ranges2 = clustering::unpack_clusterable_object_index_ranges(clustering::view_fragment_cluster_index(pbr_input.frag_coord.xy, view_z, false));
        let here = irradiance_volume::irradiance_volume_light(pbr_input.world_position.xyz, r, &ranges2);
        light *= clamp(dot(here, vec3(0.2126, 0.7152, 0.0722)) / max(shine.w, 1e-4), 0.1, 1.0);
#endif
        // IW3's: in gamma space, never past its 8-bit white.
        let reflected = pow(saturate(cod4_spec * light), vec3(2.2)) * shine.z * extra.w;
        added += reflected * pbr_input.specular_occlusion;
        cod4_added += reflected;
    }
#endif

#ifdef IRRADIANCE_VOLUME
    var ranges = clustering::unpack_clusterable_object_index_ranges(clustering::view_fragment_cluster_index(pbr_input.frag_coord.xy, view_z, false));
    let pos = pbr_input.world_position.xyz;
    let ao = pbr_input.diffuse_occlusion;
    if (c > 0.0) {
        // Grazing sheen from the surroundings.
        let around = irradiance_volume::irradiance_volume_light(pos, N, &ranges);
        added += c * cloth.x * 0.2 * sheen_tint * pow(1.0 - n_dot_v, 3.0) * around * ao;
    }
    if (g > 0.0) {
        let r = reflect(-V, N);
        let around = irradiance_volume::irradiance_volume_light(pos, r, &ranges);
        added += g * cloth.z * EnvBRDFApprox(vec3(0.04), F_AB(0.5, n_dot_v)) * around * ao;
    }
#endif

    // In the shade skin keeps some glow (light scattered in through it from
    // around), so faces don't go dead grey: its light raised, warmer.
    let lift = s * extra.y * (1.0 - sunlit) * mix(vec3(1.0), skin.rgb, 0.5);
    out.color = vec4(out.color.rgb * (1.0 + lift) + added * view.exposure, out.color.a);
    if (eye) {
        // The eye's wet shine over the eyeball beneath (drawn added).
        out.color = vec4(cod4_added * view.exposure, 1.0);
    }
    if (classes.w > 5.5) {
        out.color = vec4(sun_debug * 0.5, 1.0);
    } else if (classes.w > 4.5) {
        // Debug: what this look adds (x8), red the shade lift's share.
        out.color = vec4(added * view.exposure * 8.0 + vec3(out.color.r * lift.r * 8.0, 0.0, 0.0), out.color.a);
    }
    if (classes.w > 0.5 && classes.w < 1.5) {
        out.color = vec4(vec3(s, c, g) * 0.5 * (0.3 + 0.7 * saturate(dot(N, normalize(vec3(0.3, 1.0, 0.2))))), out.color.a);
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
