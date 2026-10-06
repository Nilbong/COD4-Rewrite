"""Aim training data from gameplay recordings (crates/game/src/bots/record.rs).

Each recording frame has the player's view (yaw, pitch, degrees) and the
enemy nearest their crosshair (id, its chest's yaw and pitch, distance).
A *sighting* is a run of frames on the same enemy. Sightings are resampled
to a fixed rate (DT), and every step becomes one sample:

  in:  error to the target (yaw, pitch), the target's angular velocity,
       the view's own angular velocity just before, time since the target
       was first seen, log distance, aiming down sights, moving
  out: the view's angular velocity over the next step (deg/s)

Frames just after a shot are marked: the game's recoil moves the view
then, not the hand, so the aim model learns from the rest (`recoil`).

Usage: python aim_data.py <recordings dir or files...> -o aim.npz
"""

import argparse
import csv
import glob
import os

import numpy as np

DT = 1.0 / 60.0  # model step (seconds)
RECOIL_AFTER_SHOT = 0.2  # a shot's kick moves the view this long
MAX_SINCE = 1.5  # time-since-seen saturates here


def wrap(a):
    return (a + 180.0) % 360.0 - 180.0


def load(path):
    rows = []
    with open(path, newline="") as f:
        for r in csv.DictReader(f):
            try:
                rows.append(r)
            except Exception:
                pass
    return rows


def sightings(rows):
    """Runs of frames with the same visible enemy, alive, at least 0.25 s."""
    out, cur, prev = [], [], None
    for r in rows:
        e = r.get("enemy", "")
        alive = r.get("dead", "0") == "0"
        if e and alive and e == prev:
            cur.append(r)
        else:
            if len(cur) > 1:
                out.append(cur)
            cur = [r] if (e and alive) else []
        prev = e if (e and alive) else None
    if len(cur) > 1:
        out.append(cur)
    return [s for s in out if float(s[-1]["t"]) - float(s[0]["t"]) >= 0.25]


def resample(seg):
    """The sighting at a fixed rate: unwrapped angles interpolated."""
    t = np.array([float(r["t"]) for r in seg])
    keep = np.concatenate([[True], np.diff(t) > 1e-6])
    seg = [r for r, k in zip(seg, keep) if k]
    t = t[keep]
    col = lambda k: np.array([float(r[k] or 0) for r in seg])
    yaw = np.unwrap(np.radians(col("yaw"))) * 180 / np.pi
    eyaw = np.unwrap(np.radians(col("eyaw"))) * 180 / np.pi
    # The target's yaw continues the view's turn count.
    eyaw += 360.0 * np.round((yaw[0] - eyaw[0]) / 360.0)
    grid = np.arange(t[0], t[-1], DT)
    if len(grid) < 6:
        return None
    lerp = lambda v: np.interp(grid, t, v)
    shots = np.cumsum(col("shots"))
    last_shot = np.full(len(t), -1e9)
    ls = -1e9
    for i in range(len(t)):
        if col("shots")[i] > 0:
            ls = t[i]
        last_shot[i] = ls
    idx = np.searchsorted(t, grid, side="right") - 1
    return {
        "t": grid - t[0],
        "yaw": lerp(yaw),
        "pitch": lerp(col("pitch")),
        "eyaw": lerp(eyaw),
        "epitch": lerp(col("epitch")),
        "dist": lerp(col("edist")),
        "ads": col("ads")[idx],
        "moving": ((np.abs(col("fwd")) + np.abs(col("right"))) > 0)[idx].astype(float),
        "since_shot": grid - last_shot[idx],
    }


def samples(s):
    """Inputs and targets for one resampled sighting."""
    n = len(s["t"])
    vy = np.gradient(s["yaw"], DT)
    vp = np.gradient(s["pitch"], DT)
    tvy = np.gradient(s["eyaw"], DT)
    tvp = np.gradient(s["epitch"], DT)
    X, Y, R = [], [], []
    for i in range(1, n - 1):
        prev_vy = (s["yaw"][i] - s["yaw"][i - 1]) / DT
        prev_vp = (s["pitch"][i] - s["pitch"][i - 1]) / DT
        X.append([
            s["eyaw"][i] - s["yaw"][i],
            s["epitch"][i] - s["pitch"][i],
            tvy[i],
            tvp[i],
            prev_vy,
            prev_vp,
            min(s["t"][i], MAX_SINCE),
            np.log(max(s["dist"][i], 50.0)),
            s["ads"][i],
            s["moving"][i],
        ])
        Y.append([(s["yaw"][i + 1] - s["yaw"][i]) / DT, (s["pitch"][i + 1] - s["pitch"][i]) / DT])
        R.append(s["since_shot"][i] < RECOIL_AFTER_SHOT)
    return np.array(X, np.float32), np.array(Y, np.float32), np.array(R, bool)


FEATURES = ["err_yaw", "err_pitch", "tgt_vyaw", "tgt_vpitch", "vyaw", "vpitch", "since", "log_dist", "ads", "moving"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("inputs", nargs="+")
    ap.add_argument("-o", "--out", default="aim.npz")
    a = ap.parse_args()
    files = []
    for p in a.inputs:
        files += sorted(glob.glob(os.path.join(p, "*.csv"))) if os.path.isdir(p) else [p]
    Xs, Ys, Rs, G = [], [], [], []
    n_sight = 0
    for fi, f in enumerate(files):
        for seg in sightings(load(f)):
            s = resample(seg)
            if s is None:
                continue
            X, Y, R = samples(s)
            if len(X):
                Xs.append(X)
                Ys.append(Y)
                Rs.append(R)
                G.append(np.full(len(X), n_sight))
                n_sight += 1
    X, Y, R, G = (np.concatenate(v) for v in (Xs, Ys, Rs, G))
    np.savez(a.out, X=X, Y=Y, recoil=R, sighting=G, features=np.array(FEATURES), dt=DT)
    print(f"{len(files)} recordings, {n_sight} sightings, {len(X)} samples ({(~R).sum()} outside recoil) -> {a.out}")


if __name__ == "__main__":
    main()
