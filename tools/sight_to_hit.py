"""Bots' (or anyone's) time from an enemy coming into line of sight to their
first hit on anyone, and hit rate: python tools/sight_to_hit.py <dir|csv>...
Same definition as tools/duel_stats.py's "sight to first hit"."""
import csv, glob, os, sys

import numpy as np

for arg in sys.argv[1:]:
    files = sorted(glob.glob(os.path.join(arg, "**", "*.csv"), recursive=True)) if os.path.isdir(arg) else [arg]
    times, shots, hits = [], 0, 0
    for f in files:
        if f.endswith("-personas.csv"):
            continue
        rows = list(csv.DictReader(open(f, newline="")))[:-1]
        if not rows or "vis" not in rows[0]:
            continue
        t = np.array([float(r["t"]) for r in rows])
        h = np.array([int(r["hits"] or 0) for r in rows])
        dead = np.array([r["dead"] == "1" for r in rows])
        shots += sum(int(r["shots"] or 0) for r in rows)
        hits += int(h.sum())
        seen = {}
        for i, r in enumerate(rows):
            if dead[i] or not r["vis"]:
                continue
            for e in r["vis"].split(";"):
                if t[i] - seen.get(e, -99) > 2.0:
                    end = np.searchsorted(t, t[i] + 4.0)
                    j = next((j for j in range(i, end) if h[j] > 0), None)
                    if j is not None:
                        times.append(t[j] - t[i])
                seen[e] = t[i]
    v = np.array(times)
    print(f"{arg}: sight to first hit p25/p50/p75 {np.percentile(v, [25, 50, 75]).round(2)} (n {len(v)}); hit rate {hits / max(shots, 1):.0%}")
