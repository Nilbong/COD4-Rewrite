"""Map load time: from the game's first log line to the match being shown.

    python tools/loadtime.py [--exe PATH] [--maps mp_crash,mp_backlot] [--runs 2] [--tag NAME]

Each map is loaded `--runs` times (the first can be a cold disk cache) in
a silent sim (COD4RW_SIM, no bots, 1280x720, a copy of the user's
settings), and timed from the first log line to `settle: match shown`
(the load screen gone). The main load steps' own times are pulled from the
log too. Writes target/loadtime/<tag>.md.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ANSI = re.compile(r"\x1b\[[0-9;]*m")
STAMP = re.compile(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z")
# (label, pattern whose first group is seconds)
STEPS = [
    ("iwds", r"mounted iwds in ([\d.]+)(ms|s)"),
    ("map parse", r"parsed mp_\w+: .*?\(([\d.]+)(ms|s)\)"),
    ("common_mp", r"parsed common_mp: .*?\(([\d.]+)(ms|s)\)"),
    ("render data", r"built render data in ([\d.]+)(ms|s)"),
    ("waw tables", r"waw: tables and weapon files read in ([\d.]+)(s)"),
    ("bo1 tables", r"bo1: tables and weapon files read in ([\d.]+)(s)"),
    ("mw2 guns", r"mw2 guns: tables and weapons read in ([\d.]+)(s)"),
    ("ui assets", r"ui: \d+ menus.* in ([\d.]+)(ms|s)"),
    ("settle", r"settle: match shown after ([\d.]+)( s)"),
]


def stamp(line: str):
    m = STAMP.match(line)
    return datetime.fromisoformat(m.group(1)[:26]) if m else None


def measure(log: str) -> dict:
    lines = [ANSI.sub("", l) for l in log.splitlines()]
    first = next((stamp(l) for l in lines if stamp(l)), None)
    shown = next((stamp(l) for l in lines if "settle: match shown" in l), None)
    out = {"total": (shown - first).total_seconds() if first and shown else None}
    text = "\n".join(lines)
    for label, pat in STEPS:
        m = re.search(pat, text)
        if m:
            v = float(m.group(1))
            out[label] = v / 1000 if m.group(2) == "ms" else v
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=str(ROOT / "target" / "release" / "cod4rw.exe"))
    ap.add_argument("--maps", default="mp_crash,mp_backlot,mp_cargoship,mp_farm,mp_highrise")
    ap.add_argument("--runs", type=int, default=2)
    ap.add_argument("--tag", default=time.strftime("%Y%m%d_%H%M"))
    ap.add_argument("--env", action="append", default=[], help="extra KEY=VALUE for the game")
    ap.add_argument("--stats", default=os.path.join(os.environ.get("LOCALAPPDATA", ""), "cod4rw", "stats.txt"))
    args = ap.parse_args()
    out_dir = ROOT / "target" / "loadtime"
    logs = out_dir / args.tag
    logs.mkdir(parents=True, exist_ok=True)
    exe = out_dir / f"load_{args.tag}.exe"
    shutil.copy2(args.exe, exe)
    stats = out_dir / "stats.txt"
    if os.path.isfile(args.stats):
        shutil.copy2(args.stats, stats)
    env = dict(os.environ)
    env.update({"COD4RW_SIM": "3", "COD4RW_RES": "1280x720", "RUST_LOG": "info"})
    if stats.is_file():
        env["COD4RW_STATSFILE"] = str(stats)
    for kv in args.env:
        k, _, v = kv.partition("=")
        env[k] = v
    rows = []
    for m in [m.strip() for m in args.maps.split(",") if m.strip()]:
        for r in range(1, args.runs + 1):
            log_path = logs / f"{m}_{r}.log"
            with open(log_path, "w", encoding="utf8", errors="replace") as f:
                try:
                    subprocess.run([str(exe), "--map", m, "--bots", "0"], env=env, stdout=f, stderr=subprocess.STDOUT, cwd=str(ROOT), timeout=240)
                except subprocess.TimeoutExpired:
                    print(f"{m} run {r}: timed out", flush=True)
            s = measure(log_path.read_text(encoding="utf8", errors="replace"))
            rows.append((m, r, s))
            print(f"{m} run {r}: total {s.get('total')} s  " + "  ".join(f"{k} {v:.2f}" for k, v in s.items() if k != "total" and v is not None), flush=True)
    labels = [l for l, _ in STEPS]
    lines = [f"# Load times {args.tag}", "", "Seconds from the first log line to the match shown; step times as each logs them (steps overlap: several load in parallel).", "",
             "| map | run | total | " + " | ".join(labels) + " |", "|---|---|---|" + "---|" * len(labels)]
    for m, r, s in rows:
        f = lambda k: f"{s[k]:.1f}" if s.get(k) is not None else "-"
        lines.append(f"| {m} | {r} | {f('total')} | " + " | ".join(f(l) for l in labels) + " |")
    (out_dir / f"{args.tag}.md").write_text("\n".join(lines) + "\n", encoding="utf8")
    exe.unlink(missing_ok=True)
    print(f"wrote {out_dir / (args.tag + '.md')}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
