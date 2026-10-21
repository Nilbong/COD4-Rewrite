"""Frame-time benchmark: every multiplayer map, 5v5 bots, the user's own settings.

    python tools/bench.py [--exe PATH] [--maps mp_crash,mp_bog] [--secs 30] [--res 1280x720]
                          [--bots 5] [--tag NAME] [--borderless] [--wait-idle] [--env KEY=VALUE ...]

Runs a copy of the game (never target/release/cod4rw.exe itself) silently on
each map with COD4RW_PERF, reading the user's settings from a copy of
%LOCALAPPDATA%/cod4rw/stats.txt (COD4RW_STATSFILE: read only, nothing saved),
and writes target/bench/<tag>.md:

    map | median ms | 1% low fps | p99 ms | GPU ms | main Mcyc | render Mcyc | all Mcyc

The first 10 s of each match (loading, settling) are left out by perf.rs.

Windowed runs (the default, safe while someone uses the PC) are composed by
Windows: their frames latch to the display's refresh (11.8 ms = two
refreshes at 170 Hz), so their medians are NOT the user's fps (borderless).
Measured 2026-10-14: Crossfire ~22 ms windowed vs ~10 borderless, Carentan
~21 vs ~9, GPU ~7 ms in both. Use --borderless (borderless full screen, on
top) for real frame times, only while nobody's at the PC.
Mcycles (per frame, busy cycles of the main and render threads, and all
threads) and GPU ms (the timed render passes) hold up when other programs
load the machine; frame times don't, so compare those in a quiet window.
"""

import argparse
import os
import re
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MAPS = [
    "mp_convoy", "mp_backlot", "mp_bloc", "mp_bog", "mp_broadcast", "mp_carentan", "mp_countdown",
    "mp_crash", "mp_creek", "mp_crossfire", "mp_citystreets", "mp_farm", "mp_killhouse", "mp_overgrown",
    "mp_pipeline", "mp_shipment", "mp_showdown", "mp_strike", "mp_vacant", "mp_cargoship", "mp_crash_snow",
]
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def summarise(log: str) -> dict:
    text = ANSI.sub("", log)
    out = {}
    run = re.search(r"perf: run ([\d.]+) ms avg.*?median ([\d.]+), p95 ([\d.]+), p99 ([\d.]+)", text)
    if run:
        out["avg"], out["median"], out["p95"], out["p99"] = (float(g) for g in run.groups())
    # Per-window figures; the first window still has the match settling.
    threads = re.findall(r"thread Mcycles per frame: main ([\d.]+), render ([\d.]+)", text)[1:]
    if threads:
        out["main"] = statistics.median(float(a) for a, _ in threads)
        out["render"] = statistics.median(float(b) for _, b in threads)
    every = re.findall(r"perf: cpu .*?([\d.]+) Mcycles all threads", text)[1:]
    if every:
        out["all"] = statistics.median(float(v) for v in every)
    gpu = re.findall(r"perf: gpu ([\d.]+) ms in timed passes", text)[1:]
    if gpu:
        out["gpu"] = statistics.median(float(v) for v in gpu)
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=str(ROOT / "target" / "release" / "cod4rw.exe"))
    ap.add_argument("--maps", default=",".join(MAPS))
    ap.add_argument("--secs", type=float, default=30.0, help="measured seconds per map (after 10 s settling)")
    # (A small window by default, so the PC stays usable; the user's own
    # 2560x1440 needs --res 2560x1440, and borderless, only while they're away.)
    ap.add_argument("--res", default="1280x720")
    ap.add_argument("--bots", type=int, default=5, help="bots per team")
    ap.add_argument("--tag", default=time.strftime("%Y%m%d_%H%M"))
    ap.add_argument("--stats", default=os.path.join(os.environ.get("LOCALAPPDATA", ""), "cod4rw", "stats.txt"))
    ap.add_argument("--borderless", action="store_true",
                    help="borderless full screen, on top, as players run it. Covers the screen: refused unless COD4RW_USER_AWAY=1 (only when the user has said they're away). "
                         "A window behind others is composed and its frames latch to the display's refresh (11.8 ms at 170 Hz).")
    ap.add_argument("--wait-idle", action="store_true",
                    help="before each map, wait while another game copy runs (other sessions' tests): one at a time")
    ap.add_argument("--env", action="append", default=[], help="extra KEY=VALUE for the game (e.g. COD4RW_PERF knobs)")
    args = ap.parse_args()

    out_dir = ROOT / "target" / "bench"
    logs = out_dir / args.tag
    logs.mkdir(parents=True, exist_ok=True)
    exe = out_dir / f"cod4rw_bench_{args.tag}.exe"
    shutil.copy2(args.exe, exe)
    stats = out_dir / "stats.txt"
    if os.path.isfile(args.stats):
        shutil.copy2(args.stats, stats)

    env = dict(os.environ)
    env.update({
        "COD4RW_SIM": str(10 + args.secs + 2),
        "COD4RW_RES": args.res,
        "COD4RW_PERF": "novsync",
        "COD4RW_PRESENT": "immediate",
        "RUST_LOG": "info",
    })
    if stats.is_file():
        env["COD4RW_STATSFILE"] = str(stats)
    if args.borderless:
        # It takes over the whole screen: only while nobody's at the PC
        # (the user couldn't reach their screen, 2026-10-14).
        if os.environ.get("COD4RW_USER_AWAY") != "1":
            print("refusing --borderless: it covers the screen. Set COD4RW_USER_AWAY=1 only when the user has said they're away.")
            return 2
        env["COD4RW_BORDERLESS"] = "1"
        env["COD4RW_ONTOP"] = "1"
    for kv in args.env:
        k, _, v = kv.partition("=")
        if k == "COD4RW_PERF":
            env[k] = env[k] + "," + v
        else:
            env[k] = v

    rows = []
    def others_running() -> bool:
        out = subprocess.run(["tasklist", "/fo", "csv", "/nh"], capture_output=True, text=True).stdout.lower()
        names = [l.split('","')[0].strip('"') for l in out.splitlines() if l]
        mine = exe.name.lower()
        return any(n != mine and (n.startswith(("bench_", "load_", "cod4rw")) or n in ("s.exe", "host.exe", "join.exe")) for n in names)

    for m in [m.strip() for m in args.maps.split(",") if m.strip()]:
        while args.wait_idle and others_running():
            time.sleep(10)
        log_path = logs / f"{m}.log"
        with open(log_path, "w", encoding="utf8", errors="replace") as f:
            try:
                subprocess.run([str(exe), "--map", m, "--bots", str(args.bots)], env=env, stdout=f, stderr=subprocess.STDOUT,
                               cwd=str(ROOT), timeout=args.secs + 180)
            except subprocess.TimeoutExpired:
                print(f"{m}: timed out (machine busy?), left out", flush=True)
        s = summarise(log_path.read_text(encoding="utf8", errors="replace"))
        rows.append((m, s))
        low = 1000.0 / s["p99"] if s.get("p99") else float("nan")
        print(f"{m:16} median {s.get('median', float('nan')):6.2f} ms  1% low {low:5.0f} fps  "
              f"gpu {s.get('gpu', float('nan')):5.2f}  main {s.get('main', float('nan')):5.1f}  "
              f"render {s.get('render', float('nan')):5.1f} Mcyc", flush=True)

    def f(v, fmt):
        return format(v, fmt) if v is not None else "-"

    mode = "borderless (as played)" if args.borderless else "composed (windowed): frame times latch to the display refresh, NOT the player's fps"
    lines = [
        f"# Benchmark {args.tag}",
        "",
        f"{args.res} {mode}, {args.bots}v{args.bots} bots, {args.secs:.0f} s per map, settings from {args.stats}, extra env {args.env or '-'}",
        "",
        "| map | median ms | 1% low fps | p99 ms | GPU ms | main Mcyc | render Mcyc | all Mcyc |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for m, s in rows:
        low = 1000.0 / s["p99"] if s.get("p99") else None
        lines.append(f"| {m} | {f(s.get('median'), '.2f')} | {f(low, '.0f')} | {f(s.get('p99'), '.2f')} | {f(s.get('gpu'), '.2f')} | "
                     f"{f(s.get('main'), '.1f')} | {f(s.get('render'), '.1f')} | {f(s.get('all'), '.0f')} |")
    med = [s["median"] for _, s in rows if "median" in s]
    if med:
        lines += ["", f"Worst median {max(med):.2f} ms ({1000 / max(med):.0f} fps); maps at 120+ fps: "
                      f"{sum(1 for v in med if v <= 1000 / 120)} of {len(med)}"]
    (out_dir / f"{args.tag}.md").write_text("\n".join(lines) + "\n", encoding="utf8")
    print(f"\nwrote {out_dir / (args.tag + '.md')}")
    exe.unlink(missing_ok=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
