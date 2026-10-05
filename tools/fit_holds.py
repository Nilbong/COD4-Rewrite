"""Fit the holding-spot score (`crates/game/src/bots/tactical.rs`'s
`spot_score`) to where real players held, from the game's
`COD4RW_HOLDFIT=<file.csv>` dumps (one run per map with demos):

    python tools/fit_holds.py holdfit.csv

Each row is a candidate spot: its features worked out from the map alone,
and how long real players held near it. A logistic model of "held at least
`HELD` seconds" is fitted on all maps but one and scored on that one (leave
one map out): pick the spots as the game does (best first, `SPACING` apart,
`SPOTS` of them) and measure the share of real holding time within `NEAR`
of a picked spot (recall), and of picked spots real players held at
(precision). The current hand-made score is measured the same way. Prints
the weights fitted on every map, for the game.
"""

import csv
import math
import sys
from collections import defaultdict

import numpy as np

HELD = 3.0
SPACING = 220.0
SPOTS = 80
NEAR = 150.0
NAMES = ["close", "mid", "long", "seen", "traffic", "exposure", "log_lane", "height", "log_spawn", "links"]


def load(path):
    maps = defaultdict(list)
    for r in csv.DictReader(open(path, newline="")):
        maps[r["map"]].append({k: (v if k == "map" else float(v)) for k, v in r.items()})
    return maps


def features(rows):
    """Per map: lane sums scaled by the map's 95th percentile (maps differ
    in size and lane count); distances on a log scale."""
    def p95(k):
        v = sorted(r[k] for r in rows)
        return max(v[int(0.95 * (len(v) - 1))], 1e-6)
    s = {k: p95(k) for k in ("close", "mid", "long", "seen")}
    X = []
    for r in rows:
        X.append([
            min(r["close"] / s["close"], 2.0),
            min(r["mid"] / s["mid"], 2.0),
            min(r["long"] / s["long"], 2.0),
            min(r["seen"] / s["seen"], 2.0),
            r["traffic"],
            r["exposure"],
            math.log1p(r["lane_dist"] / 100.0),
            max(min(r["height"] / 100.0, 3.0), -3.0),
            math.log1p(r["spawn_dist"] / 500.0),
            min(r["links"], 12.0) / 12.0,
        ])
    return np.array(X), s


def fit(X, y, l2=1e-2, steps=4000, lr=0.5):
    mu, sd = X.mean(0), X.std(0) + 1e-9
    Z = (X - mu) / sd
    w = np.zeros(Z.shape[1])
    b = 0.0
    pos = y.mean()
    # Positives are rare: weigh them up to half the loss.
    cw = np.where(y > 0.5, 0.5 / max(pos, 1e-6), 0.5 / max(1 - pos, 1e-6))
    for _ in range(steps):
        p = 1 / (1 + np.exp(-(Z @ w + b)))
        g = cw * (p - y)
        w -= lr * (Z.T @ g / len(y) + l2 * w)
        b -= lr * g.mean()
    # Back to raw features: score = raw @ (w / sd) + (b - mu @ (w / sd)).
    wr = w / sd
    return wr, b - mu @ wr


def baseline(rows):
    """The game's current score (hold_spots): lane traffic by range,
    less in a lane, more in cover; open ground left out."""
    out = []
    for r in rows:
        if r["exposure"] > 0.75 or r["seen"] == 0:
            out.append(-1e9)
            continue
        s = 0.6 * r["close"] + 1.0 * r["mid"] + 0.7 * r["long"]
        s *= 1.0 - 0.6 * r["traffic"]
        s *= 1.25 - 0.5 * r["exposure"]
        out.append(s)
    return np.array(out)


def pick(rows, scores):
    order = np.argsort(-scores)
    chosen = []
    for i in order:
        if scores[i] <= -1e8:
            break
        r = rows[i]
        if all(math.hypot(r["x"] - c["x"], r["y"] - c["y"]) > SPACING or abs(r["z"] - c["z"]) > 60 for c in chosen):
            chosen.append(r)
            if len(chosen) >= SPOTS:
                break
    return chosen


def score_map(rows, chosen):
    # Real holding places: candidates with held time (they overlap; use
    # the held rows as samples of the real time).
    held = [r for r in rows if r["held"] >= HELD]
    total = sum(r["held"] for r in held) or 1.0
    near = lambda r, c: math.hypot(r["x"] - c["x"], r["y"] - c["y"]) < NEAR and abs(r["z"] - c["z"]) < 60
    recall = sum(r["held"] for r in held if any(near(r, c) for c in chosen)) / total
    precision = sum(1 for c in chosen if c["held"] >= HELD or any(near(r, c) for r in held)) / max(len(chosen), 1)
    return recall, precision


def main():
    maps = load(sys.argv[1])
    feats = {m: features(rows)[0] for m, rows in maps.items()}
    labels = {m: np.array([1.0 if r["held"] >= HELD else 0.0 for r in rows]) for m, rows in maps.items()}
    print(f"{'map':16} {'baseline recall/prec':>22} {'fitted recall/prec':>20}   positives")
    tot = np.zeros(4)
    for m in maps:
        others = [k for k in maps if k != m]
        X = np.vstack([feats[k] for k in others])
        y = np.concatenate([labels[k] for k in others])
        w, b = fit(X, y)
        s_fit = feats[m] @ w + b
        rb, pb = score_map(maps[m], pick(maps[m], baseline(maps[m])))
        rf, pf = score_map(maps[m], pick(maps[m], s_fit))
        tot += [rb, pb, rf, pf]
        print(f"{m:16} {rb:10.2f} / {pb:.2f}      {rf:10.2f} / {pf:.2f}   {int(labels[m].sum())}")
    tot /= len(maps)
    print(f"{'mean':16} {tot[0]:10.2f} / {tot[1]:.2f}      {tot[2]:10.2f} / {tot[3]:.2f}")
    X = np.vstack(list(feats.values()))
    y = np.concatenate(list(labels.values()))
    w, b = fit(X, y)
    print("weights on every map (raw features, for spot_score):")
    for n, v in zip(NAMES, w):
        print(f"  {n:10} {v:+.4f}")
    print(f"  bias       {b:+.4f}")


if __name__ == "__main__":
    main()
