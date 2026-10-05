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

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    // Compass space: -1..1, y down; up is the player's facing.
    let p = in.uv * 2.0 - 1.0;
    let d = dir_alpha.xy;
    let off = vec2<f32>(-p.x * d.y - p.y * d.x, p.x * d.x - p.y * d.y);
    let uv = center_scale.xy + off * center_scale.zw;
    let c = textureSample(map_texture, map_sampler, uv);
    let inside = all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0));
    return select(vec4<f32>(0.0), vec4<f32>(c.rgb, c.a * dir_alpha.z), inside);
}
