// A red dot sight's dot (IW3's `mc_reflexsight`, from its shaders): the
// dot texture laid over the view directions around the lens's normal, so
// the dot sits on the sight line wherever the eye is and the rest of the
// lens stays clear, times a grain texture on the lens.

#import bevy_pbr::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
}

// xy: `detailScale`, texture widths per unit of the view direction's
// offset from the sight line.
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> scale: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var dot_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var dot_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var grain_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var grain_sampler: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let v = normalize(in.world_position.xyz - view.world_position);
    // The sight line: the lens's normal, away from the eye.
    var n = normalize(in.world_normal);
    if dot(n, v) < 0.0 {
        n = -n;
    }
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.y) > 0.99);
    let t = normalize(cross(up, n));
    let b = cross(n, t);
    let uv = vec2<f32>(0.5) + vec2<f32>(dot(v, b), dot(v, t)) * scale.xy;
    let inside = all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0));
    let g = select(0.0, textureSample(dot_texture, dot_sampler, uv).r, inside)
        * textureSample(grain_texture, grain_sampler, in.uv).r;
    // As IW3 writes it to an 8-bit target: a white-hot core in a red glow,
    // blended ONE / INVSRCALPHA (premultiplied).
    let colour = clamp(vec4<f32>(2.0 * g, 1.2 * (g - 0.2), 1.2 * (g - 0.2), 1.2 * g), vec4<f32>(0.0), vec4<f32>(1.0));
    return colour;
}
