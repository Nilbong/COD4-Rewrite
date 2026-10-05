"""Tune the parameters of the holding-spot score
(`crates/game/src/bots/tactical.rs`'s `hold_spots`) to where real players
held, on `COD4RW_HOLDFIT` dumps (see `tools/fit_holds.py`):

    python tools/tune_holds.py holdfit.csv

The score is the game's form, with its constants free:

    (close*wc + mid + long*wl) * (1 - a*traffic) * (1.25 - b*exposure)
        * (1 + h*clamp(height/100, -1, 2)) * (1 - s*exp(-spawn_dist/600))

over spots no more open than `emax`. A coordinate search maximises the mean
of recall and precision (as `fit_holds.py` measures them), checked leaving
each map out in turn, then fitted on all of them.
"""

import math
import sys

import numpy as np

sys.path.insert(0, __file__.rsplit("\\", 1)[0].rsplit("/", 1)[0])
from fit_holds import load, pick, score_map  # noqa: E402

START = {"wc": 0.6, "wl": 0.7, "a": 0.6, "b": 0.5, "h": 0.0, "s": 0.0, "emax": 0.75}
STEPS = {"wc": [0.0, 0.3, 0.6, 1.0, 1.5], "wl": [0.0, 0.3, 0.7, 1.0, 1.4], "a": [0.0, 0.3, 0.6, 0.9],
         "b": [0.0, 0.25, 0.5, 0.75, 1.0], "h": [0.0, 0.15, 0.3, 0.6], "s": [0.0, 0.3, 0.6, 0.9],
         "emax": [0.5, 0.6, 0.75, 0.9, 1.01]}


def scores(rows, p):
    out = np.empty(len(rows))
    for i, r in enumerate(rows):
        if r["exposure"] > p["emax"] or r["seen"] == 0:
            out[i] = -1e9
            continue
        s = p["wc"] * r["close"] + r["mid"] + p["wl"] * r["long"]
        s *= 1.0 - p["a"] * r["traffic"]
        s *= 1.25 - p["b"] * r["exposure"]
        s *= 1.0 + p["h"] * max(-1.0, min(2.0, r["height"] / 100.0))
        s *= 1.0 - p["s"] * math.exp(-r["spawn_dist"] / 600.0)
        out[i] = s
    return out


def quality(maps, names, p):
    q = [score_map(maps[m], pick(maps[m], scores(maps[m], p))) for m in names]
    return float(np.mean([(r + pr) / 2 for r, pr in q])), q


def search(maps, names):
    p = dict(START)
    best, _ = quality(maps, names, p)
    for _ in range(3):
        improved = False
        for k, vals in STEPS.items():
            for v in vals:
                if v == p[k]:
                    continue
                trial = dict(p, **{k: v})
                q, _ = quality(maps, names, trial)
                if q > best + 1e-4:
                    best, p, improved = q, trial, True
        if not improved:
            break
    return p, best


def main():
    maps = load(sys.argv[1])
    names = list(maps)
    print("leaving each map out:")
    for m in names:
        p, _ = search(maps, [k for k in names if k != m])
        (rb, pb), = quality(maps, [m], START)[1]
        (rt, pt), = quality(maps, [m], p)[1]
        print(f"  {m:16} start {rb:.2f}/{pb:.2f}  tuned {rt:.2f}/{pt:.2f}  {p}")
    p, q = search(maps, names)
    base, _ = quality(maps, names, START)
    print(f"all maps: start {base:.3f}, tuned {q:.3f}: {p}")


if __name__ == "__main__":
    main()
