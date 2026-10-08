"""How much of where bots go matches where real players go, on one map.

Tracks are the recorder's CSVs (`demodump` for demos, `COD4RW_RECORD` for
bots). Positions while moving are binned into 128-unit cells (per floor band
of 128 units); each side becomes a distribution over cells, and the overlap
is the sum of the smaller share per cell (1 = the same places, 0 = none in
common). With --png it also draws a top-down heatmap, real (red) vs bots
(cyan), white where they agree.

    python tools/route_overlap.py --real <dir> --bots <dir> [--png out.png]
"""

import argparse
import csv
import glob
import os
from collections import Counter

CELL = 128.0
MOVING = 60.0


def tracks(path, skip=("shane",)):
    out = []
    files = sorted(glob.glob(os.path.join(path, "*.csv"))) if os.path.isdir(path) else [path]
    for f in files:
        if any(s in os.path.basename(f).lower() for s in skip):
            continue
        with open(f, newline="") as fh:
            rows = list(csv.DictReader(fh))
        out.append([(float(r["t"]), float(r["x"]), float(r["y"]), float(r["z"])) for r in rows])
    return out


def cells(ts):
    c = Counter()
    for tr in ts:
        for (t0, x0, y0, z0), (t1, x1, y1, z1) in zip(tr, tr[1:]):
            dt = t1 - t0
            if dt <= 0 or dt > 1.0:
                continue
            speed = ((x1 - x0) ** 2 + (y1 - y0) ** 2) ** 0.5 / dt
            if speed < MOVING or speed > 600:
                continue
            c[(int(x1 // CELL), int(y1 // CELL), int(z1 // CELL))] += dt
    total = sum(c.values()) or 1.0
    return {k: v / total for k, v in c.items()}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--real", nargs="+", required=True)
    ap.add_argument("--bots", nargs="+", required=True)
    ap.add_argument("--png")
    a = ap.parse_args()
    real = cells([t for p in a.real for t in tracks(p)])
    bots = cells([t for p in a.bots for t in tracks(p)])
    overlap = sum(min(v, bots.get(k, 0.0)) for k, v in real.items())
    print(f"route overlap {overlap:.3f} ({len(real)} real cells, {len(bots)} bot cells)")
    if a.png:
        from PIL import Image

        r2, b2 = Counter(), Counter()
        for (x, y, _), v in real.items():
            r2[(x, y)] += v
        for (x, y, _), v in bots.items():
            b2[(x, y)] += v
        keys = set(r2) | set(b2)
        xs = [k[0] for k in keys]
        ys = [k[1] for k in keys]
        x0, y0 = min(xs), min(ys)
        w, h = max(xs) - x0 + 1, max(ys) - y0 + 1
        scale = 6
        img = Image.new("RGB", (w * scale, h * scale))
        rmax = max(r2.values()) or 1
        bmax = max(b2.values()) or 1
        for (x, y) in keys:
            r = min(1.0, (r2.get((x, y), 0) / rmax) ** 0.5)
            b = min(1.0, (b2.get((x, y), 0) / bmax) ** 0.5)
            col = (int(255 * r), int(255 * b), int(255 * b))
            px, py = (x - x0) * scale, (h - 1 - (y - y0)) * scale
            for i in range(scale):
                for j in range(scale):
                    img.putpixel((px + i, py + j), col)
        img.save(a.png)


if __name__ == "__main__":
    main()
