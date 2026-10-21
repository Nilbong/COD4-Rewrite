"""How the user's fights with bots go, from their own recording (read only):
python tools/duel_stats.py <recording.csv>

A fight starts when an enemy comes into the user's line of sight (the `vis`
column, any facing) after 2 s without them. Per fight: who hit first, how
long each side took from that moment to its first hit, and whether the user
got the kill or died. Also: damage taken with nobody in sight (shot from
unseen), and the user's shots, hits and kills overall."""
import csv
import sys

import numpy as np

rows = list(csv.DictReader(open(sys.argv[1], newline="")))[:-1]
t = np.array([float(r["t"]) for r in rows])
hp = np.array([float(r["health"] or 0) for r in rows])
dead = np.array([r["dead"] == "1" for r in rows])
hits = np.array([int(r["hits"] or 0) for r in rows])
shots = np.array([int(r["shots"] or 0) for r in rows])
kills = np.array([int(r["kills"] or 0) for r in rows])
vis = [set(r["vis"].split(";")) if r["vis"] else set() for r in rows]
took = np.zeros(len(rows), bool)
took[1:] = (hp[1:] < hp[:-1] - 0.5) & ~dead[1:] & ~dead[:-1]
deaths = int(np.sum(dead[1:] & ~dead[:-1]))
print(f"{(t[-1] - t[0]) / 60:.1f} min; kills {kills.sum()}, deaths {deaths}; shots {shots.sum()}, hits {hits.sum()} "
      f"({hits.sum() / max(shots.sum(), 1):.0%})")
last_seen = {}
fights = []
for i in range(len(rows)):
    if dead[i]:
        continue
    for e in vis[i]:
        if t[i] - last_seen.get(e, -99) > 2.0:
            fights.append((i, e))
        last_seen[e] = t[i]
first_user, first_bot, user_t, bot_t, won, lost = 0, 0, [], [], 0, 0
for i, e in fights:
    j_end = np.searchsorted(t, t[i] + 4.0)
    uh = next((j for j in range(i, j_end) if hits[j] > 0), None)
    bh = next((j for j in range(i, j_end) if took[j]), None)
    if uh is None and bh is None:
        continue
    if bh is None or (uh is not None and uh <= bh):
        first_user += 1
    else:
        first_bot += 1
    if uh is not None:
        user_t.append(t[uh] - t[i])
    if bh is not None:
        bot_t.append(t[bh] - t[i])
    k = next((j for j in range(i, j_end) if kills[j] > 0), None)
    d = next((j for j in range(i, j_end) if dead[j]), None)
    if k is not None and (d is None or k < d):
        won += 1
    elif d is not None:
        lost += 1
n = first_user + first_bot
print(f"fights with a hit: {n}; user hit first {first_user / max(n, 1):.0%}; user won {won}, lost {lost}")
print(f"sight to first hit: user p50 {np.median(user_t):.2f}s (n {len(user_t)}), bots on user p50 {np.median(bot_t) if bot_t else float('nan'):.2f}s (n {len(bot_t)})")
unseen = sum(1 for j in np.flatnonzero(took) if not vis[j])
print(f"damage taken with no enemy in sight: {unseen} of {int(took.sum())} hits")
