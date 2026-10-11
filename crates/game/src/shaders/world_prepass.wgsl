// World surfaces' prepass (`crate::world`): Bevy's standard prepass
// (`bevy_pbr`'s pbr_prepass.wgsl, as of 0.19.1), but an alpha-tested
// surface's alpha includes its vertex colour's, as the main pass's does
// (`pbr_input_from_standard_material`). CoD4's decals fade their edges by
// vertex alpha: with the texture's alone the prepass kept depth where the
// main pass dropped the decal, the wall behind failed its depth test there
// and the sky colour showed through (Overgrown's ivy as white patches).

#import bevy_pbr::{
    pbr_prepass_functions,
    pbr_bindings,
    pbr_bindings::material,
    pbr_types,
    pbr_functions,
    pbr_functions::SampleBias,
    prepass_io,
    mesh_bindings::mesh,
    mesh_view_bindings::view,
}

#import bevy_render::bindless::{bindless_samplers_filtering, bindless_textures_2d}

#ifdef MESHLET_MESH_MATERIAL_PASS
#import bevy_pbr::meshlet_visibility_buffer_resolve::resolve_vertex_output
#endif

#ifdef BINDLESS
#import bevy_pbr::pbr_bindings::material_indices
#endif  // BINDLESS


fn slot_of(in: prepass_io::VertexOutput) -> u32 {
#ifdef BINDLESS
    return mesh[in.instance_index].material_and_lightmap_bind_group_slot & 0xffffu;
#else
    return 0u;
#endif
}

// An alpha-tested card's cutoff lowered with distance: its texture's small
// mips average the cutouts' alpha down, and grass, leaves and bushes thinned
// to see-through ghosts far off. (Same in `world.wgsl`.)
fn distant_cutoff(cutoff: f32, p: vec3<f32>) -> f32 {
    let d = distance(p, bevy_pbr::mesh_view_bindings::view.world_position);
    return cutoff * mix(1.0, 0.45, smoothstep(8.0, 40.0, d));
}

// `pbr_prepass_functions::prepass_alpha_discard` with the vertex alpha.
fn world_alpha_discard(in: prepass_io::VertexOutput, slot: u32) {
#ifdef MAY_DISCARD
#ifdef BINDLESS
    var alpha = pbr_bindings::material_array[material_indices[slot].material].base_color.a;
    let flags = pbr_bindings::material_array[material_indices[slot].material].flags;
    let cutoff = pbr_bindings::material_array[material_indices[slot].material].alpha_cutoff;
    let uv_transform = pbr_bindings::material_array[material_indices[slot].material].uv_transform;
#else
    var alpha = pbr_bindings::material.base_color.a;
    let flags = pbr_bindings::material.flags;
    let cutoff = pbr_bindings::material.alpha_cutoff;
    let uv_transform = pbr_bindings::material.uv_transform;
#endif
#ifdef VERTEX_UVS
    let uv = (uv_transform * vec3(in.uv, 1.0)).xy;
    if (flags & pbr_types::STANDARD_MATERIAL_FLAGS_BASE_COLOR_TEXTURE_BIT) != 0u {
        alpha *= textureSampleBias(
#ifdef BINDLESS
            bindless_textures_2d[material_indices[slot].base_color_texture],
            bindless_samplers_filtering[material_indices[slot].base_color_sampler],
#else
            pbr_bindings::base_color_texture,
            pbr_bindings::base_color_sampler,
#endif
            uv,
            view.mip_bias
        ).a;
    }
#endif
#ifdef VERTEX_COLORS
    alpha *= in.color.a;
#endif
    let alpha_mode = flags & pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_RESERVED_BITS;
    if alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_MASK {
        // (Matching `world.wgsl`: cards keep their cover at a distance.)
        if alpha < distant_cutoff(cutoff, in.world_position.xyz) {
            discard;
        }
    } else if (alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_BLEND
        || alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_ADD
        || alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_PREMULTIPLIED) {
        // (Bevy's cutoff for these.)
        if alpha < 0.05 {
            discard;
        }
    }
#endif
}

#ifdef PREPASS_FRAGMENT
@fragment
fn fragment(
#ifdef MESHLET_MESH_MATERIAL_PASS
    @builtin(position) frag_coord: vec4<f32>,
#else
    in: prepass_io::VertexOutput,
    @builtin(front_facing) is_front: bool,
#endif
) -> prepass_io::FragmentOutput {
#ifdef MESHLET_MESH_MATERIAL_PASS
    let in = resolve_vertex_output(frag_coord);
    let is_front = true;
#else   // MESHLET_MESH_MATERIAL_PASS

#ifdef BINDLESS
    let slot = mesh[in.instance_index].material_and_lightmap_bind_group_slot & 0xffffu;
    let flags = pbr_bindings::material_array[material_indices[slot].material].flags;
    let uv_transform = pbr_bindings::material_array[material_indices[slot].material].uv_transform;
#else   // BINDLESS
    let flags = pbr_bindings::material.flags;
    let uv_transform = pbr_bindings::material.uv_transform;
#endif  // BINDLESS

    // If we're in the crossfade section of a visibility range, conditionally
    // discard the fragment according to the visibility pattern.
#ifdef VISIBILITY_RANGE_DITHER
    pbr_functions::visibility_range_dither(in.position, in.visibility_range_dither);
#endif  // VISIBILITY_RANGE_DITHER

    world_alpha_discard(in, slot_of(in));
#endif  // MESHLET_MESH_MATERIAL_PASS

    var out: prepass_io::FragmentOutput;

#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.frag_depth = in.unclipped_depth;
#endif // UNCLIPPED_DEPTH_ORTHO_EMULATION

#ifdef NORMAL_PREPASS
    // NOTE: Unlit bit not set means == 0 is true, so the true case is if lit
    if (flags & pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        let double_sided = (flags & pbr_types::STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT) != 0u;

        let world_normal = pbr_functions::prepare_world_normal(
            in.world_normal,
            double_sided,
            is_front,
        );

        var normal = world_normal;

#ifdef VERTEX_UVS
#ifdef VERTEX_TANGENTS
#ifdef STANDARD_MATERIAL_NORMAL_MAP

// TODO: Transforming UVs mean we need to apply derivative chain rule for meshlet mesh material pass
#ifdef STANDARD_MATERIAL_NORMAL_MAP_UV_B
        let uv = (uv_transform * vec3(in.uv_b, 1.0)).xy;
#else
        let uv = (uv_transform * vec3(in.uv, 1.0)).xy;
#endif

        // Fill in the sample bias so we can sample from textures.
        var bias: SampleBias;
#ifdef MESHLET_MESH_MATERIAL_PASS
        bias.ddx_uv = in.ddx_uv;
        bias.ddy_uv = in.ddy_uv;
#else   // MESHLET_MESH_MATERIAL_PASS
        bias.mip_bias = view.mip_bias;
#endif  // MESHLET_MESH_MATERIAL_PASS

        let Nt =
#ifdef MESHLET_MESH_MATERIAL_PASS
            textureSampleGrad(
#else   // MESHLET_MESH_MATERIAL_PASS
            textureSampleBias(
#endif  // MESHLET_MESH_MATERIAL_PASS
#ifdef BINDLESS
                bindless_textures_2d[material_indices[slot].normal_map_texture],
                bindless_samplers_filtering[material_indices[slot].normal_map_sampler],
#else   // BINDLESS
                pbr_bindings::normal_map_texture,
                pbr_bindings::normal_map_sampler,
#endif  // BINDLESS
                uv,
#ifdef MESHLET_MESH_MATERIAL_PASS
                bias.ddx_uv,
                bias.ddy_uv,
#else   // MESHLET_MESH_MATERIAL_PASS
                bias.mip_bias,
#endif  // MESHLET_MESH_MATERIAL_PASS
            ).rgb;
        let TBN = pbr_functions::calculate_tbn_mikktspace(normal, in.world_tangent);

        normal = pbr_functions::apply_normal_mapping(
            flags,
            TBN,
            double_sided,
            is_front,
            Nt,
        );

#endif  // STANDARD_MATERIAL_NORMAL_MAP
#endif  // VERTEX_TANGENTS
#endif  // VERTEX_UVS

        out.normal = vec4(normal * 0.5 + vec3(0.5), 1.0);
    } else {
        out.normal = vec4(in.world_normal * 0.5 + vec3(0.5), 1.0);
    }
#endif // NORMAL_PREPASS

#ifdef MOTION_VECTOR_PREPASS
#ifdef MESHLET_MESH_MATERIAL_PASS
    out.motion_vector = in.motion_vector;
#else
    out.motion_vector = pbr_prepass_functions::calculate_motion_vector(in.world_position, in.previous_world_position);
#endif
#endif

    return out;
}
#else
@fragment
fn fragment(in: prepass_io::VertexOutput) {
    world_alpha_discard(in, slot_of(in));
}
#endif // PREPASS_FRAGMENT
