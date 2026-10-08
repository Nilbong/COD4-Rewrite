// A 3D scope's eyepiece (`ui/scope3d.rs`): the scope camera's magnified
// view, looked up by the eye's direction; the reticle on the scope's axis;
// dark beyond the eye's reach, more so the further the axis is off the
// eye's.

#import bevy_pbr::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
}

// x: fade in, y: reticle (1 duplex, 2 chevron), z: image scale, w: reach.
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var view_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var view_sampler: sampler;
// xyz: the scope's axis, away from the eye.
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> axis: vec4<f32>;

fn bar(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, w: f32, px: f32) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / dot(ab, ab), 0.0, 1.0);
    return clamp(1.0 - max(length(p - (a + ab * t)) - w, 0.0) / px, 0.0, 1.0);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let v = normalize(in.world_position.xyz - view.world_position);
    // The camera's axes: the scope camera looks the same way.
    let right = normalize(view.world_from_view[0].xyz);
    let up = normalize(view.world_from_view[1].xyz);
    let forward = -normalize(view.world_from_view[2].xyz);
    let along = max(dot(v, forward), 1e-3);
    // The eye's direction off the view's middle (tangents).
    let d = vec2<f32>(dot(v, right), dot(v, up)) / along;
    let uv = vec2<f32>(0.5, 0.5) + vec2<f32>(d.x, -d.y) * params.z;
    var c = textureSampleLevel(view_texture, view_sampler, uv, 0.0).rgb;

    // The scope's axis (the gun's forward).
    let n = normalize(axis.xyz);
    // The eye's direction off the scope's axis, in the axis's frame (as
    // the camera's, so the reticle stands upright), over the lens's reach.
    let t = normalize(cross(n, up));
    let b = cross(t, n);
    let on_axis = max(dot(v, n), 1e-3);
    let p = vec2<f32>(dot(v, -t), dot(v, b)) / on_axis / params.w;
    // How far the scope's axis is off the eye's middle, likewise.
    let off = vec2<f32>(dot(n, right), dot(n, up)) / max(dot(n, forward), 1e-3) / params.w;
    let px = max(length(fwidth(p)), 1e-4);
    // The reticle.
    var ink = 0.0;
    if params.y < 1.5 {
        // A duplex: fine cross hairs, thick posts out towards the edge.
        ink = max(max(bar(p, vec2(-1.2, 0.0), vec2(1.2, 0.0), 0.004, px), bar(p, vec2(0.0, -1.2), vec2(0.0, 1.2), 0.004, px)),
            max(max(bar(p, vec2(-1.2, 0.0), vec2(-0.45, 0.0), 0.02, px), bar(p, vec2(0.45, 0.0), vec2(1.2, 0.0), 0.02, px)),
                bar(p, vec2(0.0, -1.2), vec2(0.0, -0.45), 0.02, px)));
        c = mix(c, vec3<f32>(0.0), ink);
    } else {
        // The ACOG's chevron, glowing red, with a post below.
        let q = p * 1.6;
        ink = max(max(bar(q, vec2(-0.12, -0.1), vec2(0.0, 0.0), 0.012, px * 1.6), bar(q, vec2(0.12, -0.1), vec2(0.0, 0.0), 0.012, px * 1.6)),
            bar(q, vec2(0.0, -0.13), vec2(0.0, -0.5), 0.008, px * 1.6));
        c = mix(c, vec3<f32>(3.0, 0.15, 0.05) * max(length(c), 0.3), ink);
    }

    // The eye's reach: dark beyond it, the dark closing in from the side
    // the axis is off to.
    let r = length(p + off * 0.9);
    let reach = 1.0 - smoothstep(0.82, 1.0, r);
    let edge = 1.0 - 0.3 * smoothstep(0.55, 0.95, length(p));
    c *= reach * edge;
    // Coming in: dark glass until the view's there.
    c = mix(vec3<f32>(0.004, 0.005, 0.006), c, params.x);
    return vec4<f32>(c, 1.0);
}
