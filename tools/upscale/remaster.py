"""Remaster a map's textures: AI upscaling plus better material maps.

    python -I tools/upscale/remaster.py <image list> <out dir> [--esrgan <exe>] [--max 2048] [--only name,...]

The image list is `cargo run -p iw3 --example mapimages <map>` output:
`# main <CoD4 main dir>` then `<semantic> <image> <material> <user>` lines.
Reads the install's .iwd archives only (never writes there). Writes, per
image, CoD4 v6 .iwi files the game loads in place of the originals
(`crates/game/src/upscaled.rs`, `COD4RW_UPSCALED=1` / knob `upscale.on`):

  <name>.iwi          colour (Real-ESRGAN x4plus; BC1, BC3 with alpha),
                      normal (bicubic + sharpen in IW3's slope packing, BC3)
                      or specular (bicubic + sharpen, BC3)
  <name>.rough.iwi    colour maps: roughness in green, from the material's
                      specular gloss where it has one, else from the colour (BC1)
  <name>.gen_n.iwi    colour maps: a normal map made from the colour, for
                      materials that have none (BC3, slope packing)
  <name>.height.iwi   normal maps and colour maps: heights for parallax (BC1)

All filters wrap round, as the textures tile; Real-ESRGAN gets a wrapped
border that's cut off afterwards so tiling seams stay invisible. Colour maps
are 4x, capped at --max a side; normal, specular, roughness and height maps
at half that cap. Heights and made-up normal maps only for world surfaces
(props get colour, normal, specular and roughness). Lower priority than the user's programs, small
GPU tiles.
"""

import os
import struct
import subprocess
import sys
import zipfile
from io import BytesIO

import numpy as np
from PIL import Image
from scipy import ndimage

PAD = 16  # wrapped border given to Real-ESRGAN (source texels)


# --------------------------------------------------------------------------
# Reading the install

def mount(main_dir):
    """images/<name>.iwi (lower case) -> (archive path, member); later archives win."""
    files = {}
    for iwd in sorted(f for f in os.listdir(main_dir) if f.lower().endswith(".iwd")):
        path = os.path.join(main_dir, iwd)
        with zipfile.ZipFile(path) as z:
            for n in z.namelist():
                files[n.lower().replace("\\", "/")] = (path, n)
    return files


def read_iwi(files, name):
    hit = files.get(f"images/{name.lstrip(',')}.iwi".lower())
    if not hit:
        return None
    with zipfile.ZipFile(hit[0]) as z:
        return z.read(hit[1])


FORMATS = {0x01: "argb32", 0x02: "rgb24", 0x03: "la16", 0x04: "a8", 0x05: "l8", 0x0B: "dxt1", 0x0C: "dxt3", 0x0D: "dxt5"}


def level_size(fmt, w, h):
    w, h = max(w, 1), max(h, 1)
    if fmt == "dxt1":
        return ((w + 3) // 4) * ((h + 3) // 4) * 8
    if fmt in ("dxt3", "dxt5"):
        return ((w + 3) // 4) * ((h + 3) // 4) * 16
    return w * h * {"argb32": 4, "rgb24": 3, "la16": 2, "l8": 1, "a8": 1}[fmt]


def decode_iwi(data):
    """Top level as float RGBA 0..1 (h, w, 4), or None."""
    if data[:3] != b"IWi" or data[3] != 6:
        return None
    fmt = FORMATS.get(data[4])
    flags = data[5]
    w, h, depth = struct.unpack_from("<HHH", data, 6)
    if fmt is None or flags & 0x0C or depth > 1:
        return None
    top = data[len(data) - level_size(fmt, w, h):]
    if fmt.startswith("dxt"):
        dds = dds_header(w, h, fmt) + top
        img = Image.open(BytesIO(dds)).convert("RGBA")
        return np.asarray(img, dtype=np.float32) / 255.0
    a = np.frombuffer(top, dtype=np.uint8)
    if fmt == "argb32":
        a = a.reshape(h, w, 4)[..., [2, 1, 0, 3]]
    elif fmt == "rgb24":
        a = np.concatenate([a.reshape(h, w, 3)[..., ::-1], np.full((h, w, 1), 255, np.uint8)], 2)
    elif fmt == "la16":
        a = a.reshape(h, w, 2)
        a = np.stack([a[..., 0]] * 3 + [a[..., 1]], 2)
    elif fmt == "l8":
        a = a.reshape(h, w)
        a = np.stack([a] * 3 + [np.full_like(a, 255)], 2)
    else:  # a8
        a = a.reshape(h, w)
        a = np.stack([np.full_like(a, 255)] * 3 + [a], 2)
    return a.astype(np.float32) / 255.0


def dds_header(w, h, fmt):
    four = {"dxt1": b"DXT1", "dxt3": b"DXT3", "dxt5": b"DXT5"}[fmt]
    pf = struct.pack("<II4sIIIII", 32, 0x4, four, 0, 0, 0, 0, 0)
    hdr = struct.pack("<IIIIIII", 124, 0x1007 | 0x80000, h, w, level_size(fmt, w, h), 0, 1) + b"\0" * 44 + pf
    hdr += struct.pack("<IIIII", 0x1000, 0, 0, 0, 0)
    return b"DDS " + hdr


# --------------------------------------------------------------------------
# BC1 / BC3 encoding (principal-axis endpoints, nearest palette entry)

def blocks_of(img):
    """(h, w, c) -> (n, 16, c), padding to 4x4 by repeating edges."""
    h, w, c = img.shape
    ph, pw = (-h) % 4, (-w) % 4
    if ph or pw:
        img = np.pad(img, ((0, ph), (0, pw), (0, 0)), mode="edge")
    h, w = img.shape[:2]
    return img.reshape(h // 4, 4, w // 4, 4, c).transpose(0, 2, 1, 3, 4).reshape(-1, 16, c)


def to565(c):
    q = np.round(c * [31, 63, 31]).astype(np.int32)
    q = np.clip(q, 0, [31, 63, 31])
    return (q[..., 0] << 11) | (q[..., 1] << 5) | q[..., 2], q / np.array([31, 63, 31], np.float32)


def encode_colour(blocks):
    """(n, 16, 3) 0..1 -> (n, 8) uint8 BC1 colour blocks, always 4-colour."""
    mean = blocks.mean(1, keepdims=True)
    d = blocks - mean
    cov = np.einsum("nki,nkj->nij", d, d)
    v = np.ones((len(blocks), 3), np.float32)
    for _ in range(8):
        v = np.einsum("nij,nj->ni", cov, v)
        v /= np.maximum(np.linalg.norm(v, axis=1, keepdims=True), 1e-12)
    t = np.einsum("nki,ni->nk", d, v)
    lo = mean[:, 0] + v * t.min(1, keepdims=True)
    hi = mean[:, 0] + v * t.max(1, keepdims=True)
    c0, f0 = to565(np.clip(hi, 0, 1))
    c1, f1 = to565(np.clip(lo, 0, 1))
    # 4-colour mode needs c0 > c1.
    swap = c0 < c1
    c0, c1 = np.where(swap, c1, c0), np.where(swap, c0, c1)
    f0, f1 = np.where(swap[:, None], f1, f0), np.where(swap[:, None], f0, f1)
    pal = np.stack([f0, f1, (2 * f0 + f1) / 3, (f0 + 2 * f1) / 3], 1)  # (n, 4, 3)
    dist = ((blocks[:, :, None, :] - pal[:, None, :, :]) ** 2).sum(-1)
    idx = dist.argmin(2).astype(np.uint32)
    idx[c0 == c1] = 0
    bits = (idx << (2 * np.arange(16, dtype=np.uint32))).sum(1).astype(np.uint32)
    out = np.zeros((len(blocks), 8), np.uint8)
    out[:, 0:2] = np.stack([c0 & 0xFF, c0 >> 8], 1)
    out[:, 2:4] = np.stack([c1 & 0xFF, c1 >> 8], 1)
    out[:, 4:8] = bits.view(np.uint8).reshape(-1, 4)
    return out


def encode_alpha(blocks):
    """(n, 16) 0..1 -> (n, 8) BC3 alpha blocks (8-value mode)."""
    a = np.round(blocks * 255)
    a0 = a.max(1)
    a1 = a.min(1)
    a1 = np.where(a0 == a1, np.maximum(a0 - 1, 0), a1)
    a0 = np.where(a0 == a1, a1 + 1, a0)
    pal = np.stack([a0, a1] + [((7 - i) * a0 + i * a1) / 7 for i in range(1, 7)], 1)
    idx = np.abs(a[:, :, None] - pal[:, None, :]).argmin(2).astype(np.uint64)
    bits = (idx << (3 * np.arange(16, dtype=np.uint64))).sum(1)
    out = np.zeros((len(blocks), 8), np.uint8)
    out[:, 0] = a0
    out[:, 1] = a1
    for i in range(6):
        out[:, 2 + i] = (bits >> np.uint64(8 * i)) & np.uint64(0xFF)
    return out


def mips(img):
    """Full chain down to 1x1, box filtered (wraps: textures tile)."""
    out = [img]
    while img.shape[0] > 1 or img.shape[1] > 1:
        h, w = img.shape[:2]
        nh, nw = max(h // 2, 1), max(w // 2, 1)
        img = img[: nh * (h // nh), : nw * (w // nw)].reshape(nh, h // nh, nw, w // nw, -1).mean((1, 3))
        out.append(img)
    return out


def write_iwi(path, img, alpha, flags=0):
    """img (h, w, 4) float 0..1 -> CoD4 v6 iwi, DXT5 if `alpha` else DXT1, all mips."""
    h, w = img.shape[:2]
    fmt = 0x0D if alpha else 0x0B
    levels = []
    for m in mips(img):
        b = blocks_of(m)
        parts = []
        for i in range(0, len(b), 32768):
            c = b[i : i + 32768]
            colour = encode_colour(c[..., :3])
            parts.append(np.concatenate([encode_alpha(c[..., 3]), colour], 1) if alpha else colour)
        levels.append(np.concatenate(parts).tobytes())
    body = b"".join(reversed(levels))  # smallest first
    header = b"IWi" + bytes([6, fmt, flags]) + struct.pack("<HHH", w, h, 1)
    total = 28 + len(body)
    header += struct.pack("<IIII", total, total - len(levels[0]), total - len(levels[0]) - len(levels[1]) if len(levels) > 1 else 28, 28)
    tmp = path + ".tmp"
    with open(tmp, "wb") as f:
        f.write(header + body)
    os.replace(tmp, path)
    return total


# --------------------------------------------------------------------------
# Image helpers (all wrap round)

def resize(ch, h, w):
    """One channel (float) to (h, w), bicubic, wrapping round."""
    sh, sw = ch.shape
    p = 4
    big = np.pad(ch, p, mode="wrap")
    k = (w / sw, h / sh)
    img = Image.fromarray(big.astype(np.float32), mode="F").resize((round(big.shape[1] * k[0]), round(big.shape[0] * k[1])), Image.BICUBIC)
    a = np.asarray(img)
    oy, ox = round(p * k[1]), round(p * k[0])
    return a[oy : oy + h, ox : ox + w].copy()


def box_down(img, h, w):
    """Average down by whole factors."""
    fh, fw = img.shape[0] // h, img.shape[1] // w
    if fh == 1 and fw == 1:
        return img
    return img[: h * fh, : w * fw].reshape(h, fh, w, fw, *img.shape[2:]).mean((1, 3))


def sharpen(ch, sigma, amount):
    return ch + amount * (ch - ndimage.gaussian_filter(ch, sigma, mode="wrap"))


def luminance(rgb):
    lin = rgb ** 2.2
    return (lin @ np.array([0.2126, 0.7152, 0.0722], np.float32)) ** (1 / 2.2)


def height_from_slopes(sx, sy):
    """Integrate a gradient field (per texel) into heights, periodic (Frankot-Chellappa)."""
    h, w = sx.shape
    fy = np.fft.fftfreq(h)[:, None]
    fx = np.fft.fftfreq(w)[None, :]
    # Wrapped forward differences' transfer functions (as the shader steps texel to texel).
    dx = np.exp(2j * np.pi * fx) - 1
    dy = np.exp(2j * np.pi * fy) - 1
    den = np.abs(dx) ** 2 + np.abs(dy) ** 2
    den[0, 0] = 1
    z = (np.conj(dx) * np.fft.fft2(sx) + np.conj(dy) * np.fft.fft2(sy)) / den
    z[0, 0] = 0
    return np.real(np.fft.ifft2(z))


def stretch(x, lo=0.5, hi=99.5):
    a, b = np.percentile(x, [lo, hi])
    return np.clip((x - a) / max(b - a, 1e-6), 0, 1)


# IW3's normal packing (shaders/world.wgsl `iw3_slope`): x in alpha, y in green.
def unpack_slopes(img):
    return img[..., 3] * 4.08 - 2.08, img[..., 1] * 4.0645161 - 2.0645161


def pack_slopes(sx, sy):
    h, w = sx.shape
    out = np.zeros((h, w, 4), np.float32)
    out[..., 3] = np.clip((sx + 2.08) / 4.08, 0, 1)
    out[..., 1] = np.clip((sy + 2.0645161) / 4.0645161, 0, 1)
    return out


def gloss_to_roughness(g):
    """As the world shader turns IW3 gloss into GGX roughness."""
    power = np.exp2(g * 9.3775) + 7
    return np.sqrt(np.sqrt(2 / (power + 2)))


def grey(ch, alpha=False):
    h, w = ch.shape
    return np.dstack([ch, ch, ch, np.ones((h, w), np.float32)])


# --------------------------------------------------------------------------

def main():
    args = sys.argv[1:]
    listing, out_dir = args[0], args[1]
    opt = lambda k, d: args[args.index(k) + 1] if k in args else d
    esrgan = opt("--esrgan", None)
    max_side = int(opt("--max", "2048"))
    only = set(filter(None, opt("--only", "").split(",")))
    os.makedirs(out_dir, exist_ok=True)
    work = os.path.join(out_dir, "_work")
    os.makedirs(work, exist_ok=True)

    main_dir = None
    by_image = {}  # image -> semantic
    world = set()  # images world surfaces use
    mats = {}  # material -> {semantic: image}
    for line in open(listing, encoding="utf-8"):
        line = line.strip()
        if line.startswith("# main "):
            main_dir = line[7:]
            continue
        if not line or line.startswith("#"):
            continue
        sem, image, mat, _user = line.split(" ", 3)
        if image.lstrip(",").startswith("$"):
            continue
        image = image.lstrip(",")
        by_image.setdefault(image, sem)
        if _user == "world":
            world.add(image)
        mats.setdefault(mat, {})[sem] = image
    files = mount(main_dir)
    names = [n for n in by_image if not only or n in only]
    print(f"{len(names)} images, {len(mats)} materials", flush=True)

    # Which spec map (and normal map) goes with each colour map.
    spec_for, normal_for = {}, {}
    for m in mats.values():
        if "color" in m:
            if "spec" in m:
                spec_for.setdefault(m["color"], m["spec"])
            if "normal" in m:
                normal_for.setdefault(m["color"], m["normal"])

    src = {}
    for n in names + [s for s in spec_for.values() if s not in names]:
        data = read_iwi(files, n)
        img = decode_iwi(data) if data else None
        if img is None:
            print(f"skip {n}: not in the iwds (zone image or unsupported)", flush=True)
            continue
        src[n] = img

    def size_for(img, side=max_side):
        h, w = img.shape[:2]
        k = min(4, side / max(h, w))
        k = max(k, 1)
        return int(h * k) // 4 * 4, int(w * k) // 4 * 4

    # Colour maps through Real-ESRGAN, all in one run (one folder in, one out).
    colours = [n for n in names if by_image[n] == "color" and n in src]
    esr_in, esr_out = os.path.join(work, "in"), os.path.join(work, "out")
    os.makedirs(esr_in, exist_ok=True)
    os.makedirs(esr_out, exist_ok=True)
    todo = []
    for n in colours:
        safe = n.replace("~", "_t_").replace("&", "_a_")
        if os.path.exists(os.path.join(out_dir, n + ".iwi")) or os.path.exists(os.path.join(esr_out, safe + ".png")):
            continue
        rgb = src[n][..., :3]
        rgb = np.pad(rgb, ((PAD, PAD), (PAD, PAD), (0, 0)), mode="wrap")
        Image.fromarray((rgb * 255 + 0.5).astype(np.uint8)).save(os.path.join(esr_in, safe + ".png"))
        todo.append(n)
    if todo and esrgan:
        print(f"real-esrgan: {len(todo)} colour maps", flush=True)
        flags = getattr(subprocess, "IDLE_PRIORITY_CLASS", 0)
        subprocess.run(
            [esrgan, "-i", esr_in, "-o", esr_out, "-n", "realesrgan-x4plus", "-s", "4", "-t", "64", "-j", "1:1:1", "-f", "png"],
            check=True,
            creationflags=flags,
            cwd=work,
        )

    stats = {"files": 0, "disk": 0, "vram_new": 0, "vram_old": 0}

    def put(path, img, alpha, flags=0):
        size = write_iwi(path, img, alpha, flags)
        stats["files"] += 1
        stats["disk"] += size
        stats["vram_new"] += size - 28

    for n in names:
        if n not in src:
            continue
        sem = by_image[n]
        img = src[n]
        h0, w0 = img.shape[:2]
        H, W = size_for(img)
        # Material maps at half the colour's cap: they're smoother, and
        # VRAM is better spent on colour.
        mH, mW = size_for(img, max_side // 2)
        stats["vram_old"] += len(read_iwi(files, n)) - 28
        out = os.path.join(out_dir, n + ".iwi")
        if os.path.exists(out):
            continue
        if sem == "color":
            safe = n.replace("~", "_t_").replace("&", "_a_")
            up_path = os.path.join(esr_out, safe + ".png")
            if os.path.exists(up_path):
                up = np.asarray(Image.open(up_path).convert("RGB"), np.float32) / 255
                up = up[PAD * 4 : PAD * 4 + h0 * 4, PAD * 4 : PAD * 4 + w0 * 4]
                up = box_down(up, H, W) if up.shape[0] != H else up
                # The network smooths fine grain (plywood went waxy) and
                # drifts colour a little: give back some of the source's
                # grain, and keep its colour at low frequency.
                bic = np.dstack([resize(img[..., c], H, W) for c in range(3)])
                sig = H / h0
                up = up + 0.3 * (bic - ndimage.gaussian_filter(bic, (sig, sig, 0), mode="wrap"))
                big = (4 * sig, 4 * sig, 0)
                up = up + ndimage.gaussian_filter(bic, big, mode="wrap") - ndimage.gaussian_filter(up, big, mode="wrap")
            else:
                up = np.dstack([resize(img[..., c], H, W) for c in range(3)])
            up = np.clip(up, 0, 1)
            a = img[..., 3]
            has_alpha = a.min() < 0.98
            alpha = np.clip(resize(a, H, W), 0, 1) if has_alpha else np.ones((H, W), np.float32)
            put(out, np.dstack([up, alpha]), has_alpha)
            # Roughness (green): from the material's gloss, else from the colour.
            small = box_down(up, mH, mW)
            lum = luminance(small)
            detail = lum - ndimage.gaussian_filter(lum, max(mH, mW) / 64, mode="wrap")
            if n in spec_for and spec_for[n] in src:
                g = src[spec_for[n]][..., 3]
                rough = gloss_to_roughness(resize(g, mH, mW))
            else:
                # Dull stone/wood/paint: rough, a little less so on brighter, smoother texels.
                rough = np.clip(0.86 - 0.5 * detail - 0.08 * (lum - lum.mean()), 0.55, 1.0)
            rimg = np.zeros((mH, mW, 4), np.float32)
            rimg[..., 1] = rough
            rimg[..., 3] = 1
            put(os.path.join(out_dir, n + ".rough.iwi"), rimg, False)
            if n not in world:
                print(f"{sem:6} {n} {w0}x{h0} -> {W}x{H} (model)", flush=True)
                continue
            # Heights from the colour: dark crevices low (wraps round).
            hh = stretch(ndimage.gaussian_filter(detail, 0.6 * mH / max(h0, 1), mode="wrap"))
            put(os.path.join(out_dir, n + ".height.iwi"), grey(hh), False)
            if n not in normal_for:
                # A normal map from those heights, gentle: as if the texture
                # spans 2 m and its heights 8 mm (slopes are height change
                # per texel spacing; the surface falls by x along u).
                gy, gx = np.gradient(np.pad(hh, 1, mode="wrap"))
                k = 0.004 * max(mH, mW)
                put(os.path.join(out_dir, n + ".gen_n.iwi"), pack_slopes(-gx[1:-1, 1:-1] * k, -gy[1:-1, 1:-1] * k), True, 0x20)
        elif sem == "normal":
            sx, sy = unpack_slopes(img)
            # Bicubic, then a light sharpen; slopes kept to what the packing holds.
            H, W = mH, mW
            ux = sharpen(resize(sx, H, W), H / h0 * 0.7, 0.5)
            uy = sharpen(resize(sy, H, W), H / h0 * 0.7, 0.5)
            # Renormalise: keep each (x, y, 1)'s tilt within the source's steepest.
            lim = np.percentile(np.hypot(sx, sy), 99.9) + 1e-3
            t = np.hypot(ux, uy)
            s = np.where(t > lim, lim / np.maximum(t, 1e-6), 1.0)
            ux, uy = ux * s, uy * s
            put(out, pack_slopes(ux, uy), True, 0x20)
            # Heights from the slopes (as crate::pom: surface falls by x along u).
            if n not in world:
                print(f"{sem:6} {n} {w0}x{h0} -> {W}x{H} (model)", flush=True)
                continue
            hh = stretch(height_from_slopes(-ux, -uy))
            put(os.path.join(out_dir, n + ".height.iwi"), grey(hh), False)
        elif sem == "spec":
            H, W = mH, mW
            up = np.dstack([np.clip(sharpen(resize(img[..., c], H, W), H / h0 * 0.7, 0.35), 0, 1) for c in range(4)])
            put(out, up, True)
        print(f"{sem:6} {n} {w0}x{h0} -> {W}x{H}", flush=True)

    print(f"done: {stats['files']} files, {stats['disk'] / 2**20:.1f} MiB on disk; texture data {stats['vram_new'] / 2**20:.1f} MiB new vs {stats['vram_old'] / 2**20:.1f} MiB originals", flush=True)


if __name__ == "__main__":
    main()
