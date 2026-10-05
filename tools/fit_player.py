"""Build a bot profile from gameplay recordings, so bots play like you.

Play some matches with `--record` (each one writes a CSV under
`recordings/`), then:

    python tools/fit_player.py recordings/*.csv -o profiles/me.txt

and start the game with `--bot-profile profiles/me.txt`. Bots then use your
reaction time and aim speed, strafe and crouch the way you do in fights,
take fights at your range, and hold still or sprint about as much as you.
`--skill 0.5` is your level; higher or lower scales bots from there.

What it measures (see crates/game/src/bots/profile.rs for how bots use it):
  aim        reaction time and first-flick Fitts' law (tools/fit_aim.py)
  fights     strafe press/pause lengths and how often you flip direction,
             by range; hip-fire range; crouching, dropshots, crouch spam,
             jump-shots; pushing or backing off
  playstyle  first-shot range; how much you stand still and sprint out of
             fights; headshot share of hits

A few matches (10+ minutes, 50+ fights) give stable numbers. Anything with
too little data is left out, and bots keep their defaults for it.
"""

import argparse
import csv
import glob
import math
import os
import statistics as st
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fit_aim  # noqa: E402

# Distance bands for fight movement, as in bots/profile.rs (CoD units).
BANDS = (600.0, 1600.0)
BAND_NAMES = ("close", "mid", "far")
# A frame counts as fighting while an enemy is in view and you fired or
# aimed in the last this many seconds.
ENGAGED_FOR = 0.6
MIN_RUNS = 8
# Shorter gaps between strafe keys are just the switch from one key to the
# other (people release A a moment before pressing D), not a pause.
KEY_SWITCH = 0.12
# The game's aim-down-sights model (bots::ads_chance): standing, shots at
# `ads_range` are aimed half the time; on the move, at this many times that.
ADS_MOVING_SCALE = 3.2


def band(dist):
    return sum(dist >= b for b in BANDS)


def load(path):
    rows = []
    with open(path, newline="") as f:
        for r in csv.DictReader(f):
            def num(k, default=0.0):
                v = r.get(k)
                return float(v) if v not in (None, "") else default

            row = {
                "t": num("t"), "yaw": num("yaw"), "pitch": num("pitch"),
                "fire": r.get("fire") == "1", "ads": r.get("ads") == "1",
                "fwd": num("fwd"), "right": num("right"),
                "enemy": r["enemy"] or None,
                "x": num("x"), "y": num("y"), "z": num("z"),
                "stance": int(num("stance")), "sprint": r.get("sprint") == "1",
                "jump": r.get("jump") == "1", "dead": r.get("dead") == "1",
                "shots": int(num("shots")), "hits": int(num("hits")),
                "heads": int(num("heads")), "kills": int(num("kills")),
            }
            if row["enemy"]:
                row["eyaw"], row["epitch"], row["edist"] = num("eyaw"), num("epitch"), num("edist")
            rows.append(row)
    return rows


def fight_segments(rows):
    """Runs of frames spent fighting (see ENGAGED_FOR), as index lists."""
    segs, cur, last_engaged = [], [], -1e9
    for i, r in enumerate(rows):
        if r["dead"]:
            last_engaged = -1e9
        if r["enemy"] and (r["fire"] or r["ads"] or r["shots"]):
            last_engaged = r["t"]
        fighting = not r["dead"] and r["enemy"] and r["t"] - last_engaged < ENGAGED_FOR
        if fighting and cur and r["t"] - rows[cur[-1]]["t"] < 0.25:
            cur.append(i)
        else:
            if len(cur) > 10:
                segs.append(cur)
            cur = [i] if fighting else []
    if len(cur) > 10:
        segs.append(cur)
    return segs


def runs(rows, seg):
    """Strafe key runs inside a fight: (direction, duration, band), leaving
    out the first and last (cut off by the fight's edges)."""
    out, start = [], 0
    for k in range(1, len(seg) + 1):
        if k == len(seg) or rows[seg[k]]["right"] != rows[seg[start]]["right"]:
            a, b = rows[seg[start]], rows[seg[k - 1]]
            mid = rows[seg[(start + k - 1) // 2]]
            end = rows[seg[k]]["t"] if k < len(seg) else b["t"]
            out.append((a["right"], end - a["t"], band(mid["edist"])))
            start = k
    # Fold key-switch gaps into the strafes around them: between opposite
    # strafes it's a flip, between same-direction ones one longer press.
    merged = []
    for i, run in enumerate(out):
        d, dur, bd = run
        gap = d == 0 and dur < KEY_SWITCH and 0 < i < len(out) - 1 and out[i - 1][0] != 0 and out[i + 1][0] != 0
        if gap:
            pd, pdur, pbd = merged[-1]
            merged[-1] = (pd, pdur + dur, pbd)
        elif merged and d != 0 and merged[-1][0] == d:
            pd, pdur, pbd = merged[-1]
            merged[-1] = (pd, pdur + dur, pbd)
        else:
            merged.append(run)
    return merged[1:-1]


def measure(all_rows):
    p, info = {}, {}
    segs = [(rows, s) for rows in all_rows for s in fight_segments(rows)]
    fight_time = sum(rows[s[-1]]["t"] - rows[s[0]]["t"] for rows, s in segs)
    info["fights"] = len(segs)
    info["fight minutes"] = round(fight_time / 60.0, 1)

    # Aim, with fit_aim's analysis.
    acq = [a for rows in all_rows for a in (fit_aim.analyse(rows, e) for e in fit_aim.episodes(rows)) if a]
    info["acquisitions"] = len(acq)
    # Reaction: the median of the real ones. Under 0.12 s you were already
    # turning towards someone you knew about; past 1.5 s you hadn't noticed.
    reactions = [a["reaction"] for a in acq if 0.12 < a["reaction"] < 1.5]
    if len(reactions) >= 5:
        p["reaction"] = st.median(reactions)
    flicks = [a for a in acq if a["flick"] is not None and 0.03 < a["flick"] < 1.0]
    if len(flicks) >= 8:
        a, b = fit_aim.fit_line([math.log2(f["amp"] / f["width"] + 1.0) for f in flicks], [f["flick"] for f in flicks])
        p["fitts_a"], p["fitts_b"] = max(a, 0.0), max(b, 0.01)
    gains = [a["gain"] for a in acq if a["gain"] is not None and 0.3 < a["gain"] < 1.7]
    if len(gains) >= 8:
        p["gain_bias"], p["gain_sd"] = st.mean(gains), st.pstdev(gains)

    # Strafing, by range band.
    press, pause, ends = ([[] for _ in BAND_NAMES] for _ in range(3))
    for rows, s in segs:
        rs = runs(rows, s)
        for i, (d, dur, bd) in enumerate(rs):
            if d == 0:
                pause[bd].append(dur)
            else:
                press[bd].append(dur)
                if i + 1 < len(rs):
                    ends[bd].append(rs[i + 1][0] == -d)
    for bd, name in enumerate(BAND_NAMES):
        if len(press[bd]) >= MIN_RUNS:
            p[f"{name}_strafe_press"] = st.mean(press[bd])
        if len(pause[bd]) >= MIN_RUNS:
            p[f"{name}_strafe_pause"] = st.mean(pause[bd])
        if len(ends[bd]) >= MIN_RUNS:
            p[f"{name}_strafe_flip"] = sum(ends[bd]) / len(ends[bd])

    # Aiming down sights: the distance at which half the shots fired
    # standing still are aimed, by maximum likelihood under the game's model
    # (1 / (1 + (range / distance)^1.5), the range ADS_MOVING_SCALE times
    # further on the move).
    shots = []
    for rows in all_rows:
        for prev, r in zip(rows, rows[1:]):
            if r["shots"] and r["enemy"]:
                dt = max(r["t"] - prev["t"], 1e-3)
                moving = math.hypot(r["x"] - prev["x"], r["y"] - prev["y"]) / dt > 40.0
                shots.append((max(r["edist"], 1.0), moving, r["ads"]))
    info["shots in view"] = len(shots)
    if len(shots) >= 20:
        def loglik(half):
            total = 0.0
            for dist, moving, ads in shots:
                h = half * ADS_MOVING_SCALE if moving else half
                chance = 1.0 if h <= 0 else 1.0 / (1.0 + (h / dist) ** 1.5)
                chance = min(max(chance, 1e-4), 1.0 - 1e-4)
                total += math.log(chance if ads else 1.0 - chance)
            return total
        p["ads_range"] = max(range(0, 3001, 10), key=loglik)

    # Stance and jumps in fights.
    if segs:
        crouched = [st.mean(rows[i]["stance"] == 1 for i in s) > 0.5 for rows, s in segs]
        p["fight_crouch"] = sum(crouched) / len(crouched)
        close = [(rows, s) for rows, s in segs if rows[s[0]]["edist"] < 650.0]
        if len(close) >= 5:
            def dropped(rows, s):
                return any(rows[i]["stance"] == 2 for i in s if rows[i]["t"] - rows[s[0]]["t"] < 1.0)
            p["dropshot"] = sum(dropped(rows, s) for rows, s in close) / len(close)
        changes = close_time = 0.0
        for rows, s in segs:
            for a, b in zip(s, s[1:]):
                if rows[a]["edist"] < 500.0:
                    close_time += rows[b]["t"] - rows[a]["t"]
                    changes += rows[a]["stance"] != rows[b]["stance"]
        if close_time > 10.0:
            p["crouch_taps"] = changes / close_time
        if fight_time > 30.0:
            jumps = sum(rows[b]["jump"] and not rows[a]["jump"] for rows, s in segs for a, b in zip(s, s[1:]))
            p["fight_jumps"] = jumps / (fight_time / 60.0)
            p["push"] = st.mean(rows[i]["fwd"] for rows, s in segs for i in s)

    # First-shot range.
    first = []
    for rows, s in segs:
        shot = next((rows[i]["edist"] for i in s if rows[i]["shots"]), None)
        if shot is not None:
            first.append(shot)
    if len(first) >= 5:
        p["preferred_range"] = st.median(first)

    # Out of fights: standing still and sprinting.
    still = moving = sprinting = total = 0
    for rows in all_rows:
        in_fight = set(i for s in fight_segments(rows) for i in s)
        for i in range(len(rows)):
            j = i
            while j + 1 < len(rows) and rows[j]["t"] - rows[i]["t"] < 0.25:
                j += 1
            a, b = rows[i], rows[j]
            dt = b["t"] - a["t"]
            if i in in_fight or a["dead"] or b["dead"] or dt <= 0.05:
                continue
            speed = math.hypot(b["x"] - a["x"], b["y"] - a["y"]) / dt
            if speed > 600.0:  # respawned
                continue
            total += 1
            still += speed < 15.0
            if speed > 60.0:
                moving += 1
                sprinting += a["sprint"]
    if total > 200:
        p["stillness"] = still / total
    if moving > 200:
        p["sprint"] = sprinting / moving

    hits = sum(r["hits"] for rows in all_rows for r in rows)
    fired = sum(r["shots"] for rows in all_rows for r in rows)
    if hits >= 20:
        p["head_rate"] = sum(r["heads"] for rows in all_rows for r in rows) / hits
    info["accuracy"] = round(hits / fired, 3) if fired else None
    info["kills"] = sum(r["kills"] for rows in all_rows for r in rows)
    return p, info


ORDER = ["reaction", "fitts_a", "fitts_b", "gain_bias", "gain_sd"] + [
    f"{b}_strafe_{k}" for b in BAND_NAMES for k in ("press", "pause", "flip")
] + ["ads_range", "fight_crouch", "dropshot", "crouch_taps", "fight_jumps", "push",
     "preferred_range", "stillness", "sprint", "head_rate"]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("recordings", nargs="+")
    ap.add_argument("-o", "--out", help="profile file to write (default: print)")
    args = ap.parse_args()
    # Expand wildcards here too: PowerShell passes them through as typed.
    paths = [p for arg in args.recordings for p in (sorted(glob.glob(arg)) or [arg])]
    all_rows = [load(path) for path in paths]
    minutes = sum((r[-1]["t"] - r[0]["t"]) / 60.0 for r in all_rows if r)
    p, info = measure(all_rows)
    lines = [f"# Bot profile fitted from {len(all_rows)} recording(s), {minutes:.1f} minutes of play.",
             "# " + ", ".join(f"{k} {v}" for k, v in info.items())]
    lines += [f"{k} = {p[k]:.4g}" for k in ORDER if k in p]
    missing = [k for k in ORDER if k not in p]
    if missing:
        lines.append("# not enough data for: " + ", ".join(missing))
    text = "\n".join(lines) + "\n"
    if args.out:
        os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
        with open(args.out, "w") as f:
            f.write(text)
        print(f"wrote {args.out}")
    print(text, end="")


if __name__ == "__main__":
    main()
