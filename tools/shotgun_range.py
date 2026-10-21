"""Shotgun bots' shots by distance to the enemy in their crosshair, from a
sim's recordings: python tools/shotgun_range.py <dir>..."""
import csv, glob, os, sys

import numpy as np

for d in sys.argv[1:]:
    dists, hits, far_hits = [], 0, 0
    for f in glob.glob(os.path.join(d, "**", "bots-*.csv"), recursive=True):
        if f.endswith("-personas.csv"):
            continue
        rows = list(csv.DictReader(open(f)))[:-1]
        for r in rows:
            if ("winchester" in r.get("weapon", "") or "m1014" in r.get("weapon", "")) and r["shots"] not in ("", "0") and r["edist"]:
                dists.append(float(r["edist"]))
    v = np.array(dists)
    if len(v):
        print(f"{d}: {len(v)} shotgun shots at someone in sight; distance p25/p50/p75 {np.percentile(v, [25, 50, 75]).round()}; "
              f"past 500 u {(v > 500).mean():.0%}, past 800 u {(v > 800).mean():.0%}")
    else:
        print(d, "no shotgun shots")
