// Effect sprites (`crate::fx`): IW3's `vertcol_simple*` and `zfeather*`
// effect pixel shaders. The texture times the vertex colour; additive
// materials (blend ONE ONE) add rgb * alpha, the others blend by alpha. Both
// go out premultiplied (Bevy's `AlphaMode::Premultiplied`, with alpha 0 for
// additive). Blended sprites fade as balls through the scene behind them
// (below); additive `zfeather` ones where the sprite nears the scene behind
// it (soft particles), over `featherParms`' distance. Both need the
// camera's depth prepass.
//
// IW3 drew effects in gamma space onto an 8-bit target. Blending by alpha
// comes out much the same in linear space, but adding doesn't: a colour
// added in gamma space to a mid-grey scene adds about three times as much
// as the same colour does in linear space, and no more than it takes to
// reach white. Additive sprites add that.

#import bevy_pbr::{
    forward_io::VertexOutput,
    mesh_functions,
    mesh_view_bindings::view,
    view_transformations::position_world_to_clip,
}
#ifdef DEPTH_PREPASS
#import bevy_pbr::prepass_utils::prepass_depth
#endif

// Additive sprites: their linear colour's gain, and the most one adds (what
// takes mid-grey, 0.45 in gamma space, to white).
const ADDITIVE_GAIN: f32 = 3.0;
const ADDITIVE_MAX: f32 = 0.83;

// x: 1 for additive, 0 for alpha blended; y: brightness; z: feather
// distance in metres (0: none).
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var color_sampler: sampler;

// A batch's vertices (`crate::fx::FxVertex`): with `FX_PULL` the vertex
// shader reads them here, by the number its mesh's position holds, in place
// of the mesh's own (rebuilt every frame, that cost the render thread).
struct FxVertex {
    // xyz: relative to the batch; w: the quad's ball radius.
    pos: vec4<f32>,
    // xy: uv; zw: the corner's place across the quad.
    uv_corner: vec4<f32>,
    color: vec4<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<storage, read> verts: array<FxVertex>;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) uv_b: vec2<f32>,
    @location(4) tangent: vec4<f32>,
    @location(5) color: vec4<f32>,
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
#ifdef FX_PULL
    let v = verts[u32(vertex.position.x + 0.5)];
    let local = v.pos.xyz;
    let uv = v.uv_corner.xy;
    let uv_b = v.uv_corner.zw;
    let radius = v.pos.w;
    let color = v.color;
#else
    let local = vertex.position;
    let uv = vertex.uv;
    let uv_b = vertex.uv_b;
    let radius = vertex.tangent.w;
    let color = vertex.color;
#endif
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4(local, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);
    out.world_normal = vec3(0.0, 0.0, 1.0);
#ifdef VERTEX_UVS_A
    out.uv = uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = uv_b;
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = vec4(1.0, 0.0, 0.0, radius);
#endif
#ifdef VERTEX_COLORS
    out.color = color;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var c = textureSample(color_texture, color_sampler, in.uv);
#ifdef VERTEX_COLORS
    c = c * in.color;
#endif
    var a = c.a;
#ifdef DEPTH_PREPASS
    // Reverse-Z infinite perspective: view distance = near / depth.
    let near = view.clip_from_view[3][2];
    let scene = prepass_depth(in.position, 0u);
    var ball = 0.0;
#ifdef VERTEX_TANGENTS
#ifdef VERTEX_UVS_B
    // Blended sprites as balls (`crate::fx::quads_mesh`): as much of the ball
    // shows as lies between the eye and what's behind it, so smoke thins
    // smoothly where walls and floors pass through it (a flat sprite cut by
    // a wall left the wall's outline in the smoke) and as the eye enters it.
    ball = select(0.0, in.world_tangent.w, params.x < 0.5);
    if (ball > 0.0) {
        let d = near / in.position.z;
        let h = ball * sqrt(max(1.0 - dot(in.uv_b, in.uv_b), 0.0));
        let behind = select(1e9, near / scene, scene > 0.0);
        let shown = min(behind, d + h) - max(d - h, near);
        a = a * select(step(d, behind), saturate(shown / (2.0 * h)), h > 1e-4);
    }
#endif
#endif
    if (ball <= 0.0 && params.z > 0.0 && scene > 0.0) {
        a = a * saturate((near / scene - near / in.position.z) / params.z);
    }
#endif
    if (params.x > 0.5) {
        let added = min(c.rgb * a * ADDITIVE_GAIN, vec3<f32>(ADDITIVE_MAX));
        return vec4<f32>(added * params.y * view.exposure, 0.0);
    }
    return vec4<f32>(c.rgb * a * params.y * view.exposure, a);
}
