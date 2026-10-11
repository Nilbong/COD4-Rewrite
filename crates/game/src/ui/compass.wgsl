// The minimap's map (`hud.menu`'s `mini_map`): the map image turned so the
// player faces up, centred on the player, clipped to the compass square.

#import bevy_ui::ui_vertex_output::UiVertexOutput

// xy: the player's place in the image (0..1), zw: image units per compass
// half-width.
@group(1) @binding(0) var<uniform> center_scale: vec4<f32>;
// xy: the player's facing in image space (x east, y south), z: alpha.
@group(1) @binding(1) var<uniform> dir_alpha: vec4<f32>;
@group(1) @binding(2) var map_texture: texture_2d<f32>;
@group(1) @binding(3) var map_sampler: sampler;
// x: roundness (0 square, 1 circle), y: outline width (share of the half
// size), z: fill behind the image (alpha), w: pixels per half size.
@group(1) @binding(4) var<uniform> shape: vec4<f32>;
@group(1) @binding(5) var<uniform> outline_color: vec4<f32>;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    // Compass space: -1..1, y down; up is the player's facing.
    let p = in.uv * 2.0 - 1.0;
    let d = dir_alpha.xy;
    let off = vec2<f32>(-p.x * d.y - p.y * d.x, p.x * d.x - p.y * d.y);
    let uv = center_scale.xy + off * center_scale.zw;
    let c = textureSample(map_texture, map_sampler, uv);
    let inside = all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0));
    let image = select(vec4<f32>(0.0), vec4<f32>(c.rgb, c.a * dir_alpha.z), inside);
    // The map's image over its black fill.
    let alpha = image.a + shape.z * (1.0 - image.a);
    var color = vec4<f32>(image.rgb * image.a / max(alpha, 1e-4), alpha);
    // Inside the (rounded) square: its signed distance, in half sizes.
    let r = shape.x;
    let q = abs(p) - vec2<f32>(1.0 - r);
    let sd = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
    let aa = 1.0 / max(shape.w, 1.0);
    // The outline: a band just inside the edge.
    let band = clamp((sd + shape.y) / aa + 0.5, 0.0, 1.0) * step(1e-5, shape.y) * outline_color.a;
    color = vec4<f32>(mix(color.rgb, outline_color.rgb, band), mix(color.a, 1.0, band));
    let cover = clamp(0.5 - sd / aa, 0.0, 1.0);
    return vec4<f32>(color.rgb, color.a * cover);
}
