"""Bot lab: run a fixed suite of bot matches and score how close the bots
play to real players (the CoD4X demos' tracks in data/demo_tracks/real).

    python tools/botlab.py run [--label NAME] [--secs 180] [--only mp_x,...]
        copy the built game (target/supply/release/cod4rw.exe), play each
        suite match headless (COD4RW_SIM, every bot recorded), one at a
        time, and score it: target/botlab/<time>-<label>/ with each match's
        log and tracks, scores.json and summary.md
    python tools/botlab.py score <run dir>        re-score a run
    python tools/botlab.py compare <run a> <run b>  metric by metric

Scores: each metric's distance from real players, in units of how much
real players themselves differ (half their p10-p90 spread, or a stated
tolerance), capped at 3. The total is the mean over every metric of every
map, so lower is better. Guards (not scored, but a change shouldn't make
them worse): bots getting stuck, kills per minute, flags taken.
"""

import argparse
import bisect
import csv
import glob
import json
import math
import os
import re
import shutil
import statistics as st
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WORK = os.path.join(ROOT, "target", "demo_work")
# The real players' tracks and the maps' flags (kept out of target/).
REAL = os.path.join(ROOT, "data", "demo_tracks", "real")
FLAGS = os.path.join(ROOT, "data", "demo_tracks", "flags.txt")
LAB = os.path.join(ROOT, "target", "botlab")
EXE = os.path.join(ROOT, "target", "supply", "release", "cod4rw.exe")

# (map, game type, bots a side, the demos of real play on it)
SUITE = [
    ("mp_killhouse", "war", 6, ["demo0001"]),
    ("mp_crossfire", "war", 8, ["demo0002"]),
    ("mp_pipeline", "war", 10, ["demo0005"]),
    ("mp_broadcast", "dom", 6, ["demo0003"]),
    ("mp_citystreets", "dom", 5, ["demo0004"]),
    ("mp_cargoship", "dom", 8, ["demo0000", "demo0006"]),
]

# The user's own recording (recordings/mp_killhouse-*.csv): seen -> first
# shot on fresh acquisitions, p50 0.51 s (p25 0.29, p75 0.70); accuracy 27%.
FIRST_SHOT = (0.51, 0.2)
ACCURACY = (0.27, 0.08)


def real_files(demos):
    return [f for d in demos for f in glob.glob(os.path.join(REAL, d + "-*.csv")) if not f.endswith("-Shane.csv")]


# ---------------------------------------------------------------- tracks

def load(files):
    tracks = {}
    for f in files:
        try:
            rows = list(csv.DictReader(open(f, newline="")))
        except OSError:
            continue
        out = []
        for r in rows:
            try:
                out.append((
                    float(r["t"]), float(r["x"]), float(r["y"]), float(r["z"]), float(r["yaw"]), float(r["pitch"]),
                    r["fire"] == "1", r.get("team", ""), r.get("dead", "0") not in ("0", ""), r.get("ads", "0") == "1",
                    int(r.get("shots") or 0), int(r.get("hits") or 0), int(r.get("kills") or 0),
                ))
            except (KeyError, ValueError):
                continue
        if out:
            tracks[f] = out
    return tracks


def track_measures(tracks):
    """Looking away from the way walked, firing distances, accuracy, kills."""
    walk = over90 = back = 0.0
    alive = fire = idle = 0.0
    shots = hits = kills = 0
    fire_d = []
    for f, tr in tracks.items():
        last_fire = -1.0
        for a, b in zip(tr, tr[1:]):
            t, x, y, z, yaw, pitch, firing, team, dead, ads, s, h, k = a
            shots += s
            hits += h
            kills += k
            if dead:
                continue
            dt = b[0] - t
            if not 0.01 < dt < 0.5:
                continue
            alive += dt
            if firing:
                fire += dt
            elif ads:
                idle += dt
            dx, dy = b[1] - x, b[2] - y
            if not firing and math.hypot(dx, dy) / dt > 80:
                off = abs((yaw + 90.0 - math.degrees(math.atan2(dy, dx)) + 180) % 360 - 180)
                walk += dt
                over90 += dt if off > 90 else 0
                back += dt if off > 135 else 0
            # The enemy nearest the crosshair when firing (4 a second).
            if firing and t - last_fire >= 0.25:
                last_fire = t
                vy, vp = math.radians(yaw + 90), math.radians(pitch)
                v = (math.cos(vy) * math.cos(vp), math.sin(vy) * math.cos(vp), math.sin(vp))
                best = None
                for g, o in tracks.items():
                    if g == f:
                        continue
                    i = bisect.bisect_left(o, (t,))
                    if i >= len(o) or abs(o[i][0] - t) > 0.2 or o[i][8] or o[i][7] == team:
                        continue
                    e = o[i]
                    ex, ey, ez = e[1] - x, e[2] - y, e[3] + 40 - (z + 60)
                    d = math.sqrt(ex * ex + ey * ey + ez * ez)
                    if d > 50 and (ex * v[0] + ey * v[1] + ez * v[2]) / d > math.cos(math.radians(6)):
                        best = d if best is None else min(best, d)
                if best:
                    fire_d.append(best)
    m = {}
    if walk > 0:
        m["look >90 off walk %"] = 100 * over90 / walk
        m["look backwards %"] = 100 * back / walk
    if alive > 0:
        m["firing %"] = 100 * fire / alive
        m["sights up idle %"] = 100 * idle / alive
    if len(fire_d) > 20:
        m["fire dist p50"] = st.median(fire_d)
        m["fire beyond 1500 %"] = 100 * sum(d > 1500 for d in fire_d) / len(fire_d)
    m["_shots"] = shots
    m["_hits"] = hits
    m["_kills_per_min"] = kills / (alive / 60) if alive else 0
    return m


# ------------------------------------------------------- other tools' output

def compare_play(real_dir, bots_dir):
    """compare_play.py's rows: name -> (real, p10, p90, bots)."""
    out = subprocess.run([sys.executable, os.path.join(ROOT, "tools", "compare_play.py"), "--real", real_dir, "--bots", bots_dir],
                         capture_output=True, text=True).stdout
    rows = {}
    section = ""
    for line in out.splitlines():
        if line.startswith("--"):
            section = "fight " if "in fights" in line else ""
            continue
        m = re.match(r"^(\S.*?)\s{2,}(-?[\d.]+)\s+(-?[\d.]+)-(-?[\d.]+)\s+(-?[\d.]+)", line)
        if m:
            name = section + m.group(1).strip()
            rows[name] = tuple(float(m.group(i)) for i in range(2, 6))
    return rows


def dom_play(map_name, files):
    if not files:
        return {}
    out = subprocess.run([sys.executable, os.path.join(ROOT, "tools", "dom_play.py"), "--flags", FLAGS, "--map", map_name] + files,
                         capture_output=True, text=True).stdout
    m = {}
    g = re.search(r"time on a flag ([\d.]+)%, within 600 of one ([\d.]+)%", out)
    if g:
        m["dom on flag %"], m["dom within 600 %"] = float(g.group(1)), float(g.group(2))
    g = re.search(r"visits ([\d.]+)/min, lasting median ([\d.]+)s", out)
    if g:
        m["dom visits /min"], m["dom visit s"] = float(g.group(1)), float(g.group(2))
    g = re.search(r"together: 1: (\d+)%", out)
    if g:
        m["dom alone %"] = float(g.group(1))
    return m


def log_measures(log):
    m = {}
    try:
        text = re.sub(r"\x1b\[[0-9;]*m", "", open(log, encoding="utf-8", errors="replace").read())
    except OSError:
        return m
    shots = [float(x) for x in re.findall(r"seen->shot ([\d.]+)s", text)]
    if len(shots) > 10:
        m["first shot s p50"] = st.median(shots)
    stuck = re.findall(r"behaviour: .*?stuck \{([^}]*)\}", text)
    if stuck:
        m["_stuck"] = sum(int(v) for v in re.findall(r":\s*(\d+)", stuck[-1]))
    m["_flags_taken"] = len(re.findall(r"dom: \w+ took", text))
    m["_panics"] = len(re.findall(r"panicked", text))
    return m


# ----------------------------------------------------------------- scoring

def dist(bot, real, unit):
    return min(abs(bot - real) / max(unit, 1e-6), 3.0)


def score_map(run_dir, map_name, mode, demos):
    bots_dir = os.path.join(run_dir, map_name)
    bot_files = glob.glob(os.path.join(bots_dir, "*.csv"))
    rfiles = real_files(demos)
    metrics = {}
    if not bot_files or not rfiles:
        return {"metrics": metrics, "guards": {}, "error": "no tracks"}
    real_tmp = os.path.join(run_dir, "_real_" + map_name)
    os.makedirs(real_tmp, exist_ok=True)
    for f in rfiles:
        shutil.copy(f, real_tmp)
    for name, (real, p10, p90, bots) in compare_play(real_tmp, bots_dir).items():
        metrics[name] = {"bots": bots, "real": real, "p10": p10, "p90": p90, "score": dist(bots, real, (p90 - p10) / 2)}
    rm, bm = track_measures(load(rfiles)), track_measures(load(bot_files))
    for name in ("look >90 off walk %", "look backwards %", "firing %", "sights up idle %", "fire dist p50", "fire beyond 1500 %"):
        if name in rm and name in bm:
            unit = {"fire dist p50": 0.2 * rm[name], "fire beyond 1500 %": 8.0}.get(name, max(0.35 * rm[name], 2.0))
            metrics[name] = {"bots": bm[name], "real": rm[name], "score": dist(bm[name], rm[name], unit)}
    if mode == "dom":
        rd, bd = dom_play(map_name, rfiles), dom_play(map_name, bot_files)
        for name in rd:
            if name in bd:
                metrics[name] = {"bots": bd[name], "real": rd[name], "score": dist(bd[name], rd[name], max(0.4 * rd[name], 0.5))}
    lm = log_measures(os.path.join(run_dir, map_name + ".log"))
    if "first shot s p50" in lm:
        metrics["first shot s p50"] = {"bots": lm["first shot s p50"], "real": FIRST_SHOT[0], "score": dist(lm["first shot s p50"], *FIRST_SHOT)}
    if bm["_shots"] > 50:
        acc = bm["_hits"] / bm["_shots"]
        metrics["accuracy"] = {"bots": acc, "real": ACCURACY[0], "score": dist(acc, *ACCURACY)}
    guards = {"stuck": lm.get("_stuck"), "kills/min": round(bm["_kills_per_min"], 3), "flags taken": lm.get("_flags_taken"), "panics": lm.get("_panics")}
    return {"metrics": metrics, "guards": guards}


def score_run(run_dir):
    result = {"maps": {}}
    for map_name, mode, _, demos in SUITE:
        if os.path.isdir(os.path.join(run_dir, map_name)):
            result["maps"][map_name] = score_map(run_dir, map_name, mode, demos)
    scores = [m["score"] for r in result["maps"].values() for m in r["metrics"].values()]
    result["total"] = sum(scores) / len(scores) if scores else None
    json.dump(result, open(os.path.join(run_dir, "scores.json"), "w"), indent=1)
    write_summary(run_dir, result)
    return result


def write_summary(run_dir, result):
    lines = [f"# Bot lab run {os.path.basename(run_dir)}", "", f"Total distance from real players: **{result['total']:.3f}** (lower is better)", ""]
    for map_name, r in result["maps"].items():
        ms = r["metrics"]
        mean = sum(m["score"] for m in ms.values()) / max(len(ms), 1)
        lines += [f"## {map_name}: {mean:.3f}", "", f"Guards: {r['guards']}", "", "| metric | bots | real | p10-p90 | score |", "|---|---|---|---|---|"]
        for name, m in sorted(ms.items(), key=lambda kv: -kv[1]["score"]):
            rng = f"{m['p10']:.2f}-{m['p90']:.2f}" if "p10" in m else ""
            lines.append(f"| {name} | {m['bots']:.3f} | {m['real']:.3f} | {rng} | {m['score']:.2f} |")
        lines.append("")
    worst = sorted(((m["score"], mp, n) for mp, r in result["maps"].items() for n, m in r["metrics"].items()), reverse=True)[:12]
    lines += ["## Furthest from real", ""] + [f"- {s:.2f} {mp}: {n}" for s, mp, n in worst]
    open(os.path.join(run_dir, "summary.md"), "w").write("\n".join(lines) + "\n")


# --------------------------------------------------------------------- run

def run(label, secs, only, exe_from=EXE, lab=""):
    stamp = time.strftime("%Y%m%d-%H%M%S")
    run_dir = os.path.join(LAB, f"{stamp}-{label}" if label else stamp)
    os.makedirs(run_dir)
    exe = os.path.join(run_dir, "sim.exe")
    shutil.copy(exe_from, exe)
    env = dict(os.environ, COD4RW_SIM=str(secs), COD4RW_RECORD_WHO="all", COD4RW_BOTLAB=lab)
    with open(os.path.join(run_dir, "lab.txt"), "w") as f:
        f.write(f"experiments: {lab or '(none)'}\n")
        f.write(f"exe: {exe_from}\n")
    for map_name, mode, bots, _ in SUITE:
        if only and map_name not in only:
            continue
        out_dir = os.path.join(run_dir, map_name)
        os.makedirs(out_dir)
        env["COD4RW_RECORD"] = os.path.join(out_dir, "bots.csv")
        t0 = time.time()
        with open(os.path.join(run_dir, map_name + ".log"), "w") as log:
            try:
                subprocess.run([exe, "--map", map_name, "--mode", mode, "--bots", str(bots), "--spectate"], env=env, stdout=log,
                               stderr=subprocess.STDOUT, timeout=secs * 3 + 300)
            except subprocess.TimeoutExpired:
                log.write("\nbotlab: timed out\n")
        print(f"{map_name}: {time.time() - t0:.0f}s", flush=True)
    os.remove(exe)
    result = score_run(run_dir)
    print(f"total {result['total']:.3f}  ->  {run_dir}")
    return run_dir


def compare(a, b):
    ra, rb = (json.load(open(os.path.join(d, "scores.json"))) for d in (a, b))
    print(f"total {ra['total']:.3f} -> {rb['total']:.3f}")
    for mp in ra["maps"]:
        if mp not in rb["maps"]:
            continue
        ma, mb = ra["maps"][mp], rb["maps"][mp]
        print(f"== {mp}  guards {ma['guards']} -> {mb['guards']}")
        for n, m in ma["metrics"].items():
            if n in mb["metrics"]:
                d = mb["metrics"][n]["score"] - m["score"]
                if abs(d) >= 0.15:
                    print(f"   {n:28s} {m['bots']:9.3f} -> {mb['metrics'][n]['bots']:9.3f} (real {m['real']:.3f})  score {m['score']:.2f} -> {mb['metrics'][n]['score']:.2f}")


def group(a_dirs, b_dirs, focus=None):
    """Average two sets of runs metric by metric: (mean, spread) of each
    side's totals, and the metrics that moved by more than their own spread."""
    def load_all(dirs):
        return [json.load(open(os.path.join(d, "scores.json"))) for d in dirs]
    A, B = load_all(a_dirs), load_all(b_dirs)
    def tot(rs):
        t = [r["total"] for r in rs]
        return st.mean(t), (st.stdev(t) if len(t) > 1 else 0.0)
    (ma, sa), (mb, sb) = tot(A), tot(B)
    print(f"total  A {ma:.3f} (+-{sa:.3f}, n={len(A)})   B {mb:.3f} (+-{sb:.3f}, n={len(B)})   change {mb - ma:+.3f}")
    rows = []
    for mp in A[0]["maps"]:
        for n in A[0]["maps"][mp]["metrics"]:
            va = [r["maps"][mp]["metrics"][n] for r in A if mp in r["maps"] and n in r["maps"][mp]["metrics"]]
            vb = [r["maps"][mp]["metrics"][n] for r in B if mp in r["maps"] and n in r["maps"][mp]["metrics"]]
            if not va or not vb:
                continue
            sa_, sb_ = st.mean(m["score"] for m in va), st.mean(m["score"] for m in vb)
            spread = st.stdev([m["score"] for m in va]) if len(va) > 1 else 0.0
            rows.append((sb_ - sa_, spread, mp, n, st.mean(m["bots"] for m in va), st.mean(m["bots"] for m in vb), va[0]["real"]))
    for name in sorted({r[3] for r in rows}):
        if focus and focus not in name:
            continue
        sel = [r for r in rows if r[3] == name]
        print(f"  {name:28s} score change {st.mean(r[0] for r in sel):+.2f} (maps: " + ", ".join(f"{r[2][3:]} {r[4]:.2f}->{r[5]:.2f} real {r[6]:.2f}" for r in sel) + ")")
    guards = {}
    for label, rs in (("A", A), ("B", B)):
        for k in ("stuck", "kills/min", "flags taken", "panics"):
            vals = [r["maps"][mp]["guards"].get(k) or 0 for r in rs for mp in r["maps"]]
            guards.setdefault(k, {})[label] = sum(vals) / len(rs)
    print("guards (sum over maps, mean per run): " + "; ".join(f"{k} {v['A']:.1f} -> {v['B']:.1f}" for k, v in guards.items()))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--label", default="")
    r.add_argument("--secs", type=int, default=180)
    r.add_argument("--only", default="")
    r.add_argument("--exe", default=EXE, help="the game build to play (default: the latest release build)")
    r.add_argument("--lab", default="", help="bot lab experiments to switch on (COD4RW_BOTLAB)")
    s = sub.add_parser("score")
    s.add_argument("dir")
    c = sub.add_parser("compare")
    c.add_argument("a")
    c.add_argument("b")
    g = sub.add_parser("group", help="average two sets of runs: group A1,A2 B1,B2 [--focus NAME]")
    g.add_argument("a")
    g.add_argument("b")
    g.add_argument("--focus", default=None)
    a = ap.parse_args()
    if a.cmd == "run":
        run(a.label, a.secs, [m for m in a.only.split(",") if m], a.exe, a.lab)
    elif a.cmd == "score":
        print(f"total {score_run(a.dir)['total']:.3f}")
    elif a.cmd == "group":
        group(a.a.split(","), a.b.split(","), a.focus)
    else:
        compare(a.a, a.b)


if __name__ == "__main__":
    main()
