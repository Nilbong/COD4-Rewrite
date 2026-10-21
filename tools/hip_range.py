"""Hip fire by range, from recordings (bots from a sim with
COD4RW_RECORD_WHO=all, or the user's own):

    python tools/hip_range.py <dir|csv>...

Per distance band: the share of shots fired from the hip (sights fully
down), and the hit rate of hip and of aimed shots. Shots are counted in the
row they were fired in, with that row's hits and its nearest enemy's
distance (`edist`).
"""

import collections
import csv
import glob
import os
import sys

BANDS = [(0, 394, "<10m"), (394, 985, "10-25m"), (985, 1970, "25-50m"), (1970, 3940, "50-100m"), (3940, 1e9, ">100m")]


def files_of(arg):
    if os.path.isdir(arg):
        return [f for f in sorted(glob.glob(os.path.join(arg, "**", "*.csv"), recursive=True)) if not f.endswith("-personas.csv")]
    return [arg]


for arg in sys.argv[1:]:
    # band -> [hip shots, hip hits, aimed shots, aimed hits]
    acc = collections.defaultdict(lambda: [0, 0, 0, 0])
    for f in files_of(arg):
        rows = list(csv.DictReader(open(f, newline="")))[:-1]
        if not rows or "ads_level" not in rows[0]:
            continue
        for r in rows:
            n = int(r["shots"] or 0)
            if not n or not r["edist"]:
                continue
            d = float(r["edist"])
            band = next(b[2] for b in BANDS if b[0] <= d < b[1])
            hip = float(r["ads_level"] or 0) == 0
            h = int(r["hits"] or 0)
            a = acc[band]
            a[0 if hip else 2] += n
            a[1 if hip else 3] += h
    print(f"== {arg}")
    for _, _, band in BANDS:
        hs, hh, as_, ah = acc[band]
        if hs + as_ < 20:
            continue
        print(f"   {band:8} shots {hs + as_:5}  hip {hs / (hs + as_):5.1%}  hip hit {hh / max(hs, 1):5.1%} (n {hs})  aimed hit {ah / max(as_, 1):5.1%}")
