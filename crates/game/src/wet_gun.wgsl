// Rain on the gun in hand (`wet_gun.rs`): a thin layer over each viewmodel
// surface, premultiplied: it darkens what's under it as a water film does,
// and adds the film's and the drops' reflections. Drops bead on what faces
// up (the gun's up as held), in cells fixed to the model (its bind pose), so they stay put as the
// gun moves; now and then one runs down the sides.

#import bevy_pbr::{
    mesh_bindings::mesh,
    mesh_functions,
    skinning,
    forward_io::Vertex,
    view_transformations::position_world_to_clip,
    pbr_types,
    pbr_functions,
}

// x: how wet, y: how hard it rains here, z: time (s), w: 1 the arms.
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: vec4<f32>;
// Strengths (`wetgun.*` in tuning.txt): x beads, y darkening, z shine, w
// running drops.
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> knobs: vec4<f32>;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    // The model's own frame (metres, bind pose) and, in it, the normal
    // and which way is down.
    @location(2) local_position: vec3<f32>,
    @location(3) local_normal: vec3<f32>,
    @location(4) local_down: vec3<f32>,
    @location(5) @interpolate(flat) instance_index: u32,
};

@vertex
fn vertex(vertex: Vertex) -> Out {
    var out: Out;
#ifdef SKINNED
    let world_from_local = skinning::skin_model(vertex.joint_indices, vertex.joint_weights, vertex.instance_index);
    out.world_normal = skinning::skin_normals(world_from_local, vertex.normal);
#else
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    out.world_normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
#endif
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);
    out.local_position = vertex.position;
    out.local_normal = vertex.normal;
    // Down as the gun's held level (models' up is +Y), not the world's:
    // the gun turns with the view, and drops placed by the world's up
    // came and went as it did, as if sliding over it.
    out.local_down = vec3<f32>(0.0, -1.0, 0.0);
    out.instance_index = vertex.instance_index;
    return out;
}

fn hash3(p: vec3<f32>) -> vec3<f32> {
    var q = fract(p * vec3<f32>(0.1031, 0.1030, 0.0973));
    q += dot(q, q.yxz + 33.33);
    return fract((q.xxy + q.yxx) * q.zyx);
}

// Smooth noise (0..1) over the model's space, for where drops gather and
// where a sleeve is soaked through.
fn noise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    var v = 0.0;
    for (var k = 0u; k < 8u; k++) {
        let c = vec3<f32>(f32(k & 1u), f32((k >> 1u) & 1u), f32((k >> 2u) & 1u));
        let w = mix(1.0 - u, u, c);
        v += hash3(i + c).x * w.x * w.y * w.z;
    }
    return v;
}

// The drop in `p`'s cell, if it has one: how far `p` is from its middle
// (1 at its edge), its offset (metres) and radius. `dn` is the way down
// the surface, `stretch` how long a drop runs along it (a side's drops
// sag into short streaks, heavier at the bottom).
struct Drop {
    d: f32,
    o: vec3<f32>,
    r: f32,
};

fn drop_in(p: vec3<f32>, cell: f32, density: f32, seed: f32, dn: vec3<f32>, stretch: f32) -> Drop {
    var out: Drop;
    out.d = 2.0;
    let g = p / cell;
    let id = floor(g);
    let r3 = hash3(id + seed);
    if r3.x >= density {
        return out;
    }
    // Mostly small, a few large.
    let radius = min(mix(0.1, 0.42, r3.y * r3.y * r3.y), 0.45 / stretch);
    let centre = id + 0.5 + (hash3(id + seed + 17.0) - 0.5) * max(1.0 - 2.0 * radius * stretch, 0.0);
    let o = (g - centre) * cell;
    let along = dot(o, dn);
    let perp = o - along * dn;
    let s = select(stretch * 0.6, stretch, along > 0.0);
    out.d = sqrt(dot(perp, perp) + (along / s) * (along / s)) / (radius * cell);
    out.o = o;
    out.r = radius * cell;
    return out;
}

@fragment
fn fragment(in: Out, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let wet = params.x;
    let rain = params.y;
    let arms = params.w > 0.5;
    if wet < 0.002 {
        discard;
    }
    var n = normalize(in.world_normal);
    if !front {
        n = -n;
    }
    let p = in.local_position;
    let ln = normalize(in.local_normal);
    // (The model's up, likewise.)
    let up = ln.y;
    let down = in.local_down - dot(in.local_down, ln) * ln;
    let sloped = clamp(length(down) * 1.5 - 0.2, 0.0, 1.0);
    let dn = select(normalize(cross(ln, vec3<f32>(0.0, 0.0, 1.0)) + 1e-4), normalize(down), length(down) > 0.05);

    // Beads (not on the arms: a sleeve soaks it up): two sizes, gathered
    // in patches, more the wetter and the more the surface faces up,
    // sagging into streaks down its sides; drying, they shrink and go.
    var d = Drop(2.0, vec3<f32>(0.0), 1.0);
    if !arms {
        let gather = smoothstep(0.3, 0.8, noise(p / 0.025)) * 1.7;
        let facing = smoothstep(0.0, 0.6, up) + 0.25 * (1.0 - abs(up));
        let density = wet * facing * gather * knobs.x;
        let stretch = 1.0 + 1.6 * sloped * (1.0 - smoothstep(0.3, 0.8, up));
        let size = 0.55 + 0.45 * wet;
        let small = drop_in(p, 0.0035 * size, density * 0.7, 0.0, dn, stretch);
        let large = drop_in(p, 0.012 * size, density * 0.4, 31.0, dn, stretch);
        d = small;
        if large.d < small.d {
            d = large;
        }
        // Now and then one running down a side: lanes across the way
        // down, each stretch of a lane with its own drop or none, a thin
        // wet trail above it.
        if rain > 0.0 && sloped > 0.3 {
            let side = normalize(cross(ln, dn));
            let width = 0.005;
            let span = 0.12;
            let across = dot(p, side) / width;
            let lane = floor(across);
            let lr = hash3(vec3<f32>(lane, 3.7, 9.1));
            let speed = mix(0.012, 0.03, lr.y);
            let along = (dot(p, dn) - params.z * speed) / span + lr.x;
            let live = hash3(vec3<f32>(lane, floor(along), 5.3)).x < 0.04 * rain * knobs.w * (1.0 - smoothstep(0.4, 0.8, up));
            if live {
                let u = fract(along) * span;
                let x = (fract(across) - 0.5) * width;
                let back = min(u, span - u);
                let o = x * side + select(-back, back, u < span * 0.5) * dn;
                let head = length(vec2<f32>(x, back * select(0.6, 1.0, u < span * 0.5))) / 0.0018;
                if head < d.d {
                    d = Drop(head, o, 0.0018);
                }
            }
        }
    }
    let inside = d.d < 1.0;

    // Lit as water: a dielectric with nothing of its own but what it
    // reflects (glossy on the gun, duller on cloth), looked up once along
    // the surface's own normal: the drops show it through their shape
    // below, not by bending it (which made white crescents).
    var pbr = pbr_types::pbr_input_new();
    pbr.material.base_color = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    pbr.material.metallic = 0.0;
    pbr.material.reflectance = vec3<f32>(0.35);
    pbr.material.perceptual_roughness = select(0.14, 0.5, arms);
    pbr.frag_coord = in.position;
    pbr.world_position = in.world_position;
    pbr.world_normal = n;
    pbr.N = n;
    pbr.V = pbr_functions::calculate_view(in.world_position, false);
    pbr.flags = mesh[in.instance_index].flags;
    let env = pbr_functions::apply_pbr_lighting(pbr).rgb;

    // The film: darker, with a faint sheen; a sleeve darker still, in
    // blotches where it's soaked through.
    var shade = wet * 0.24 * (0.6 + 0.4 * max(up, 0.0));
    var light = env * wet * 0.3;
    if arms {
        let soaked = mix(0.55, 1.0, smoothstep(0.25, 0.75, noise(p / 0.04)));
        shade = wet * 0.38 * soaked;
        light = env * wet * 0.18 * soaked;
    }
    shade *= knobs.y;
    light *= knobs.z;
    if inside {
        // A bead: clear in the middle, a dark rim where it bends the light
        // away, a small highlight on the sky's side and a fainter one (the
        // light it focuses) on the far side.
        let rim = smoothstep(0.62, 0.92, d.d) * (1.0 - smoothstep(0.92, 1.0, d.d));
        let sky = mix(vec3<f32>(0.0), -dn, sloped);
        let hl = d.o / d.r - sky * 0.45;
        let spot = 1.0 - smoothstep(0.1, 0.24, length(hl - dot(hl, ln) * ln));
        let caustic = 1.0 - smoothstep(0.12, 0.35, length(d.o / d.r + sky * 0.5));
        shade = max(shade, 0.08) + 0.6 * rim;
        light += env * (4.0 * spot + 0.35 * caustic * (1.0 - rim)) * knobs.z;
    }
    return vec4<f32>(light, clamp(shade, 0.0, 0.85));
}
