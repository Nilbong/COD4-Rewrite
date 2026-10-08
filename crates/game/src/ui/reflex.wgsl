// A red dot sight's dot (IW3's `mc_reflexsight`, from its shaders): the
// dot texture laid over the view directions around the lens's normal, so
// the dot sits on the sight line wherever the eye is and the rest of the
// lens stays clear, times a grain texture on the lens. Or a custom
// reticle (`crate::reticles`): a shape drawn here, crisp at any
// resolution, in its colour.

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
// x: the reticle's shape (0: the dot texture), y: its size.
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var<uniform> style: vec4<f32>;
// rgb: its colour.
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var<uniform> tint: vec4<f32>;

// How much brighter than the 8-bit original the dot is drawn in the HDR
// scene (auto-exposure and tonemapping bring it back down).
const DOT_BRIGHTNESS: f32 = 14.0;
// And how much bigger: IW3's is a pinpoint at today's resolutions.
const DOT_SIZE: f32 = 1.35;
// A custom reticle's brightness, likewise.
const SHAPE_BRIGHTNESS: f32 = 4.0;

// How much of a bar from a to b (half-width w) covers p, `px` a pixel.
fn bar(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, w: f32, px: f32) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / dot(ab, ab), 0.0, 1.0);
    return clamp(1.0 - max(length(p - (a + ab * t)) - w, 0.0) / px, 0.0, 1.0);
}

fn disc(p: vec2<f32>, r: f32, px: f32) -> f32 {
    return clamp(1.0 - max(length(p) - r, 0.0) / px, 0.0, 1.0);
}

fn ring(p: vec2<f32>, r: f32, w: f32, px: f32) -> f32 {
    return clamp(1.0 - max(abs(length(p) - r) - w, 0.0) / px, 0.0, 1.0);
}

// The shapes (as `crate::reticles::coverage`), over -1..1 of the dot
// texture's span, y up.
fn shape_coverage(shape: i32, p: vec2<f32>, px: f32) -> f32 {
    switch shape {
        case 2: {
            return max(max(max(bar(p, vec2(-0.45, 0.0), vec2(-0.1, 0.0), 0.035, px), bar(p, vec2(0.1, 0.0), vec2(0.45, 0.0), 0.035, px)),
                max(bar(p, vec2(0.0, -0.45), vec2(0.0, -0.1), 0.035, px), bar(p, vec2(0.0, 0.1), vec2(0.0, 0.45), 0.035, px))), disc(p, 0.035, px));
        }
        case 3: {
            return max(ring(p, 0.55, 0.03, px), disc(p, 0.07, px));
        }
        case 4: {
            return max(bar(p, vec2(-0.3, -0.25), vec2(0.0), 0.035, px), bar(p, vec2(0.3, -0.25), vec2(0.0), 0.035, px));
        }
        case 5: {
            return max(max(max(ring(p, 0.6, 0.03, px), bar(p, vec2(-0.85, 0.0), vec2(-0.6, 0.0), 0.03, px)),
                max(bar(p, vec2(0.6, 0.0), vec2(0.85, 0.0), 0.03, px), bar(p, vec2(0.0, -0.85), vec2(0.0, -0.6), 0.03, px))), disc(p, 0.06, px));
        }
        case 6: {
            return max(bar(p, vec2(-0.4, 0.0), vec2(0.4, 0.0), 0.035, px), bar(p, vec2(0.0), vec2(0.0, -0.45), 0.035, px));
        }
        default: {
            return disc(p, 0.09, px);
        }
    }
}

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
    let size = max(style.y, 0.1);
    let uv = vec2<f32>(0.5) + vec2<f32>(dot(v, b), dot(v, t)) * scale.xy / (DOT_SIZE * size);
    let inside = all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0));
    // CoD4's dot texture, or a shape (on the lens, uv.x is up and uv.y
    // left: `b` and `t`).
    let p = (uv - vec2<f32>(0.5)) * 2.0;
    let shape = i32(style.x + 0.5);
    let px = max(length(fwidth(p)), 1e-4);
    let drawn = select(shape_coverage(shape, vec2<f32>(-p.y, p.x), px), textureSample(dot_texture, dot_sampler, uv).r, shape == 0);
    let g = select(0.0, drawn, inside || shape != 0) * textureSample(grain_texture, grain_sampler, in.uv).r;
    // As IW3 writes it to an 8-bit target: a white-hot core in a glow of
    // its colour (CoD4's red by default), blended ONE / INVSRCALPHA
    // (premultiplied).
    let core = 1.2 * (g - 0.2);
    var colour = clamp(vec4<f32>(tint.rgb * 2.0 * g + (vec3<f32>(1.0) - tint.rgb) * core, 1.2 * g), vec4<f32>(0.0), vec4<f32>(1.0));
    var brightness = DOT_BRIGHTNESS;
    if shape != 0 {
        // A shape: its colour, pure (a white-hot core and the dot's
        // brightness came out of tonemapping all but white).
        colour = vec4<f32>(tint.rgb * g, g);
        brightness = SHAPE_BRIGHTNESS;
    }
    // Here it goes into the HDR scene, which auto-exposure then scales down
    // (about -2.5 EV by day): lit like an emitter, so it reaches the screen
    // as bright as IW3's and stays readable against a bright sky.
    return vec4<f32>(colour.rgb * brightness, colour.a);
}
