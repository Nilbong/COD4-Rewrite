// The material-class layer (`crate::material_class`): what CoD4's low-res
// textures can't carry, worked out from each surface's CoD4 surface type
// (`surfaceTypeBits`: concrete, brick, asphalt, metal...), in world space so
// it never tiles with the texture:
// - anti-tiling: two octaves of large-scale noise varying the colour and
//   roughness, so repeated textures stop reading as a grid at range;
// - detail normals: fine bumps by class (concrete pitting, asphalt
//   aggregate, plaster stipple...), fading out by ~8 m;
// - roughness by class where CoD4 gave no specular map (the sun's
//   highlight shows what a surface is made of), a little variation where
//   it did;
// - specular anti-aliasing: roughness widened where the normal turns
//   fast across a pixel (Kaplanyan & Hoffman), against sparkle.
#define_import_path cod4rw::material_class

#import bevy_pbr::pbr_types::PbrInput
#import bevy_pbr::mesh_view_bindings::view

fn mc_hash(p: vec3<f32>) -> f32 {
    let q = fract(p * vec3(0.1031, 0.1030, 0.0973));
    let r = q + dot(q, q.yxz + 33.33);
    return fract((r.x + r.y) * r.z) * 2.0 - 1.0;
}

// Value noise (-1..1) and its gradient.
fn mc_noise(p: vec3<f32>) -> vec4<f32> {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    let du = 6.0 * f * (1.0 - f);
    let a = mc_hash(i);
    let b = mc_hash(i + vec3(1.0, 0.0, 0.0));
    let c = mc_hash(i + vec3(0.0, 1.0, 0.0));
    let d = mc_hash(i + vec3(1.0, 1.0, 0.0));
    let e = mc_hash(i + vec3(0.0, 0.0, 1.0));
    let g = mc_hash(i + vec3(1.0, 0.0, 1.0));
    let h = mc_hash(i + vec3(0.0, 1.0, 1.0));
    let k = mc_hash(i + vec3(1.0, 1.0, 1.0));
    let k0 = a;
    let k1 = b - a;
    let k2 = c - a;
    let k3 = e - a;
    let k4 = a - b - c + d;
    let k5 = a - c - e + h;
    let k6 = a - b - e + g;
    let k7 = -a + b + c - d + e - g - h + k;
    let v = k0 + k1 * u.x + k2 * u.y + k3 * u.z + k4 * u.x * u.y + k5 * u.y * u.z + k6 * u.z * u.x + k7 * u.x * u.y * u.z;
    let grad = du * vec3(
        k1 + k4 * u.y + k6 * u.z + k7 * u.y * u.z,
        k2 + k5 * u.z + k4 * u.x + k7 * u.z * u.x,
        k3 + k6 * u.x + k5 * u.y + k7 * u.x * u.y,
    );
    return vec4(v, grad);
}

// Per CoD4 surface type: detail bumps' frequency (per metre) and strength,
// and the roughness range without a specular map.
fn mc_class(c: u32) -> vec4<f32> {
    switch c {
        case 2u: { return vec4(9.0, 0.22, 0.80, 0.95); }   // brick
        case 5u: { return vec4(14.0, 0.30, 0.75, 0.95); }  // concrete
        case 6u, 14u, 18u: { return vec4(18.0, 0.35, 0.85, 1.0); } // dirt, mud, sand
        case 11u: { return vec4(20.0, 0.45, 0.85, 1.0); }  // gravel
        case 16u: { return vec4(10.0, 0.18, 0.80, 0.95); } // plaster
        case 17u: { return vec4(7.0, 0.35, 0.70, 0.95); }  // rock
        case 21u, 1u: { return vec4(6.0, 0.12, 0.60, 0.85); } // wood, bark
        case 22u: { return vec4(24.0, 0.40, 0.70, 0.92); } // asphalt
        case 13u: { return vec4(30.0, 0.05, 0.35, 0.60); } // metal
        case 28u: { return vec4(26.0, 0.07, 0.40, 0.65); } // painted metal
        case 23u: { return vec4(0.0, 0.0, 0.25, 0.45); }   // ceramic
        case 24u, 25u: { return vec4(12.0, 0.05, 0.45, 0.70); } // plastic, rubber
        case 9u, 12u, 20u: { return vec4(0.0, 0.0, 0.10, 0.25); } // glass, ice, water
        case 3u, 4u, 26u: { return vec4(16.0, 0.10, 0.90, 1.0); } // carpet, cloth, cushion
        default: { return vec4(10.0, 0.15, 0.75, 0.95); }
    }
}

// `surface`: x the class, y 1 when on, z detail strength, w anti-tiling
// amount. `spec_map`: the roughness is given already (CoD4's gloss, or a
// remastered roughness texture), only varied, not replaced.
fn material_class(pos: vec3<f32>, pbr_input: ptr<function, PbrInput>, surface: vec4<f32>, spec_map: bool, dn: f32) {
    let on = surface.y > 0.5;
    let c = u32(surface.x + 0.5);
    let k = mc_class(c);
    let dist = distance(pos, view.world_position);
    var rough = (*pbr_input).material.perceptual_roughness;
    if (on) {
        // Anti-tiling: large, soft variation (about 8 m and 2 m across).
        let m = mc_noise(pos * 0.13).x * 0.65 + mc_noise(pos * 0.47 + 17.0).x * 0.35;
        let glassy = c == 9u || c == 12u || c == 20u;
        let tint = select(1.0 + surface.w * m, 1.0, glassy);
        (*pbr_input).material.base_color = vec4((*pbr_input).material.base_color.rgb * tint, (*pbr_input).material.base_color.a);
        if (spec_map) {
            rough = clamp(rough + 0.06 * m, 0.04, 1.0);
        } else {
            rough = mix(k.z, k.w, m * 0.5 + 0.5);
        }
        // Detail normals: fine bumps by class, fading out with distance.
        let fade = (1.0 - smoothstep(3.0, 8.0, dist)) * surface.z * k.y;
        if (fade > 0.001 && k.x > 0.0) {
            let n = (*pbr_input).N;
            let g1 = mc_noise(pos * k.x);
            let g2 = mc_noise(pos * k.x * 2.7 + 5.0);
            let g = (g1.yzw + g2.yzw * 0.5) / k.x;
            let tangential = g - dot(g, n) * n;
            (*pbr_input).N = normalize(n - tangential * fade * k.x * 0.12);
        }
    }
    // Specular anti-aliasing (`dn`: how fast the normal turns per pixel).
    let widen = min(2.0 * dn, 0.18);
    (*pbr_input).material.perceptual_roughness = sqrt(sqrt(min(rough * rough * rough * rough + widen, 1.0)));
}
