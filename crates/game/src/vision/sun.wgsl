// The sun's lens flare (and glare), added to the screen like CoD4's `2d`
// materials with ONE/ONE blending.

#import bevy_ui::ui_vertex_output::UiVertexOutput

// rgb: tint; a: strength.
@group(1) @binding(0) var<uniform> color: vec4<f32>;
@group(1) @binding(1) var flare_texture: texture_2d<f32>;
@group(1) @binding(2) var flare_sampler: sampler;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let c = textureSample(flare_texture, flare_sampler, in.uv).rgb;
    return vec4<f32>(c * color.rgb * color.a, 1.0);
}
