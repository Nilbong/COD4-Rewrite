// A lens scope (`ui::scope`'s Lens style): the scope camera's magnified view
// in a circle, with the scope's own picture (its reticle and rim) over it,
// darkening towards the edge.

#import bevy_ui::ui_vertex_output::UiVertexOutput

// x: 1 to draw the scope's picture, y: how far in the lens has faded.
@group(1) @binding(0) var<uniform> params: vec4<f32>;
@group(1) @binding(1) var view_texture: texture_2d<f32>;
@group(1) @binding(2) var view_sampler: sampler;
@group(1) @binding(3) var overlay_texture: texture_2d<f32>;
@group(1) @binding(4) var overlay_sampler: sampler;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let d = distance(in.uv, vec2<f32>(0.5));
    if d > 0.5 {
        return vec4<f32>(0.0);
    }
    var c = textureSample(view_texture, view_sampler, in.uv).rgb;
    let overlay = textureSample(overlay_texture, overlay_sampler, in.uv);
    c = mix(c, overlay.rgb, overlay.a * params.x);
    // The lens's edge: a dark rim, and a soft cut against the world.
    c *= 1.0 - 0.8 * smoothstep(0.42, 0.5, d);
    let edge = 1.0 - smoothstep(0.492, 0.5, d);
    return vec4<f32>(c, edge * params.y);
}
