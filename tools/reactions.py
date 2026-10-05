"""How players react to being shot from behind: how often and how fast they
turn round, fire back, or die first.

Takes a folder of CSV tracks from one match, all on one clock and with a
team column: a demo's players (`iw3 --example demodump`) or every bot in a
sim (`COD4RW_RECORD=<dir>/x.csv COD4RW_RECORD_WHO=all`).

    python tools/reactions.py <dir> [<dir>...]

An attack from behind: someone starts firing at an enemy (in their
crosshair, within 6 degrees and 2000 units) who is facing more than 110
degrees away from them. Then over 3 s: does the victim face the shooter
(within 30 degrees), fire back while facing them (within 15), or die (or
vanish) first?
"""

import csv
import glob
import math
import os
import sys

import numpy as np

RATE = 20
WINDOW = 3.0
AIM_CONE = 6.0
BEHIND = 110.0
TURNED = 30.0
FIRE_BACK = 15.0
RANGE = 2000.0


def load(path):
    """{tick: (team, x, y, z, yaw_cod, fire, dead)} at 20 Hz on the file's clock."""
    out = {}
    fired = {}
    with open(path, newline="") as f:
        for r in csv.DictReader(f):
            if not r.get("team"):
                continue
            k = int(round(float(r["t"]) * RATE))
            fire = r["fire"] == "1"
            fired[k] = fired.get(k, False) or fire
            out[k] = (
                int(r["team"]),
                float(r["x"]),
                float(r["y"]),
                float(r["z"]),
                float(r["yaw"]) + 90.0,  # the recorder's yaw is CoD's - 90
                fired[k],
                r.get("dead") == "1",
            )
    return out


def wrap(a):
    return (a + 180.0) % 360.0 - 180.0


def bearing(a, b):
    return math.degrees(math.atan2(b[2] - a[2], b[1] - a[1]))


def analyse(folder):
    players = {os.path.basename(p): load(p) for p in sorted(glob.glob(os.path.join(folder, "*.csv")))}
    players = {k: v for k, v in players.items() if v}
    ticks = sorted(set().union(*[set(v) for v in players.values()]))
    events = []
    last_attack = {}
    for k in ticks:
        here = {n: s[k] for n, s in players.items() if k in s and not s[k][6]}
        for a, sa in here.items():
            if not sa[5]:
                continue
            # Who's in the shooter's crosshair?
            best = None
            for b, sb in here.items():
                if sb[0] == sa[0] or sb[0] == 0 or sa[0] == 0:
                    continue
                d = math.hypot(sb[1] - sa[1], sb[2] - sa[2])
                if d > RANGE or abs(wrap(bearing(sa, sb) - sa[4])) > AIM_CONE:
                    continue
                if best is None or d < best[1]:
                    best = (b, d)
            if best is None:
                continue
            b = best[0]
            # A new attack, not more of the same one.
            if k - last_attack.get((a, b), -10**9) < WINDOW * RATE:
                last_attack[(a, b)] = k
                continue
            last_attack[(a, b)] = k
            sb = here[b]
            if abs(wrap(bearing(sb, sa) - sb[4])) < BEHIND:
                continue
            events.append(outcome(players[a], players[b], k))
    return events


def outcome(shooter, victim, k0):
    """(turned after s or None, fired back after s or None, died first)."""
    turned = fired = None
    prev = victim[k0]
    for k in range(k0 + 1, k0 + int(WINDOW * RATE)):
        v = victim.get(k)
        if v is None or v[6] or math.hypot(v[1] - prev[1], v[2] - prev[2]) > 60:
            return (turned, fired, turned is None)
        prev = v
        s = shooter.get(k)
        if s is None:
            continue
        off = abs(wrap(bearing(v, s) - v[4]))
        t = (k - k0) / RATE
        if turned is None and off < TURNED:
            turned = t
        if fired is None and v[5] and off < FIRE_BACK:
            fired = t
    return (turned, fired, False)


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    for folder in sys.argv[1:]:
        ev = analyse(folder)
        n = len(ev)
        if not n:
            print(f"{folder}: no attacks from behind")
            continue
        turned = [e[0] for e in ev if e[0] is not None]
        fired = [e[1] for e in ev if e[1] is not None]
        died = sum(1 for e in ev if e[2])
        print(f"{os.path.basename(folder.rstrip('/'))}: {n} attacks from behind")
        print(f"  turned to face the shooter {100 * len(turned) / n:.0f}%"
              + (f", after p25 {np.percentile(turned, 25):.2f}s p50 {np.percentile(turned, 50):.2f}s"
                 f" p75 {np.percentile(turned, 75):.2f}s" if turned else ""))
        print(f"  fired back {100 * len(fired) / n:.0f}%"
              + (f", after p50 {np.percentile(fired, 50):.2f}s" if fired else ""))
        print(f"  died or gone before turning {100 * died / n:.0f}%;"
              f" neither within {WINDOW:.0f}s {100 * (n - len(turned) - died) / n:.0f}%")


if __name__ == "__main__":
    main()
