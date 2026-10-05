"""How players play Domination's flags, from demo tracks (`iw3 --example
demodump`) or the game's recordings (`COD4RW_RECORD=<file.csv>
COD4RW_RECORD_WHO=all`), to compare bots with real players:

    python tools/dom_play.py --flags flags.txt --map mp_broadcast <csv files or dirs>...

`flags.txt` has a line per flag, `<map>|<the map entity's keys>` as
`iw3 --example ents <map> flag_primary` prints them, prefixed with the map.

For each player's time alive it measures: the share spent on a flag (inside
its trigger) and near one (within 600 units), flag visits a minute and how
long they last, how many of a team stand on a flag together, which flag a
player goes to next after leaving one (the same, the nearest other, the
far one), and how the time on flags splits over A, B and C.
"""

import argparse
import csv
import glob
import math
import os
import re
from collections import Counter, defaultdict

DT = 0.2
NEAR = 600.0
MIN_VISIT = 1.0


def flags_of(path, map_name):
    out = []
    for line in open(path, encoding="utf8"):
        if "|" not in line:
            continue
        m, rest = line.split("|", 1)
        if m.strip().lower() != map_name.lower():
            continue
        kv = dict(re.findall(r"(\w+)=([^=]+?)(?=\s+\w+=|$)", rest.strip()))
        x, y, z = (float(v) for v in kv["origin"].split()[:3])
        label = kv.get("script_label", "_?").lstrip("_").upper()[:1]
        out.append({"label": label, "x": x, "y": y, "z": z, "r": float(kv.get("radius", 160)), "h": float(kv.get("height", 128))})
    return sorted(out, key=lambda f: f["label"])


def files(patterns):
    out = []
    for p in patterns:
        if os.path.isdir(p):
            out += sorted(glob.glob(os.path.join(p, "*.csv")))
        else:
            out += sorted(glob.glob(p))
    return out


def load(path):
    """Alive samples every DT seconds: (t, x, y, z, team), split into runs."""
    with open(path, newline="") as f:
        rows = list(csv.DictReader(f))
    if len(rows) < 25:
        return []
    runs, run = [], []
    last = None
    next_t = None
    for r in rows:
        t = float(r["t"])
        if next_t is not None and t < next_t:
            continue
        dead = float(r.get("dead") or 0) > 0.5
        x, y, z = float(r["x"]), float(r["y"]), float(r["z"])
        team = r.get("team", "")
        jump = last is not None and math.hypot(x - last[1], y - last[2]) > 120.0
        gap = last is not None and t - last[0] > 3 * DT
        if dead or jump or gap:
            if len(run) >= 10:
                runs.append(run)
            run = []
        if not dead:
            run.append((t, x, y, z, team))
            last = (t, x, y, z)
        next_t = t + DT - 1e-3
    if len(run) >= 10:
        runs.append(run)
    return runs


def on(f, x, y, z):
    return math.hypot(x - f["x"], y - f["y"]) <= f["r"] and -24.0 <= z - f["z"] <= f["h"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--flags", required=True)
    ap.add_argument("--map", required=True)
    ap.add_argument("--skip", default="Shane", help="comma-separated names to leave out (the recorder)")
    ap.add_argument("paths", nargs="+")
    a = ap.parse_args()
    flags = flags_of(a.flags, a.map)
    if not flags:
        raise SystemExit(f"no flags for {a.map}")
    skip = [s for s in a.skip.split(",") if s]
    paths = [p for p in files(a.paths) if not any(s in os.path.basename(p) for s in skip)]
    total = on_flag = near = 0
    per_flag = Counter()
    visits = []  # (player, flag index, start, seconds)
    occupancy = defaultdict(Counter)  # (file group, team, t) -> flag -> count
    players = 0
    minutes = 0.0
    for p in paths:
        group = os.path.basename(p).split("-")[0]
        runs = load(p)
        if not runs:
            continue
        players += 1
        for run in runs:
            minutes += len(run) * DT / 60.0
            cur, start = None, 0.0
            for (t, x, y, z, team) in run:
                total += 1
                here = next((i for i, f in enumerate(flags) if on(f, x, y, z)), None)
                d = min(math.hypot(x - f["x"], y - f["y"]) for f in flags)
                if here is not None:
                    on_flag += 1
                    per_flag[flags[here]["label"]] += 1
                    occupancy[(group, team, round(t / DT))][here] += 1
                if d <= NEAR:
                    near += 1
                if here != cur:
                    if cur is not None and t - start >= MIN_VISIT:
                        visits.append((p, cur, start, t - start))
                    cur, start = here, t
            if cur is not None and run[-1][0] - start >= MIN_VISIT:
                visits.append((p, cur, start, run[-1][0] - start))
    if total == 0:
        raise SystemExit("no samples")
    print(f"{a.map}: {players} players, {minutes:.0f} player-minutes alive, {len(flags)} flags")
    print(f"  time on a flag {100 * on_flag / total:.1f}%, within {NEAR:.0f} of one {100 * near / total:.1f}%")
    shares = ", ".join(f"{k} {100 * v / max(on_flag, 1):.0f}%" for k, v in sorted(per_flag.items()))
    print(f"  on-flag time by flag: {shares}")
    if visits:
        secs = sorted(v[3] for v in visits)
        print(
            f"  visits {len(visits) / minutes:.2f}/min, lasting median {secs[len(secs) // 2]:.1f}s, "
            f"mean {sum(secs) / len(secs):.1f}s, 90th {secs[int(0.9 * (len(secs) - 1))]:.1f}s"
        )
    sizes = Counter()
    for counts in occupancy.values():
        for n in counts.values():
            sizes[min(n, 4)] += 1
    n = sum(sizes.values())
    if n:
        print("  teammates on a flag together: " + ", ".join(f"{k}{'+' if k == 4 else ''}: {100 * v / n:.0f}%" for k, v in sorted(sizes.items())))
    # Where players went next after a flag.
    by_player = defaultdict(list)
    for v in visits:
        by_player[v[0]].append(v)
    nxt = Counter()
    for vs in by_player.values():
        vs.sort(key=lambda v: v[2])
        for a_, b_ in zip(vs, vs[1:]):
            if b_[2] - (a_[2] + a_[3]) > 45.0:
                continue
            fa = flags[a_[1]]
            others = sorted((i for i in range(len(flags)) if i != a_[1]), key=lambda i: math.hypot(flags[i]["x"] - fa["x"], flags[i]["y"] - fa["y"]))
            if b_[1] == a_[1]:
                nxt["same"] += 1
            elif others and b_[1] == others[0]:
                nxt["nearest other"] += 1
            else:
                nxt["far"] += 1
    m = sum(nxt.values())
    if m:
        print("  next flag after leaving one: " + ", ".join(f"{k} {100 * v / m:.0f}%" for k, v in nxt.most_common()))


if __name__ == "__main__":
    main()
