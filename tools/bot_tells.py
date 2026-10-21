"""The "mindless bot" tells and their acceptance targets
(data/plans/2026-10-19/bots_critique.md), from recordings (bots from a sim
with COD4RW_RECORD_WHO=all, or the user's own):

    python tools/bot_tells.py <dir|csv>...

- opening shots (the first in a fight: an enemy in sight after 2 s
  without): partial ADS (0 < sights < 1; target under 3%), hip (sights 0)
  by gun class and range against the C2 table (within 0.1)
- bursts (shots under 0.3 s apart) fired with nobody in sight in the last
  second (target under 3%)
- stares: view_dist under 120 units (a floor or wall right there) held over
  0.5 s outside fights (target: none outside stairs)
"""

import collections
import csv
import glob
import os
import sys

C2 = {"smg": (.64, .38, .27), "rifle": (.62, .26, .16), "lmg": (.74, .60, .49), "pistol": (.70, .43, .34), "shotgun": (.98, .95, .95)}
GUNS = {
    "smg": ("mp5", "p90", "uzi", "ak74u", "skorpion"),
    "rifle": ("m16", "ak47", "m4", "g36c", "g3", "m14", "mp44"),
    "lmg": ("rpd", "saw", "m60e4"),
    "pistol": ("beretta", "colt45", "usp", "deserteagle"),
    "shotgun": ("winchester1200", "m1014"),
    "sniper": ("barrett", "m40a3", "m21", "dragunov", "remington700"),
}


def gun_class(w):
    w = w.replace("iw4_", "")
    return next((k for k, g in GUNS.items() if any(w.startswith(x) for x in g)), "other")


def files_of(arg):
    if os.path.isdir(arg):
        return [f for f in sorted(glob.glob(os.path.join(arg, "**", "*.csv"), recursive=True)) if not f.endswith("-personas.csv")]
    return [arg]


for arg in sys.argv[1:]:
    openings = partial = 0
    hip = collections.defaultdict(lambda: [0, 0])
    bursts = blind_bursts = 0
    stares = 0
    minutes = 0.0
    for f in files_of(arg):
        rows = list(csv.DictReader(open(f, newline="")))[:-1]
        if len(rows) < 100 or "vis" not in rows[0]:
            continue
        t = [float(r["t"]) for r in rows]
        minutes += (t[-1] - t[0]) / 60.0
        seen_any = -99.0
        last_shot = -99.0
        last_seen = {}
        pending = []  # fights waiting for their first shot: (start, dist)
        stare_from = None
        for i, r in enumerate(rows):
            if r["dead"] == "1":
                pending.clear()
                stare_from = None
                continue
            vis = r["vis"].split(";") if r["vis"] else []
            if vis:
                seen_any = t[i]
            for e in vis:
                if t[i] - last_seen.get(e, -99) > 2.0:
                    pending.append(t[i])
                last_seen[e] = t[i]
            n = int(r["shots"] or 0)
            ads = float(r.get("ads_level") or 0)
            if n:
                if t[i] - last_shot > 0.3:
                    bursts += 1
                    if t[i] - seen_any > 1.0:
                        blind_bursts += 1
                last_shot = t[i]
                pending = [p for p in pending if t[i] - p < 4.0]
                if pending and r["edist"]:
                    openings += 1
                    partial += 0 < ads < 1
                    d = float(r["edist"])
                    col = 0 if d < 394 else 1 if d < 985 else 2
                    k = gun_class(r.get("weapon", ""))
                    if k in C2:
                        hip[(k, col)][0] += 1
                        hip[(k, col)][1] += ads == 0
                    pending.clear()
            # Stares out of fights.
            # (Bots' own fight modes count as fights too: holding where a
            # target ducked out of sight is not a stare.)
            calm = not vis and t[i] - last_shot > 1.0 and r.get("mode", "") not in ("Engage", "Cover")
            vd = float(r.get("view_dist") or 9999)
            if calm and vd < 120:
                if stare_from is None:
                    stare_from = t[i]
            else:
                if stare_from is not None and t[i] - stare_from > 0.5:
                    stares += 1
                stare_from = None
    print(f"== {arg}: {minutes:.1f} min of play")
    print(f"   openings {openings}: partial ADS {partial / max(openings, 1):.1%} (target <3%)")
    for (k, col), (nn, h) in sorted(hip.items()):
        if nn >= 5:
            print(f"     hip {k:7} {['<10m', '10-25m', '25-50m'][col]:7} {h / nn:.2f} (C2 {C2[k][col]:.2f}, n {nn})")
    print(f"   bursts {bursts}: with nobody seen in the last 1 s {blind_bursts / max(bursts, 1):.1%} (target <3%)")
    print(f"   floor/wall stares >0.5 s outside fights: {stares} ({stares / max(minutes, 1e-6):.1f}/min per player-file)")
