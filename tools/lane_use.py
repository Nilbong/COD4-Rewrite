"""Lane use across a map, real players vs bots: python tools/lane_use.py <real demo dir> <bot sim dir>...
Positions are projected onto the map's long axis (the main direction of
travel, from all real positions) and the axis across it; the across axis is
cut in five lanes by the real players' quantiles, so real time is 20% each,
and bots' share per lane shows if they bunch up in the middle."""
import csv, glob, os, sys

import numpy as np


def positions(d):
    pts = []
    for f in glob.glob(os.path.join(d, "**", "*.csv"), recursive=True):
        if f.endswith("-personas.csv") or f.endswith("players.csv") or f.endswith("events.csv"):
            continue
        rows = list(csv.DictReader(open(f, newline="")))[:-1]
        for r in rows[::4]:
            if r.get("dead", "0") == "1":
                continue
            pts.append((float(r["x"]), float(r["y"])))
    return np.array(pts)


real = positions(sys.argv[1])
c = real.mean(0)
u_, s_, vt = np.linalg.svd(real - c, full_matrices=False)
along, across = vt[0], vt[1]
qa = np.quantile((real - c) @ across, [0.2, 0.4, 0.6, 0.8])
qa_long = np.quantile((real - c) @ along, [0.33, 0.67])


def lanes(p):
    x = (p - c) @ across
    y = (p - c) @ along
    mid = (y > qa_long[0]) & (y < qa_long[1])  # the middle third of the map's length
    h = np.histogram(x, bins=[-1e9, *qa, 1e9])[0] / len(x)
    hm = np.histogram(x[mid], bins=[-1e9, *qa, 1e9])[0] / max(mid.sum(), 1)
    return h, hm


print("lanes across the map (left .. right), share of time; real = 20% each by construction")
for name, d in [("real", sys.argv[1])] + [(os.path.basename(a.rstrip('/')), a) for a in sys.argv[2:]]:
    h, hm = lanes(positions(d))
    print(f"{name:12} all {' '.join(f'{v:.0%}' for v in h)} | middle third of the map {' '.join(f'{v:.0%}' for v in hm)}")
