// The Upscaler setting (`render_scale.rs`): the world, drawn into the
// frame's top left at `share` of its size, brought up to the full frame in
// two passes after the method of AMD's FSR 1:
// - `easu`: an edge-adaptive upscale. Twelve source texels around the
//   output pixel are weighted with a Lanczos-like window stretched along
//   the local edge (found from the luma gradients of the four nearest), so
//   edges stay sharp instead of going soft as in a bilinear stretch; the
//   result is held inside the four nearest texels (no ringing).
// - `rcas`: sharpening of that upscale, limited per pixel so it can't push
//   past its neighbours (no halos).
// Both work on a reversibly tonemapped copy (x / (1 + max(x))) of the
// linear HDR frame, as the filters expect values in 0..1; `rcas` maps it
// back.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
// xy: the share of the frame drawn; z: sharpening (0 none .. 1 most).
@group(0) @binding(2) var<uniform> params: vec4<f32>;

fn squash(c: vec3<f32>) -> vec3<f32> {
    return c / (1.0 + max(max(c.r, c.g), c.b));
}

fn unsquash(c: vec3<f32>) -> vec3<f32> {
    return c / max(1.0 - max(max(c.r, c.g), c.b), 1.0 / 1024.0);
}

fn luma(c: vec3<f32>) -> f32 {
    return 0.5 * c.g + 0.25 * (c.r + c.b);
}

// A texel of the drawn part (clamped to it), squashed.
fn fetch(p: vec2<i32>, hi: vec2<i32>) -> vec3<f32> {
    return squash(max(textureLoad(source, clamp(p, vec2(0), hi), 0).rgb, vec3(0.0)));
}

// One of the four nearest texels' share of the edge direction and strength:
// its left/right and up/down neighbours' differences, weighted `w`.
fn edge(l: f32, c: f32, r: f32, u: f32, d: f32, w: f32, acc: ptr<function, vec3<f32>>) {
    let dx = r - l;
    let dy = d - u;
    // How one-sided the change is: 1 for a clean edge, 0 for a ridge/noise.
    let lx = clamp(abs(dx) / max(max(abs(r - c), abs(c - l)), 1.0 / 32768.0), 0.0, 1.0);
    let ly = clamp(abs(dy) / max(max(abs(d - c), abs(c - u)), 1.0 / 32768.0), 0.0, 1.0);
    (*acc) += vec3(dx * w, dy * w, 0.5 * (lx * lx + ly * ly) * w);
}

// A tap's weight: its offset from the output point, turned onto the edge
// and stretched by `len2`, into the Lanczos-2-like window.
fn tap(off: vec2<f32>, dir: vec2<f32>, len2: vec2<f32>, lob: f32, clp: f32, c: vec3<f32>, acc: ptr<function, vec4<f32>>) {
    var v = vec2(off.x * dir.x + off.y * dir.y, off.y * dir.x - off.x * dir.y);
    v *= len2;
    let d2 = min(dot(v, v), clp);
    var base = 0.4 * d2 - 1.0;
    var win = lob * d2 - 1.0;
    base *= base;
    win *= win;
    let w = (25.0 / 16.0 * base - (25.0 / 16.0 - 1.0)) * win;
    (*acc) += vec4(c * w, w);
}

@fragment
fn easu(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(source));
    let hi = vec2<i32>(params.xy * dims) - vec2(1);
    // The output pixel in the drawn part's texels.
    let ip = in.uv * params.xy * dims - 0.5;
    let fp = floor(ip);
    let pp = ip - fp;
    let o = vec2<i32>(fp);
    //    b c
    //  e f g h
    //  i j k l
    //    n o
    let b = fetch(o + vec2(0, -1), hi);
    let c = fetch(o + vec2(1, -1), hi);
    let e = fetch(o + vec2(-1, 0), hi);
    let f = fetch(o, hi);
    let g = fetch(o + vec2(1, 0), hi);
    let h = fetch(o + vec2(2, 0), hi);
    let i = fetch(o + vec2(-1, 1), hi);
    let j = fetch(o + vec2(0, 1), hi);
    let k = fetch(o + vec2(1, 1), hi);
    let l = fetch(o + vec2(2, 1), hi);
    let n = fetch(o + vec2(0, 2), hi);
    let oo = fetch(o + vec2(1, 2), hi);
    let lb = luma(b);
    let lc = luma(c);
    let le = luma(e);
    let lf = luma(f);
    let lg = luma(g);
    let lh = luma(h);
    let li = luma(i);
    let lj = luma(j);
    let lk = luma(k);
    let ll = luma(l);
    let ln = luma(n);
    let lo = luma(oo);

    // The edge, bilinearly from the four nearest texels' views of it.
    var dl = vec3(0.0);
    edge(le, lf, lg, lb, lj, (1.0 - pp.x) * (1.0 - pp.y), &dl);
    edge(lf, lg, lh, lc, lk, pp.x * (1.0 - pp.y), &dl);
    edge(li, lj, lk, lf, ln, (1.0 - pp.x) * pp.y, &dl);
    edge(lj, lk, ll, lg, lo, pp.x * pp.y, &dl);
    var dir = dl.xy;
    let d2 = dot(dir, dir);
    if (d2 < 1.0 / 32768.0) {
        dir = vec2(1.0, 0.0);
    } else {
        dir *= inverseSqrt(d2);
    }
    let len = dl.z * dl.z;
    // Stretched along diagonal edges, narrowed across strong ones.
    let stretch = dot(dir, dir) / max(abs(dir.x), abs(dir.y));
    let len2 = vec2(1.0 + (stretch - 1.0) * len, 1.0 - 0.5 * len);
    let lob = 0.5 + (0.25 - 0.04 - 0.5) * len;
    let clp = 1.0 / lob;

    var acc = vec4(0.0);
    tap(vec2(0.0, -1.0) - pp, dir, len2, lob, clp, b, &acc);
    tap(vec2(1.0, -1.0) - pp, dir, len2, lob, clp, c, &acc);
    tap(vec2(-1.0, 1.0) - pp, dir, len2, lob, clp, i, &acc);
    tap(vec2(0.0, 1.0) - pp, dir, len2, lob, clp, j, &acc);
    tap(vec2(0.0, 0.0) - pp, dir, len2, lob, clp, f, &acc);
    tap(vec2(-1.0, 0.0) - pp, dir, len2, lob, clp, e, &acc);
    tap(vec2(1.0, 1.0) - pp, dir, len2, lob, clp, k, &acc);
    tap(vec2(2.0, 1.0) - pp, dir, len2, lob, clp, l, &acc);
    tap(vec2(2.0, 0.0) - pp, dir, len2, lob, clp, h, &acc);
    tap(vec2(1.0, 0.0) - pp, dir, len2, lob, clp, g, &acc);
    tap(vec2(1.0, 2.0) - pp, dir, len2, lob, clp, oo, &acc);
    tap(vec2(0.0, 2.0) - pp, dir, len2, lob, clp, n, &acc);
    let lo4 = min(min(f, g), min(j, k));
    let hi4 = max(max(f, g), max(j, k));
    let col = clamp(acc.rgb / max(acc.w, 1.0 / 32768.0), lo4, hi4);
    // Still squashed: `rcas` maps it back.
    return vec4(col, 1.0);
}

@fragment
fn rcas(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(source));
    let p = vec2<i32>(in.position.xy);
    let hi = dims - vec2(1);
    let c = textureLoad(source, p, 0).rgb;
    let n = textureLoad(source, clamp(p + vec2(0, -1), vec2(0), hi), 0).rgb;
    let s = textureLoad(source, clamp(p + vec2(0, 1), vec2(0), hi), 0).rgb;
    let w = textureLoad(source, clamp(p + vec2(-1, 0), vec2(0), hi), 0).rgb;
    let e = textureLoad(source, clamp(p + vec2(1, 0), vec2(0), hi), 0).rgb;
    if (params.z <= 0.0) {
        return vec4(unsquash(c), 1.0);
    }
    // The most negative lobe that keeps the result within 0..1 given the
    // neighbours' range, per channel; the least of the three wins.
    let mn = min(min(n, s), min(w, e));
    let mx = max(max(n, s), max(w, e));
    let hit_min = mn / max(4.0 * mx, vec3(1.0 / 32768.0));
    let hit_max = (1.0 - mx) / min(4.0 * mn - 4.0, vec3(-1.0 / 32768.0));
    let lobes = max(-hit_min, hit_max);
    let lobe = max(-0.1875, min(max(max(lobes.r, lobes.g), lobes.b), 0.0)) * params.z;
    let col = (lobe * (n + s + w + e) + c) / (4.0 * lobe + 1.0);
    return vec4(unsquash(clamp(col, vec3(0.0), vec3(0.999))), 1.0);
}
