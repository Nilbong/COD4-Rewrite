// The showcase's shadow upgrades for the lightmapped world (`crate::showcase_shadows`),
// imported by `world.wgsl`.
#define_import_path cod4rw::shadows

#import bevy_pbr::{
    clustered_forward as clustering,
    mesh_view_bindings as view_bindings,
    mesh_view_types::{POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT, POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE},
    shadows,
}
#ifdef CONTACT_SHADOWS
#ifdef DEPTH_PREPASS
#import bevy_pbr::pbr_functions::calculate_contact_shadow
#endif
#endif

// The shadow-casting lamps' power (lumens), `showcase_shadows::LAMP_MARK`:
// their light is this times their weight.
const LAMP_MARK: f32 = 0.01;
const FOUR_PI: f32 = 12.566371;

// The sun's contact shadow (screen-space rays toward it), 1 without
// contact shadows on the view.
fn sun_contact(world_position: vec3<f32>, frag_coord: vec2<f32>, to_sun: vec3<f32>) -> f32 {
#ifdef CONTACT_SHADOWS
#ifdef DEPTH_PREPASS
    let steps = view_bindings::contact_shadows_settings.linear_steps;
    if (steps > 0u) {
        return calculate_contact_shadow(world_position, frag_coord, to_sun, steps);
    }
#endif
#endif
    return 1.0;
}

// How much of the baked light is left where something moving blocks one of
// the shadow-casting lamps (1: nothing does). Each lamp's light is weighed
// by its reach and how the surface faces it.
fn lamp_shade(world_position: vec4<f32>, normal: vec3<f32>, frag_coord: vec2<f32>, view_z: f32) -> f32 {
    let ranges = clustering::unpack_clusterable_object_index_ranges(clustering::view_fragment_cluster_index(frag_coord, view_z, false));
    var shade = 1.0;
    for (var i: u32 = ranges.first_point_light_index_offset; i < ranges.first_reflection_probe_index_offset; i = i + 1u) {
        let id = clustering::get_clusterable_object_id(i);
        let light = &view_bindings::clustered_lights.data[id];
        if (((*light).flags & POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u) {
            continue;
        }
        let c = (*light).color_inverse_square_range;
        // (Bevy's intensity is the power over 4 pi.)
        var weight = max(c.r, max(c.g, c.b)) * FOUR_PI / LAMP_MARK;
        // (Debug, `COD4RW_LAMPDEBUG`: weight 2.)
        let debug = weight > 1.5 && weight < 2.5;
        if (debug) {
            weight = 1.0;
        }
        if (weight <= 0.001 || weight > 1.01) {
            continue; // not one of ours
        }
        let to = (*light).position_radius.xyz - world_position.xyz;
        let d2 = dot(to, to);
        let reach = saturate(1.0 - d2 * c.w);
        let facing = saturate(dot(normal, to * inverseSqrt(max(d2, 1e-6))));
        let k = weight * reach * reach * facing;
        if (k <= 0.001) {
            continue;
        }
        var lit = 1.0;
        if (i < ranges.first_spot_light_index_offset) {
            lit = shadows::fetch_point_shadow(id, world_position, normal, frag_coord);
        } else {
            // Only inside the cone (as Bevy's `spot_light`): outside it the
            // lamp lit nothing to take away.
            var spot_dir = vec3((*light).light_custom_data.x, 0.0, (*light).light_custom_data.y);
            spot_dir.y = sqrt(max(0.0, 1.0 - spot_dir.x * spot_dir.x - spot_dir.z * spot_dir.z));
            if (((*light).flags & POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE) != 0u) {
                spot_dir.y = -spot_dir.y;
            }
            let cone = saturate(dot(-spot_dir, normalize(to)) * (*light).light_custom_data.z + (*light).light_custom_data.w);
            if (cone * cone * k <= 0.001) {
                continue;
            }
            lit = mix(1.0, shadows::fetch_spot_shadow(id, world_position, normal, (*light).shadow_map_near_z, frag_coord), cone * cone);
        }
        if (debug) {
            shade *= select(1.0, 0.75 * mix(0.05, 1.0, lit), k > 0.001);
            continue;
        }
        shade *= 1.0 - k * (1.0 - lit);
    }
    return shade;
}
