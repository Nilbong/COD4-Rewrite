"""How recordings aim (players' or bots'): per sighting, the time to get
within 2 degrees of the target, overshoot past it and tracking error once
there, before the first shot's recoil. Medians and quartiles per group.

Usage: python aim_eval.py <name>=<recordings dir or files> ...
e.g.   python aim_eval.py player=recordings hand=bots_hand netaim=bots_net
"""

import glob
import os
import sys

import numpy as np

from aim_data import DT, load, resample, samples, sightings

SETTLE_DEG = 2.0


def metrics(err):
    mag = np.hypot(err[:, 0], err[:, 1])
    inside = np.nonzero(mag < SETTLE_DEG)[0]
    settle = inside[0] * DT if len(inside) else np.nan
    d0 = err[0] / (np.linalg.norm(err[0]) + 1e-9)
    over = max(0.0, -(err @ d0).min()) if np.linalg.norm(err[0]) > 4 else np.nan
    rms = np.sqrt((mag[inside[0]:] ** 2).mean()) if len(inside) else np.nan
    return settle, over, rms, np.linalg.norm(err[0])


def group(paths):
    files = []
    for p in paths.split(","):
        files += sorted(glob.glob(os.path.join(p, "*.csv"))) if os.path.isdir(p) else glob.glob(p)
    rows = []
    for f in files:
        for seg in sightings(load(f)):
            s = resample(seg)
            if s is None:
                continue
            X, _, R = samples(s)
            n = int(np.argmax(R)) if R.any() else len(X)
            if n >= 6:
                rows.append(metrics(X[:n, 0:2]))
    return np.array(rows, float)


def q(a):
    a = a[~np.isnan(a)]
    return "n/a" if len(a) == 0 else "%.2f / %.2f / %.2f" % tuple(np.percentile(a, [25, 50, 75]))


def main():
    print(f"{'group':10} {'sightings':>9}  settle s (p25/50/75)   overshoot deg          tracking rms deg       start error deg")
    for arg in sys.argv[1:]:
        name, paths = arg.split("=", 1)
        r = group(paths)
        if len(r) == 0:
            print(f"{name:10} none")
            continue
        print(f"{name:10} {len(r):9}  {q(r[:,0]):22} {q(r[:,1]):22} {q(r[:,2]):22} {q(r[:,3])}")


if __name__ == "__main__":
    main()
