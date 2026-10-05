"""Compare how bots play with how real players do.

Both come as CSV tracks with the game recorder's columns: real players from
demos (`cargo run --release -p iw3 --example demodump -- <demo> <dir>`), bots
from the game (`COD4RW_RECORD=<file.csv> COD4RW_RECORD_WHO=all`).

    python tools/compare_play.py --real <dir|glob>... --bots <dir|glob>...

Everything is resampled at 20 Hz (the demos' rate). A moment counts as a
fight when the player fired or aimed down sights within half a second of it.
Prints each measure for real players and bots, with how much real players
differ from each other (10th-90th percentile across players) so a bot
figure inside that range is as human as the players were.
"""

import argparse
import csv
import glob
import math
import os
import sys

import numpy as np

RATE = 20.0
DT = 1.0 / RATE
MOVING = 40.0  # units/s
FIGHT_WINDOW = 10  # samples either side (0.5 s)


def files(patterns):
    out = []
    for p in patterns:
        if os.path.isdir(p):
            out += sorted(glob.glob(os.path.join(p, "*.csv")))
        else:
            out += sorted(glob.glob(p))
    return out


def load(path):
    """Runs of 20 Hz samples: dicts of arrays."""
    with open(path, newline="") as f:
        rows = list(csv.DictReader(f))
    if len(rows) < 40:
        return []

    def col(name, default=0.0):
        if name not in rows[0]:
            return np.full(len(rows), default)
        return np.array([float(r[name]) if r[name] != "" else default for r in rows])

    t = col("t")
    data = {k: col(k) for k in ("yaw", "pitch", "fire", "ads", "x", "y", "z", "stance", "dead")}
    # Demos send angles and positions in whole degrees and units: round
    # everything the same so small movements compare fairly.
    for k in ("yaw", "pitch", "x", "y", "z"):
        data[k] = np.round(data[k])
    # Resample: the latest row at or before each tick, if it's recent.
    ticks = np.arange(t[0], t[-1], DT)
    idx = np.searchsorted(t, ticks + 1e-3, side="right") - 1
    fresh = (ticks - t[idx]) < 0.03
    s = {k: v[idx] for k, v in data.items()}
    # Firing: any row since the last tick (the game records every frame and
    # a tap may last one).
    fired = np.concatenate([[0.0], np.cumsum(data["fire"] > 0)])
    prev = np.concatenate([[idx[0] - 1], idx[:-1]])
    s["fire"] = (fired[idx + 1] - fired[prev + 1] > 0).astype(float)
    s["ok"] = fresh & (s["dead"] < 0.5)
    # Split into runs where samples are missing, dead or teleport (respawn).
    step = np.hypot(np.diff(s["x"]), np.diff(s["y"]))
    cut = np.ones(len(ticks), bool)
    cut[1:] = ~(s["ok"][1:] & s["ok"][:-1] & (step < 60.0))
    runs = []
    starts = np.flatnonzero(cut)
    for a, b in zip(starts, list(starts[1:]) + [len(ticks)]):
        if b - a >= 40 and s["ok"][a]:
            runs.append({k: v[a:b] for k, v in s.items()})
    return runs


def wrap(a):
    return (a + 180.0) % 360.0 - 180.0


def measures(runs):
    """Every measure over a player's (or everyone's) runs."""
    acc = {k: [] for k in (
        "speed", "still", "turn", "rel", "yawspd", "pitch", "offset", "crouch", "prone",
        "f_ads", "f_movefire", "f_crouch", "f_yawspd", "fight",
    )}
    stops = still_runs = 0
    still_durations = []
    reversals = 0
    calm_time = 0.0
    strafe_rev = 0
    fight_time = 0.0
    jumps = 0
    for r in runs:
        n = len(r["x"])
        busy = (r["fire"] + r["ads"]) > 0
        fight = np.convolve(busy.astype(float), np.ones(2 * FIGHT_WINDOW + 1), "same") > 0
        vx, vy = np.diff(r["x"]) / DT, np.diff(r["y"]) / DT
        speed = np.hypot(vx, vy)
        heading = np.degrees(np.arctan2(vy, vx))
        view = r["yaw"][:-1] + 90.0  # CoD yaw
        yawspd = np.abs(wrap(np.diff(r["yaw"]))) / DT
        moving = speed > MOVING
        f = fight[:-1]
        calm = ~f
        acc["fight"].append(f)
        acc["speed"].append(speed[calm & moving])
        acc["still"].append(~moving[calm])
        both = moving[1:] & moving[:-1] & calm[1:]
        acc["turn"].append(np.abs(wrap(np.diff(heading)))[both] / DT)
        rel = np.abs(wrap(heading - view))
        acc["rel"].append(rel[calm & moving])
        acc["offset"].append(rel[calm & moving])
        acc["yawspd"].append(yawspd[calm])
        acc["pitch"].append(r["pitch"][:-1][calm])
        acc["crouch"].append(r["stance"][:-1][calm] == 1)
        acc["prone"].append(r["stance"][:-1][calm] == 2)
        acc["f_ads"].append(r["ads"][:-1][f] > 0)
        firing = r["fire"][:-1] > 0
        acc["f_movefire"].append(moving[firing])
        acc["f_crouch"].append(r["stance"][:-1][f] >= 1)
        acc["f_yawspd"].append(yawspd[f])
        calm_time += calm.sum() * DT
        fight_time += f.sum() * DT
        # Stops: still out of fights for 0.25 s or more after moving for
        # 0.5 s or more (a fight cuts a stop short rather than counting
        # towards it).
        state = np.where(~calm, 2, np.where(moving, 1, 0))
        i = 0
        while i < len(state):
            j = i
            while j < len(state) and state[j] == state[i]:
                j += 1
            if state[i] == 0 and j - i >= 5 and i >= 10 and (state[i - 10:i] == 1).all():
                stops += 1
                still_durations.append((j - i) * DT)
            i = j
        # View reversals out of fights, as demoview counts them.
        signed = wrap(np.diff(r["yaw"])) / DT
        prev = 0.0
        for k in range(len(signed)):
            if not calm[k]:
                prev = 0.0
                continue
            v = signed[k]
            if abs(v) > 15.0:
                if abs(prev) > 15.0 and (v > 0) != (prev > 0):
                    reversals += 1
                prev = v
        # Strafe reversals while fighting: sideways velocity flips.
        side = vx * np.cos(np.radians(view + 90.0)) + vy * np.sin(np.radians(view + 90.0))
        prev = 0.0
        for k in range(len(side)):
            if not f[k]:
                prev = 0.0
                continue
            if abs(side[k]) > 60.0:
                if abs(prev) > 60.0 and (side[k] > 0) != (prev > 0):
                    strafe_rev += 1
                prev = side[k]
        # Jumps: a peak 14+ units above 0.3 s before, coming down 10+ after.
        z = r["z"]
        for k in range(6, n - 6):
            if z[k] >= z[k - 1] and z[k] > z[k + 1] and z[k] - z[k - 6] >= 14 and z[k] - z[k + 6] >= 10:
                jumps += 1

    cat = {k: np.concatenate(v) if v else np.array([]) for k, v in acc.items()}

    def pct(a, q):
        return float(np.percentile(a, q)) if len(a) else float("nan")

    def frac(a):
        return 100.0 * float(np.mean(a)) if len(a) else float("nan")

    rel = cat["rel"]
    minutes = (calm_time + fight_time) / 60.0
    return {
        "minutes": minutes,
        "fight time %": frac(cat["fight"]),
        "-- out of fights": None,
        "standing still %": frac(cat["still"]),
        "speed moving p50": pct(cat["speed"], 50),
        "speed moving p90": pct(cat["speed"], 90),
        "stops /min": stops / max(calm_time / 60.0, 1e-6),
        "stop length p50 s": pct(still_durations, 50) if still_durations else float("nan"),
        "path turn deg/s p50": pct(cat["turn"], 50),
        "path turn deg/s p90": pct(cat["turn"], 90),
        "moving forward %": frac(rel < 30),
        "moving diagonal %": frac((rel >= 30) & (rel < 60)),
        "moving sideways %": frac((rel >= 60) & (rel < 120)),
        "moving backward %": frac(rel >= 120),
        "view yaw deg/s p50": pct(cat["yawspd"], 50),
        "view yaw deg/s p90": pct(cat["yawspd"], 90),
        "view still %": frac(cat["yawspd"] < 5.0),
        "view reversals /s": reversals / max(calm_time, 1e-6),
        "pitch mean": float(np.mean(cat["pitch"])) if len(cat["pitch"]) else float("nan"),
        "pitch sd": float(np.std(cat["pitch"])) if len(cat["pitch"]) else float("nan"),
        "crouched %": frac(cat["crouch"]),
        "prone %": frac(cat["prone"]),
        "jumps /min": jumps / max(minutes, 1e-6),
        "-- in fights": None,
        "ADS %": frac(cat["f_ads"]),
        "moving while firing %": frac(cat["f_movefire"]),
        "crouched/prone %": frac(cat["f_crouch"]),
        "strafe flips /s": strafe_rev / max(fight_time, 1e-6),
        "view yaw deg/s p50 ": pct(cat["f_yawspd"], 50),
        "view yaw deg/s p90 ": pct(cat["f_yawspd"], 90),
    }


def group(paths):
    players = {}
    for p in paths:
        runs = load(p)
        if runs:
            players[os.path.basename(p)] = runs
    return players


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--real", nargs="+", required=True)
    ap.add_argument("--bots", nargs="+", required=True)
    a = ap.parse_args()
    real, bots = group(files(a.real)), group(files(a.bots))
    if not real or not bots:
        sys.exit("no tracks found")
    pooled_r = measures([r for runs in real.values() for r in runs])
    pooled_b = measures([r for runs in bots.values() for r in runs])
    # Spread across real players with a few minutes of play.
    per = [measures(runs) for runs in real.values()]
    per = [m for m in per if m["minutes"] >= 2.0]
    print(f"real: {len(real)} players, {pooled_r['minutes']:.0f} min; bots: {len(bots)}, {pooled_b['minutes']:.0f} min")
    print(f"{'':28} {'real':>8} {'players p10-p90':>17} {'bots':>8}")
    for k, v in pooled_r.items():
        if k == "minutes":
            continue
        if v is None:
            print(k)
            continue
        vals = [m[k] for m in per if not math.isnan(m[k])]
        lo, hi = (np.percentile(vals, 10), np.percentile(vals, 90)) if vals else (float("nan"),) * 2
        b = pooled_b[k]
        flag = "" if math.isnan(b) or math.isnan(lo) or lo - 1e-9 <= b <= hi + 1e-9 else "  <-- outside"
        print(f"{k:28} {v:8.2f} {lo:8.2f}-{hi:<8.2f} {b:8.2f}{flag}")


if __name__ == "__main__":
    main()
