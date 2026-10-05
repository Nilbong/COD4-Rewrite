"""Measure aiming behaviour from a gameplay recording and fit the bots' aim
model to it.

Record a session with `COD4RW_RECORD=play.csv` (optionally
`COD4RW_RECORD_WHO=<bot name>` to record a bot instead of yourself), then:

    python tools/fit_aim.py play.csv

For every moment an enemy comes into view with the crosshair off target it
measures the reaction time (until the first purposeful mouse move), the first
ballistic flick (duration, how far it lands relative to the target), the
number of corrective flicks, and the time until the crosshair is on target.
It then fits Fitts' law to the flicks and prints matching `AimProfile`
parameters (crates/game/src/bots/aim.rs).

Checked against bot recordings (whose true parameters are known): the Fitts
fit lands within sampling noise after ~20 flicks. The reaction estimate
includes the time to notice the enemy, and its mean mixes in targets you were
already turning towards (tools/fit_player.py uses a more robust median).
Record several minutes of play for stable numbers.
"""

import csv
import math
import statistics as st
import sys


def wrap(a):
    return (a + 180.0) % 360.0 - 180.0


def load(path):
    rows = []
    with open(path, newline="") as f:
        for r in csv.DictReader(f):
            row = {k: float(r[k]) for k in ("t", "yaw", "pitch")}
            row["fire"] = r["fire"] == "1"
            if r["enemy"]:
                row["enemy"] = r["enemy"]
                row["eyaw"], row["epitch"], row["edist"] = float(r["eyaw"]), float(r["epitch"]), float(r["edist"])
            else:
                row["enemy"] = None
            rows.append(row)
    return rows


def episodes(rows):
    """Runs of frames with the same enemy in view."""
    out, cur = [], []
    for i, r in enumerate(rows):
        if r["enemy"] and cur and rows[cur[-1]]["enemy"] == r["enemy"] and r["t"] - rows[cur[-1]]["t"] < 0.1:
            cur.append(i)
        else:
            if len(cur) > 5:
                out.append(cur)
            cur = [i] if r["enemy"] else []
    if len(cur) > 5:
        out.append(cur)
    return out


def analyse(rows, idx):
    r0 = rows[idx[0]]
    err0 = (wrap(r0["eyaw"] - r0["yaw"]), r0["epitch"] - r0["pitch"])
    amp = math.hypot(*err0)
    width = 2.0 * math.degrees(math.atan(9.0 / max(r0["edist"], 16.0)))
    if amp < max(2.0 * width, 2.0):
        return None
    d0 = (err0[0] / amp, err0[1] / amp)
    speeds, errs, along = [], [], []
    for a, b in zip(idx, idx[1:]):
        ra, rb = rows[a], rows[b]
        dt = max(rb["t"] - ra["t"], 1e-4)
        dv = (wrap(rb["yaw"] - ra["yaw"]), rb["pitch"] - ra["pitch"])
        speed = math.hypot(*dv) / dt
        toward = (dv[0] * d0[0] + dv[1] * d0[1]) / dt
        speeds.append((rb["t"] - r0["t"], speed, toward))
        e = math.hypot(wrap(rb["eyaw"] - rb["yaw"]), rb["epitch"] - rb["pitch"])
        errs.append((rb["t"] - r0["t"], e))
        # Progress along the initial error direction.
        moved = (wrap(rb["yaw"] - r0["yaw"]) * d0[0] + (rb["pitch"] - r0["pitch"]) * d0[1])
        along.append((rb["t"] - r0["t"], moved))
    # Only acquisitions that start from a roughly still view: otherwise the
    # player was already turning and the reaction can't be seen.
    if len(speeds) < 6 or st.mean(s for _, s, _ in speeds[:3]) > 20.0:
        return None
    # The first flick: the first speed peak moving toward the target, from
    # where speed first rises above 5% of that peak to where it falls below.
    first = next((i for i, (t, s, tw) in enumerate(speeds) if s > 40.0 and tw > 30.0), None)
    if first is None:
        return None
    peak_i = first
    while peak_i + 1 < len(speeds) and speeds[peak_i + 1][1] >= speeds[peak_i][1]:
        peak_i += 1
    peak = speeds[peak_i][1]
    start_i = peak_i
    while start_i > 0 and speeds[start_i - 1][1] > 0.05 * peak:
        start_i -= 1
    # It ends where speed bottoms out: below 5% of the peak, or at the first
    # dip under 30% (a corrective flick or tracking takes over).
    end_i = peak_i
    while end_i + 1 < len(speeds):
        nxt = speeds[end_i + 1][1]
        if speeds[end_i][1] < 0.05 * peak or (speeds[end_i][1] < 0.3 * peak and nxt >= speeds[end_i][1]):
            break
        end_i += 1
    onset = speeds[start_i][0]
    end = speeds[end_i][0] if end_i + 1 < len(speeds) else None
    after = speeds[start_i:]
    acquired = next((t for t, e in errs if e < max(width * 0.5, 0.5)), None)
    flicks = 0
    moving = False
    for t, s, _ in after:
        if not moving and s > 40.0:
            flicks += 1
            moving = True
        elif moving and s < 15.0:
            moving = False
    gain = None
    if end is not None:
        landed = next((m for t, m in along if t >= end), None)
        if landed is not None:
            gain = landed / amp
    return {
        "amp": amp,
        "width": width,
        "reaction": onset,
        "flick": (end - onset) if end is not None else None,
        "gain": gain,
        "acquire": acquired,
        "flicks": flicks,
    }


def fit_line(xs, ys):
    mx, my = st.mean(xs), st.mean(ys)
    vx = sum((x - mx) ** 2 for x in xs)
    if vx == 0:
        return my, 0.0
    b = sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / vx
    return my - b * mx, b


def main():
    rows = load(sys.argv[1])
    results = [a for a in (analyse(rows, e) for e in episodes(rows)) if a]
    print(f"{len(rows)} frames, {len(results)} acquisitions")
    if len(results) < 5:
        print("not enough acquisitions to fit; play longer")
        return
    def med(key):
        v = [r[key] for r in results if r[key] is not None]
        return (st.median(v), len(v)) if v else (float("nan"), 0)
    for key in ("reaction", "flick", "gain", "acquire", "flicks", "amp"):
        m, n = med(key)
        print(f"  {key:9} median {m:.3f} (n={n})")
    flicks = [r for r in results if r["flick"] is not None and 0.03 < r["flick"] < 1.0]
    if len(flicks) >= 5:
        ids = [math.log2(r["amp"] / r["width"] + 1.0) for r in flicks]
        a, b = fit_line(ids, [r["flick"] for r in flicks])
        print(f"Fitts' law on first flicks: T = {a:.3f} + {b:.3f} * log2(A/W + 1)")
    gains = [r["gain"] for r in results if r["gain"] is not None and 0.3 < r["gain"] < 1.7]
    print()
    print("Suggested AimProfile (bots/aim.rs):")
    if len(flicks) >= 5:
        print(f"  fitts_a: {max(a, 0.02):.3f}, fitts_b: {max(b, 0.02):.3f},")
    if len(gains) >= 3:
        print(f"  gain_bias: {st.mean(gains):.3f}, gain_sd: {st.pstdev(gains):.3f},")
    print(f"  mean reaction: {st.mean([r['reaction'] for r in results]):.3f}s")


if __name__ == "__main__":
    main()
