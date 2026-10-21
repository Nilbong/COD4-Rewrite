"""The user's own matches against bots as a real-player reference (read
only: the recordings are never changed).

    python tools/my_matches.py [--dir %LOCALAPPDATA%\\cod4rw\\recordings] [--unverified]

Keeps matches that ran to the end with at least 5 a side: the game writes
`<recording>.match.txt` beside a recording when its match ends (map, mode,
length, players a side, scores; bots/record.rs). Recordings from before that
(2026-10-19) have no summary: `--unverified` lists them too, kept if they
last 5+ minutes, as a guess only.

For each kept match, and pooled: ADS share while shooting by distance and
moving or not, engagement range by weapon class (shots with an enemy in the
crosshair), reaction (an enemy coming into the crosshair cone to the first
shot), and how often the player keeps shooting just after losing sight.
"""

import argparse
import collections
import csv
import glob
import os

import numpy as np

CLASSES = {
    "sniper": ("barrett", "m40a3", "m21", "dragunov", "remington700"),
    "smg": ("mp5", "p90", "uzi", "ak74u", "skorpion"),
    "rifle": ("m16", "ak47", "m4", "g36c", "g3", "m14", "mp44"),
    "lmg": ("rpd", "saw", "m60e4"),
    "shotgun": ("winchester1200", "m1014", "model1887", "spas12", "aa12", "striker", "ranger"),
}


def gun_class(w):
    name = w.replace("iw4_", "").replace("t4_", "").replace("t5_", "")
    for k, guns in CLASSES.items():
        if any(name.startswith(g) for g in guns):
            return k
    return "other"


def summary_of(path):
    s = path[:-4] + ".match.txt"
    if not os.path.exists(s):
        return None
    return dict(line.strip().split("=", 1) for line in open(s) if "=" in line)


def analyse(path):
    rows = list(csv.DictReader(open(path, newline="")))[:-1]
    if len(rows) < 1000:
        return None
    t = np.array([float(r["t"]) for r in rows])
    num = lambda k: np.array([float(r.get(k) or 0) for r in rows])
    x, y = num("x"), num("y")
    dt = np.clip(np.diff(t, append=t[-1]), 0, 0.1)
    speed = np.hypot(np.gradient(x, t), np.gradient(y, t))
    alive = num("dead") < 0.5
    out = {"minutes": float(dt[alive].sum()) / 60.0}
    shots = num("shots") > 0
    edist = np.array([float(r["edist"]) if r["edist"] else np.nan for r in rows])
    ads = num("ads") > 0
    moving = speed > 40
    acc = collections.defaultdict(lambda: [0, 0])
    for i in np.flatnonzero(shots & alive & ~np.isnan(edist)):
        b = 0 if edist[i] < 500 else 1 if edist[i] < 1000 else 2
        acc[(b, bool(moving[i]))][0] += 1
        acc[(b, bool(moving[i]))][1] += ads[i]
    out["ads"] = {k: v[1] / v[0] for k, v in acc.items() if v[0] >= 10}
    out["ads_n"] = {k: v[0] for k, v in acc.items()}
    ranges = collections.defaultdict(list)
    for i in np.flatnonzero(shots & alive & ~np.isnan(edist)):
        ranges[gun_class(rows[i].get("weapon") or "?")].append(edist[i])
    out["range"] = {k: (len(v), float(np.median(v))) for k, v in ranges.items()}
    # Reaction: an enemy entering the crosshair cone (the recorder's enemy
    # column) after 1 s without one, to the first shot within 2 s.
    has = ~np.isnan(edist)
    reacts = []
    last_none = -10.0
    for i in range(1, len(rows)):
        if not has[i]:
            last_none = t[i]
            continue
        if not has[i - 1] and t[i] - last_none < 0.05 and i > 1:
            j = i
            while j < len(rows) and t[j] - t[i] < 2.0 and has[j]:
                if shots[j]:
                    reacts.append(t[j] - t[i])
                    break
                j += 1
    out["reaction_p50"] = float(np.median(reacts)) if len(reacts) >= 5 else float("nan")
    out["reaction_n"] = len(reacts)
    # Follow-through: shots within 1 s after the enemy left the crosshair.
    lost = [i for i in range(1, len(rows)) if has[i - 1] and not has[i]]
    follow = [any(shots[j] for j in range(i, min(len(rows), i + 92))) for i in lost]
    out["follow_share"] = float(np.mean(follow)) if follow else float("nan")
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--dir", default=os.path.expandvars(r"%LOCALAPPDATA%\cod4rw\recordings"))
    ap.add_argument("--unverified", action="store_true")
    a = ap.parse_args()
    kept = []
    for f in sorted(glob.glob(os.path.join(a.dir, "*.csv")), key=os.path.getmtime):
        s = summary_of(f)
        if s:
            ok = s.get("ended") == "1" and min(int(s.get("allies", 0)), int(s.get("axis", 0))) >= 5
            why = f"{s.get('mode')} {s.get('allies')}v{s.get('axis')}"
        elif a.unverified:
            ok, why = None, "no summary"
        else:
            continue
        r = analyse(f) if ok is not False else None
        if r is None or (ok is None and r["minutes"] < 5):
            continue
        kept.append((f, why, ok, r))
    if not kept:
        print("no qualifying matches yet (they get a .match.txt from 2026-10-19's build on; --unverified for older ones)")
        return
    names = ["<500", "500-1000", "1000+"]
    for f, why, ok, r in kept:
        ads = ", ".join(f"{names[b]} {'moving' if m else 'still'} {v:.2f}" for (b, m), v in sorted(r["ads"].items()))
        rng = ", ".join(f"{k} {n} shots p50 {d:.0f}u" for k, (n, d) in r["range"].items())
        print(f"{os.path.basename(f)} [{why}{'' if ok else ', unverified'}] {r['minutes']:.1f} min alive")
        print(f"   ADS while shooting: {ads}")
        print(f"   ranges: {rng}")
        print(f"   reaction p50 {r['reaction_p50']:.2f}s (n {r['reaction_n']}), keeps shooting after losing sight {r['follow_share']:.0%}")


if __name__ == "__main__":
    main()
