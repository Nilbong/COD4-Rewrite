"""How real players play, from CoD4X demos: one row of traits per player per
match, with per-map and per-mode summaries and map knowledge (where people
hold, fight, die, throw from), for the bots' personas.

    python tools/demo_traits.py [--demos DIR] [--work data/demo_traits]

New demos in DIR are run through the extractor first
(`cargo build --release -p iw3 --example demotraits`; it tests line of sight
against the map's collision), so re-running after more recordings arrive
picks them up. Writes into the work dir:

- players.csv: every trait for every player-match (2+ minutes alive)
- summary.md: distributions overall, per mode and per map; how traits go
  together; skill groups by rank
- maps/<map>.json: occupancy, hold spots and the directions held, kill and
  death spots, grenade spots, prefire spots (for tactical knowledge)

Demos are 20 Hz snapshots of what the recording client was sent: players far
from the recorder may be missing, and angles are quantised. Good for
behaviour, too coarse for fine aim.
"""

import argparse
import collections
import csv
import glob
import json
import math
import os
import subprocess
import sys

import numpy as np

DEMOS = r"G:\SteamLibrary\steamapps\common\Call of Duty 4\main\demos"
EXE = os.path.join(os.path.dirname(__file__), "..", "target", "release", "examples", "demotraits.exe")
RECORDER = "Shane"

MOVING = 40.0  # units/s
FIGHT_WINDOW = 0.5  # s either side of a shot or ADS
HOLD_MIN = 2.0  # s still, out of fights, to count as holding
CAMP_MIN = 8.0  # s still for a hold to count as camping
ANGLE_TOL = 12.0  # deg: the view staying on one angle
CELL = 256.0  # roaming cell, units
OCC_CELL = 64.0  # map occupancy cell, units
PREFIRE_AHEAD = 1.0  # s: an enemy appearing this soon after a blind shot
PREFIRE_CONE = 20.0  # deg: ... where the shot was aimed
NEW_CONTACT_GAP = 1.0  # s without sight before it's a new contact
SNIPERS = ("barrett", "m40a3", "m21", "dragunov", "remington700")
SMGS = ("mp5", "p90", "uzi", "ak74u", "skorpion")
RIFLES = ("m16", "ak47", "m4", "g36c", "g3", "m14", "mp44")
LMGS = ("rpd", "saw", "m60e4")
SHOTGUNS = ("winchester1200", "m1014")
PISTOLS = ("beretta", "colt45", "usp", "deserteagle", "deserteaglegold")
ATTACHMENTS = ("silencer", "reflex", "acog", "gl", "grip")
# specialty bits (bg_perks_mp.cpp order) and CoD4's names for them.
PERKS = [
    ("gpsjammer", "UAV Jammer"), ("bulletaccuracy", "Steady Aim"), ("fastreload", "Sleight of Hand"),
    ("rof", "Double Tap"), ("holdbreath", "Iron Lungs"), ("bulletpenetration", "Deep Impact"),
    ("grenadepulldeath", "Martyrdom"), ("pistoldeath", "Last Stand"), ("quieter", "Dead Silence"),
    ("parabolic", "Eavesdrop"), ("longersprint", "Extreme Conditioning"), ("detectexplosive", "Bomb Squad"),
    ("explosivedamage", "Sonic Boom"), ("exposeenemy", "?"), ("bulletdamage", "Stopping Power"),
    ("extraammo", "Bandolier"), ("twoprimaries", "Overkill"), ("armorvest", "Juggernaut"),
    ("fraggrenade", "Frag x3"), ("specialgrenade", "Special x3"),
]


def gun_class(w):
    base = w.replace("_mp", "")
    if base.startswith("gl_"):
        base = base[3:] + "_gl"
    head = base.split("_")[0]
    for kind, names in (("sniper", SNIPERS), ("smg", SMGS), ("rifle", RIFLES), ("lmg", LMGS), ("shotgun", SHOTGUNS), ("pistol", PISTOLS)):
        if head in names:
            return kind, head, "_".join(base.split("_")[1:])
    return None, head, ""


def wrap(a):
    return (a + 180.0) % 360.0 - 180.0


# ------------------------------------------------------------------ extract


def extract(demos, work):
    raw = os.path.join(work, "raw")
    os.makedirs(raw, exist_ok=True)
    todo = []
    for f in sorted(glob.glob(os.path.join(demos, "*.dm_1"))):
        stem = os.path.splitext(os.path.basename(f))[0]
        meta = os.path.join(raw, stem, "meta.txt")
        if not os.path.exists(meta) or os.path.getmtime(meta) < os.path.getmtime(f):
            todo.append(f)
    if todo and not os.path.exists(EXE):
        sys.exit(f"{len(todo)} new demos but no extractor: cargo build --release -p iw3 --example demotraits")
    for f in todo:
        print("extracting", os.path.basename(f), flush=True)
        r = subprocess.run([EXE, f, raw], capture_output=True, text=True)
        if r.returncode != 0:
            print("  failed:", r.stderr.strip().splitlines()[-1:] if r.stderr else r.returncode)
    return sorted(d for d in glob.glob(os.path.join(raw, "*")) if os.path.exists(os.path.join(d, "meta.txt")))


# --------------------------------------------------------------------- load


def load_demo(d):
    meta = dict(line.rstrip("\n").split("=", 1) for line in open(os.path.join(d, "meta.txt")) if "=" in line)
    players = list(csv.DictReader(open(os.path.join(d, "players.csv"), newline="", encoding="utf-8")))
    events = list(csv.DictReader(open(os.path.join(d, "events.csv"), newline="", encoding="utf-8")))
    tracks = {}
    for f in glob.glob(os.path.join(d, "tracks", "*.csv")):
        with open(f, newline="") as fh:
            r = csv.reader(fh)
            head = next(r)
            rows = list(r)
        if len(rows) < 100:
            continue
        col = {k: i for i, k in enumerate(head)}
        num = lambda k: np.array([float(x[col[k]]) for x in rows])
        t = {k: num(k) for k in ("t", "cn", "yaw", "pitch", "fire", "shots", "ads", "x", "y", "z", "stance", "team", "sprint", "jump")}
        t["weapon"] = [x[col["weapon"]] for x in rows]
        t["vis"] = [frozenset(int(v) for v in x[col["vis"]].split(";")) if x[col["vis"]] else frozenset() for x in rows]
        t["ms"] = np.round(t["t"] * 1000).astype(np.int64)
        tracks[os.path.splitext(os.path.basename(f))[0]] = t
    return meta, players, events, tracks


def flags_for(map_name):
    """Domination flag origins from data/demo_tracks/flags.txt."""
    path = os.path.join(os.path.dirname(__file__), "..", "data", "demo_tracks", "flags.txt")
    out = []
    if os.path.exists(path):
        for line in open(path):
            m, _, rest = line.partition("|")
            if m != map_name or "origin=" not in rest:
                continue
            o = rest.split("origin=")[1].split()[:3]
            out.append([float(v) for v in o])
    return out


def load_bot_run(paths, log=None):
    """A sim's bot recordings (`COD4RW_RECORD=<dir>/bots.csv
    COD4RW_RECORD_WHO=all`) as demo-style tracks: alive samples at 20 Hz on
    the shared clock, line of sight from the `vis` column, kills matched to
    deaths. `log`: the sim's log, for grenades thrown."""
    raw = {}
    for f in paths:
        name = os.path.splitext(os.path.basename(f))[0].split("-", 1)[-1]
        if name == "personas":
            continue
        with open(f, newline="") as fh:
            # (The last row may be cut short if the sim was stopped.)
            rows = list(csv.DictReader(fh))[:-1]
        if len(rows) < 100 or "vis" not in rows[0]:
            continue
        raw[name] = rows
    if not raw:
        return {}, [], {}
    t0 = max(float(r[0]["t"]) for r in raw.values())
    t1 = min(float(r[-1]["t"]) for r in raw.values())
    ticks = np.arange(t0, t1, 0.05)
    tracks, deaths = {}, []
    kills = []
    for name, rows in raw.items():
        t = np.array([float(r["t"]) for r in rows])
        def num(v):
            try:
                return float(v or 0)
            except ValueError:
                return 0.0

        col = lambda k: np.array([num(r[k]) for r in rows])
        dead = col("dead")
        idx = np.searchsorted(t, ticks, side="right") - 1
        prev = np.concatenate([[-1], idx[:-1]])
        cs = lambda a: np.concatenate([[0], np.cumsum(a)])
        shots = cs(col("shots"))
        fire = cs(col("fire"))
        jump = cs(col("jump"))
        ok = dead[idx] < 0.5
        out = {
            "t": ticks[ok], "cn": col("id")[idx][ok], "yaw": col("yaw")[idx][ok], "pitch": col("pitch")[idx][ok],
            "fire": ((fire[idx + 1] - fire[prev + 1]) > 0).astype(float)[ok], "shots": (shots[idx + 1] - shots[prev + 1])[ok],
            "ads": col("ads")[idx][ok], "x": col("x")[idx][ok], "y": col("y")[idx][ok], "z": col("z")[idx][ok],
            "stance": col("stance")[idx][ok], "team": col("team")[idx][ok], "sprint": col("sprint")[idx][ok],
            "jump": ((jump[idx + 1] - jump[prev + 1]) > 0).astype(float)[ok],
        }
        out["weapon"] = [rows[i]["weapon"] for i in idx[ok]]
        out["vis"] = [frozenset(int(v) for v in rows[i]["vis"].split(";")) if rows[i]["vis"] else frozenset() for i in idx[ok]]
        out["ms"] = np.round(out["t"] * 1000).astype(np.int64)
        out["motive"] = [rows[i]["motive"] for i in idx[ok]]
        tracks[name] = out
        for i in np.flatnonzero((dead[1:] > 0.5) & (dead[:-1] < 0.5)) + 1:
            deaths.append((t[i], name, float(rows[i - 1]["x"]), float(rows[i - 1]["y"]), float(rows[i - 1]["z"])))
        for i in np.flatnonzero(col("kills") > 0):
            kills.append((t[i], name, rows[i]["heads"] != "0"))
    events = []
    for kt, killer, head in kills:
        near = [d for d in deaths if abs(d[0] - kt) < 0.15 and d[1] != killer]
        if near:
            d = min(near, key=lambda d: abs(d[0] - kt))
            events.append({"t": f"{kt - t0:.3f}", "kind": "kill", "a": "", "a_name": killer, "b": "", "b_name": d[1], "weapon": "",
                           "extra": "8" if head else "", "x": d[2], "y": d[3], "z": d[4]})
    for d in deaths:
        if not any(e["b_name"] == d[1] and abs(float(e["t"]) + t0 - d[0]) < 0.2 for e in events):
            events.append({"t": f"{d[0] - t0:.3f}", "kind": "kill", "a": "", "a_name": "", "b": "", "b_name": d[1], "weapon": "", "extra": "", "x": d[2], "y": d[3], "z": d[4]})
    if log and os.path.exists(log):
        kinds = {"frag": "frag_grenade_mp", "flash": "flash_grenade_mp", "stun": "concussion_grenade_mp", "smoke": "smoke_grenade_mp"}
        for line in open(log, encoding="utf-8", errors="replace"):
            if "grenade: " in line and " throws a " in line:
                who = line.split("grenade: ", 1)[1].split(" throws a ")[0].strip()
                what = line.split(" throws a ", 1)[1].split()[0].lower()
                name = who.replace(" ", "_")
                name = "".join(c if c.isalnum() else "_" for c in who)
                events.append({"t": "0", "kind": "throw", "a": "", "a_name": name, "b": "", "b_name": "", "weapon": kinds.get(what, what), "extra": "", "x": "", "y": "", "z": ""})
    for t in tracks.values():
        t["t"] = t["t"] - t0
        t["ms"] = np.round(t["t"] * 1000).astype(np.int64)
    return tracks, events, {}


def bot_rows(run, map_name, mode, min_alive=0.6):
    """Trait rows for every bot of a sim run dir (bots*.csv, sim log), or
    of each run in its subdirectories (several short sims pooled)."""
    subs = sorted(d for d in glob.glob(os.path.join(run, "*")) if os.path.isdir(d))
    if subs:
        return [r for d in subs for r in bot_rows(d, map_name, mode, min_alive)]
    paths = sorted(glob.glob(os.path.join(run, "*.csv")))
    logs = sorted(glob.glob(os.path.join(run, "*.log")))
    tracks, events, _ = load_bot_run(paths, logs[0] if logs else None)
    where = collections.defaultdict(dict)
    for name, t in tracks.items():
        for i in range(len(t["t"])):
            where[int(t["ms"][i])][int(t["cn"][i])] = (t["x"][i], t["y"][i], t["z"][i], int(t["team"][i]))
    sheet = {}
    for f in glob.glob(os.path.join(run, "*-personas.csv")):
        for r in csv.DictReader(open(f, encoding="utf-8")):
            sheet["".join(c if c.isalnum() else "_" for c in r["name"])] = r
    flags = flags_for(map_name) if mode == "dom" else []
    rows = []
    for name, t in tracks.items():
        if len(t["t"]) < 20 * 60 * min_alive:
            continue
        # The recorder's names keep only letters and digits.
        mine = [dict(e, a_name=name) if e["a_name"] in (name, name.replace("_", " ")) else e for e in events]
        tr, _ = player_traits(name, t, ({"map": map_name}, mine, where, flags))
        motives = collections.Counter(t["motive"])
        p = sheet.get(name, {})
        rows.append({"player": name, "map": map_name, "mode": mode, "source": p.get("source", ""), "style": p.get("style", ""),
                     "rank": int(p.get("rank") or 0) + 1, "motives": "+".join(f"{k}:{v / len(t['t']):.2f}" for k, v in motives.most_common(4)), **tr})
    return rows


def compare(real, bots, keys=None, title=""):
    """Real players' p10/p50/p90 against the bots' p50, trait by trait."""
    keys = keys or [k for k in NUMERIC if k not in ("rank",)]
    dr, db = describe(real, keys), describe(bots, keys)
    s = f"### {title}: {len(real)} real player-matches, {len(bots)} bots\n\n"
    out = []
    for k in keys:
        n, lo, mid, hi = dr[k]
        b = db[k][2]
        flag = "" if math.isnan(b) or math.isnan(lo) or lo - 1e-9 <= b <= hi + 1e-9 else " **outside**"
        out.append([k, lo, mid, hi, b, flag])
    return s + table(["trait", "real p10", "real p50", "real p90", "bots p50", ""], out)


def fidelity(bots, real_by_source, keys):
    """Does each bot play like its own real player? Spearman across bots of
    the persona's trait and the bot's measured one."""
    out = []
    for k in keys:
        pairs = [(float(real_by_source[b["source"]][k]), b[k]) for b in bots
                 if b.get("source") in real_by_source and real_by_source[b["source"]].get(k) not in ("", None)
                 and not math.isnan(b[k])]
        if len(pairs) < 6:
            out.append([k, len(pairs), math.nan])
            continue
        x, y = np.array(pairs).T
        rx, ry = np.argsort(np.argsort(x)), np.argsort(np.argsort(y))
        out.append([k, len(pairs), float(np.corrcoef(rx, ry)[0, 1])])
    return table(["trait", "bots", "r (persona vs bot)"], out)


# ----------------------------------------------------------------- measures


def lives(t):
    """Index ranges of continuous alive stretches (a gap or teleport ends one)."""
    gap = np.diff(t["t"]) > 0.3
    jump = np.hypot(np.diff(t["x"]), np.diff(t["y"])) > 120.0
    cut = np.flatnonzero(gap | jump) + 1
    starts = np.concatenate([[0], cut])
    ends = np.concatenate([cut, [len(t["t"])]])
    return [(a, b) for a, b in zip(starts, ends) if b - a >= 10]


def runs_of(mask):
    """(start, end) of each run of True."""
    m = np.concatenate([[False], mask, [False]]).astype(int)
    d = np.diff(m)
    return list(zip(np.flatnonzero(d == 1), np.flatnonzero(d == -1)))


def view_dir(yaw, pitch):
    # Recorder yaw: CoD yaw - 90.
    y, p = math.radians(yaw + 90.0), math.radians(pitch)
    return np.array([math.cos(p) * math.cos(y), math.cos(p) * math.sin(y), math.sin(p)])


def angle_between(v, w):
    n = np.linalg.norm(v) * np.linalg.norm(w)
    if n < 1e-6:
        return 180.0
    return math.degrees(math.acos(max(-1.0, min(1.0, float(np.dot(v, w)) / n))))


def player_traits(name, t, demo_ctx):
    """Every trait for one player's track in one demo, plus map knowledge."""
    meta, events, where, flags = demo_ctx
    out = {}
    n = len(t["t"])
    dt = np.diff(t["t"], append=t["t"][-1] + 0.05).clip(0, 0.1)
    alive = float(dt.sum())
    out["alive_min"] = alive / 60.0
    cn = int(np.bincount(t["cn"].astype(int)).argmax())
    out["cn"] = cn
    team = int(np.bincount(t["team"].astype(int)).argmax())
    out["team"] = team
    spans = lives(t)
    # Speeds within lives.
    vx = np.zeros(n)
    vy = np.zeros(n)
    for a, b in spans:
        tt = t["t"][a:b]
        if b - a > 2:
            vx[a:b] = np.gradient(t["x"][a:b], tt)
            vy[a:b] = np.gradient(t["y"][a:b], tt)
    speed = np.hypot(vx, vy)
    moving = speed > MOVING
    busy = (t["shots"] > 0) | (t["fire"] > 0) | (t["ads"] > 0)
    # Fight: within FIGHT_WINDOW of shooting/ADS.
    k = int(FIGHT_WINDOW * 20)
    fight = np.convolve(busy.astype(float), np.ones(2 * k + 1), "same") > 0
    contact = np.array([len(v) > 0 for v in t["vis"]])
    calm = ~fight
    w = lambda m: float((dt * m).sum())
    out["fight_share"] = w(fight) / alive
    out["contact_share"] = w(contact) / alive
    mv = calm & moving
    out["speed_p50"] = float(np.median(speed[mv])) if mv.any() else math.nan
    out["sprint_share"] = w(mv & (t["sprint"] > 0)) / max(w(mv), 1e-6)
    out["still_share"] = w(calm & ~moving) / max(w(calm), 1e-6)
    out["stand_share"] = w(t["stance"] == 0) / alive
    out["crouch_share"] = w(t["stance"] == 1) / alive
    out["prone_share"] = w(t["stance"] == 2) / alive
    out["jumps_pm"] = float(t["jump"].sum()) / out["alive_min"]
    # Holds: still out of fights for HOLD_MIN+.
    holds = []
    for a, b in runs_of(calm & ~moving):
        dur = t["t"][b - 1] - t["t"][a]
        if dur >= HOLD_MIN and not np.any(np.diff(t["t"][a:b]) > 0.3):
            holds.append((a, b, dur))
    out["holds_pm"] = len(holds) / out["alive_min"]
    hd = [h[2] for h in holds]
    out["hold_p50"] = float(np.median(hd)) if hd else math.nan
    out["hold_p90"] = float(np.percentile(hd, 90)) if hd else math.nan
    out["camp_share"] = sum(d for d in hd if d >= CAMP_MIN) / alive
    # Angles within holds: how long the view stays on one direction.
    angle_holds = []
    hold_spots = []
    for a, b, dur in holds:
        yaw = t["yaw"][a:b]
        s = 0
        for i in range(1, len(yaw) + 1):
            if i == len(yaw) or abs(wrap(yaw[i] - np.median(yaw[s:i]))) > ANGLE_TOL:
                d = t["t"][a + i - 1] - t["t"][a + s]
                if d >= 0.5:
                    angle_holds.append(d)
                    hold_spots.append((float(t["x"][a + s]), float(t["y"][a + s]), float(t["z"][a + s]), float(np.median(yaw[s:i])), float(d)))
                s = i
    out["angle_hold_p50"] = float(np.median(angle_holds)) if angle_holds else math.nan
    out["angles_per_hold"] = len(angle_holds) / max(len(holds), 1)
    # Roaming: distinct cells per minute; spread of where they play.
    cells = set(zip((t["x"] // CELL).astype(int), (t["y"] // CELL).astype(int)))
    out["cells_pm"] = len(cells) / out["alive_min"]
    occ = collections.Counter(zip((t["x"] // CELL).astype(int), (t["y"] // CELL).astype(int)))
    top = sum(c for _, c in occ.most_common(3))
    out["top3_cell_share"] = top / n
    out["gyration"] = float(np.sqrt(np.var(t["x"]) + np.var(t["y"])))
    # Aggression: closing on vs backing from the nearest enemy (in range).
    toward = away = 0.0
    near_d = []
    for i in range(0, n - 1):
        enemies = where.get(int(t["ms"][i]), {})
        best = None
        for ecn, (ex, ey, ez, eteam) in enemies.items():
            if ecn == cn or (eteam == team and team != 0) or eteam == 3:
                continue
            d = math.hypot(ex - t["x"][i], ey - t["y"][i])
            if best is None or d < best[0]:
                best = (d, ex, ey)
        if best is None or best[0] > 3000.0:
            continue
        near_d.append(best[0])
        if not moving[i]:
            continue
        ux, uy = (best[1] - t["x"][i]) / max(best[0], 1.0), (best[2] - t["y"][i]) / max(best[0], 1.0)
        closing = vx[i] * ux + vy[i] * uy
        if closing > 50.0:
            toward += dt[i]
        elif closing < -50.0:
            away += dt[i]
    out["aggression"] = toward / max(toward + away, 1e-6) if toward + away > 5.0 else math.nan
    out["enemy_dist_p50"] = float(np.median(near_d)) if near_d else math.nan
    # Contacts: an enemy newly in sight. Crosshair placement (view error to
    # them then), first shot reaction, prefire (shot before sight, at them).
    seen_at = {}
    new_contacts = []
    for i in range(n):
        for e in t["vis"][i]:
            last = seen_at.get(e)
            if last is None or t["t"][i] - last > NEW_CONTACT_GAP:
                new_contacts.append((i, e))
            seen_at[e] = t["t"][i]
    place_err, reacts, prefires, contacts_close = [], [], 0, 0
    prefire_spots = []
    for i, e in new_contacts:
        pos = where.get(int(t["ms"][i]), {}).get(e)
        if pos is None:
            continue
        eye = np.array([t["x"][i], t["y"][i], t["z"][i] + 60.0 - 20.0 * t["stance"][i]])
        to = np.array(pos[:3]) + np.array([0, 0, 48.0]) - eye
        dist = float(np.linalg.norm(to))
        if dist > 2500.0:
            continue
        contacts_close += 1
        err = angle_between(view_dir(t["yaw"][i], t["pitch"][i]), to)
        place_err.append(err)
        # First shot within 2 s while they're still in sight.
        for j in range(i, min(n, i + 40)):
            if e not in t["vis"][j]:
                break
            if t["shots"][j] > 0:
                reacts.append(t["t"][j] - t["t"][i])
                break
        # A shot in the second before, with nobody in sight, aimed there.
        for j in range(max(0, i - int(PREFIRE_AHEAD * 20)), i):
            if t["shots"][j] > 0 and not t["vis"][j]:
                if angle_between(view_dir(t["yaw"][j], t["pitch"][j]), to) < PREFIRE_CONE:
                    prefires += 1
                    prefire_spots.append((float(t["x"][j]), float(t["y"][j]), float(t["z"][j]), float(t["yaw"][j])))
                    break
    out["contacts_pm"] = contacts_close / out["alive_min"]
    out["placement_err_p50"] = float(np.median(place_err)) if len(place_err) >= 5 else math.nan
    out["first_shot_p50"] = float(np.median(reacts)) if len(reacts) >= 5 else math.nan
    out["prefire_rate"] = prefires / contacts_close if contacts_close >= 10 else math.nan
    shots = t["shots"] > 0
    out["shots_pm"] = float(t["shots"].sum()) / out["alive_min"]
    blind = shots & ~contact
    out["blind_shot_share"] = float(blind.sum()) / max(float(shots.sum()), 1.0)
    out["ads_fight_share"] = w(fight & (t["ads"] > 0)) / max(w(fight), 1e-6)
    out["move_fire_share"] = float((shots & moving).sum()) / max(float(shots.sum()), 1.0)
    # Snipers: how long they were scoped in before each shot.
    scoped = []
    ads_since = None
    for i in range(n):
        if t["ads"][i] > 0:
            if ads_since is None:
                ads_since = t["t"][i]
        else:
            ads_since = None
        if t["shots"][i] > 0 and gun_class(t["weapon"][i])[0] == "sniper":
            scoped.append(-1.0 if ads_since is None else t["t"][i] - ads_since)
    sc = np.array(scoped)
    out["sniper_shots"] = len(scoped)
    out["noscope_share"] = float((sc < 0).mean()) if len(sc) >= 5 else math.nan
    out["quickscope_share"] = float(((sc >= 0) & (sc < 0.5)).mean()) if len(sc) >= 5 else math.nan
    out["scope_time_p50"] = float(np.median(sc[sc >= 0])) if (sc >= 0).sum() >= 5 else math.nan
    # Kit: the gun held most (primaries), the pistol, attachments.
    held = collections.Counter()
    pistol = collections.Counter()
    for wpn, d in zip(t["weapon"], dt):
        kind, head, att = gun_class(wpn)
        if kind == "pistol":
            pistol[wpn.replace("_mp", "")] += d
        elif kind:
            held[(kind, head, att)] += d
    total_held = sum(held.values())
    if held:
        (kind, head, att), d = held.most_common(1)[0]
        out["gun_class"], out["gun"], out["attachment"] = kind, head, att or "none"
        out["main_gun_share"] = d / max(total_held, 1e-6)
        out["guns_used"] = sum(1 for v in held.values() if v > 20.0)
    else:
        out["gun_class"], out["gun"], out["attachment"], out["main_gun_share"], out["guns_used"] = "none", "none", "none", math.nan, 0
    out["pistol"] = pistol.most_common(1)[0][0] if pistol else "none"
    out["pistol_share"] = sum(pistol.values()) / alive
    # Events: utility, kills, streaks, objective.
    mine = [e for e in events if e["a_name"] == name]
    throws = collections.Counter(e["weapon"].replace("_mp", "") for e in mine if e["kind"] in ("throw", "equip"))
    for k in ("frag_grenade", "smoke_grenade", "flash_grenade", "concussion_grenade", "claymore", "c4"):
        out[k + "_pm"] = throws.get(k, 0) / out["alive_min"]
    out["frag_pm"] = out.pop("frag_grenade_pm")
    out["smoke_pm"] = out.pop("smoke_grenade_pm")
    out["flash_pm"] = out.pop("flash_grenade_pm")
    out["stun_pm"] = out.pop("concussion_grenade_pm")
    out["tube_pm"] = sum(v for k, v in throws.items() if k.startswith("gl") or k.startswith("rpg")) / out["alive_min"]
    out["utility_pm"] = out["frag_pm"] + out["smoke_pm"] + out["flash_pm"] + out["stun_pm"] + out["claymore_pm"] + out["c4_pm"]
    out["cooks_pm"] = sum(1 for e in mine if e["kind"] == "cook") / out["alive_min"]
    out["reloads_pm"] = sum(1 for e in mine if e["kind"].startswith("reload")) / out["alive_min"]
    out["empty_reload_share"] = sum(1 for e in mine if e["kind"] == "reload_empty") / max(sum(1 for e in mine if e["kind"].startswith("reload")), 1)
    out["pickups_pm"] = sum(1 for e in mine if e["kind"] == "pickup") / out["alive_min"]
    streaks = collections.Counter(e["extra"] for e in events if e["kind"] == "streak" and e["a_name"] == name)
    out["streaks_called"] = sum(streaks.values())
    out["streak_kinds"] = "+".join(f"{k}x{v}" for k, v in streaks.most_common())
    kills = [e for e in events if e["kind"] == "kill" and e["a_name"] == name and e["b_name"] != name]
    deaths = [e for e in events if e["kind"] == "kill" and e["b_name"] == name]
    out["kills"], out["deaths"] = len(kills), len(deaths)
    out["kd"] = len(kills) / max(len(deaths), 1)
    out["kills_pm"] = len(kills) / out["alive_min"]
    out["headshot_share"] = sum(1 for e in kills if e["extra"] == "8") / max(len(kills), 1)
    out["knife_share"] = sum(1 for e in kills if e["extra"] == "7") / max(len(kills), 1)
    # Kill distance: my position at the kill, their death spot.
    kd = []
    for e in kills:
        if not e["x"]:
            continue
        i = int(np.searchsorted(t["t"], float(e["t"])))
        if 0 < i < n and abs(t["t"][min(i, n - 1)] - float(e["t"])) < 0.3:
            kd.append(math.hypot(float(e["x"]) - t["x"][i - 1], float(e["y"]) - t["y"][i - 1]))
    out["kill_dist_p50"] = float(np.median(kd)) if len(kd) >= 3 else math.nan
    # Best streak and temperament from the order of kills and deaths.
    seq = sorted([(float(e["t"]), "k", e["b_name"]) for e in kills] + [(float(e["t"]), "d", e["a_name"]) for e in deaths])
    best = cur = 0
    for _, kind, _ in seq:
        cur = cur + 1 if kind == "k" else 0
        best = max(best, cur)
    out["best_streak"] = best
    # Revenge: the next kill after a death is on the killer (vs chance).
    nxt = rev = 0
    for i, (_, kind, who) in enumerate(seq):
        if kind != "d" or not who or who == name:
            continue
        later = [s for s in seq[i + 1:] if s[1] == "k"]
        if later and later[0][0] - seq[i][0] < 60.0:
            nxt += 1
            rev += later[0][2] == who
    out["revenge_share"] = rev / nxt if nxt >= 3 else math.nan
    # Life-level: aggression / sprint right after deaths in a row (tilt),
    # and stillness while on a streak (protecting it).
    life_rows = []
    deaths_t = [s[0] for s in seq if s[1] == "d"]
    kills_t = [s[0] for s in seq if s[1] == "k"]
    for a, b in spans:
        t0, t1 = t["t"][a], t["t"][b - 1]
        prev_deaths = 0
        for s in reversed([s for s in seq if s[0] < t0]):
            if s[1] == "k":
                break
            prev_deaths += 1
        m = slice(a, b)
        c, mvl, dl = calm[m], moving[m], dt[m]
        wl = lambda mask: float((dl * mask).sum())
        life_rows.append((prev_deaths, wl(c & mvl & (t["sprint"][m] > 0)) / max(wl(c & mvl), 1e-6), wl(c & ~mvl) / max(wl(c), 1e-6)))
    tilt = [r for r in life_rows if r[0] >= 2]
    calm_l = [r for r in life_rows if r[0] == 0]
    out["tilt_sprint_delta"] = (np.mean([r[1] for r in tilt]) - np.mean([r[1] for r in calm_l])) if len(tilt) >= 2 and len(calm_l) >= 2 else math.nan
    # On a streak: still share at 3+ kills in the current life vs 0.
    on, off = [], []
    for i in range(n):
        if not calm[i]:
            continue
        kills_life = 0
        # Kills since this life started.
        life = next(((a, b) for a, b in spans if a <= i < b), None)
        if life is None:
            continue
        start = t["t"][life[0]]
        kills_life = sum(1 for kt in kills_t if start <= kt <= t["t"][i])
        (on if kills_life >= 3 else off).append(not moving[i])
    out["streak_still_delta"] = (np.mean(on) - np.mean(off)) if len(on) >= 200 and len(off) >= 200 else math.nan
    # Objective: captures, time near a flag.
    caps = [e for e in events if e["kind"] == "capture" and e["a_name"] == name]
    out["captures_pm"] = len(caps) / out["alive_min"]
    if flags:
        f = np.array(flags)
        dmin = np.min(np.hypot(t["x"][:, None] - f[None, :, 0], t["y"][:, None] - f[None, :, 1]), axis=1)
        out["on_flag_share"] = w(dmin < 200.0) / alive
        out["near_flag_share"] = w(dmin < 600.0) / alive
    else:
        out["on_flag_share"] = out["near_flag_share"] = math.nan
    know = {
        "holds": hold_spots,
        "prefires": prefire_spots,
        "occ": [(int(x), int(y), c) for (x, y), c in collections.Counter(zip((t["x"] // OCC_CELL).astype(int), (t["y"] // OCC_CELL).astype(int))).items()],
    }
    return out, know


# ------------------------------------------------------------------ summary


SKILL_GROUPS = [("new (rank < 15)", 0, 15), ("mid (15-54)", 15, 55), ("high (55+)", 55, 10_000)]


# A typical public lobby's gun mix, which the pooled priors are weighted
# towards (some recorded lobbies were mostly snipers). An assumption, not
# measured: CoD4 public TDM/Dom lobbies are mostly assault rifles and SMGs.
TARGET_MIX = {"rifle": 0.42, "smg": 0.28, "lmg": 0.10, "sniper": 0.14, "shotgun": 0.06}
# No match counts for more than this many players (big 20v20 lobbies
# otherwise outweigh several normal ones).
MATCH_CAP = 14
SNIPER_HEAVY = 0.35


def weigh(rows):
    """Each row's weight for the pooled priors: matches capped at
    MATCH_CAP players' worth, then gun classes eased towards TARGET_MIX
    (by at most 4x either way). Per-class figures stay unweighted between
    classes, so sniper players remain a full sniper source."""
    per = collections.Counter(r["demo"] for r in rows)
    for r in rows:
        r["match_weight"] = min(1.0, MATCH_CAP / per[r["demo"]])
        r["weight"] = r["match_weight"]
    for _ in range(4):
        total = sum(r["weight"] for r in rows if r["gun_class"] in TARGET_MIX)
        share = {c: sum(r["weight"] for r in rows if r["gun_class"] == c) / max(total, 1e-9) for c in TARGET_MIX}
        for r in rows:
            c = r["gun_class"]
            if c in TARGET_MIX and share[c] > 0:
                r["weight"] *= TARGET_MIX[c] / share[c]
    for r in rows:
        r["weight"] = min(max(r["weight"], 0.25 * r["match_weight"]), 4.0 * r["match_weight"])


def wpercentile(v, w, qs):
    order = np.argsort(v)
    v, w = v[order], w[order]
    c = np.cumsum(w) - 0.5 * w
    c /= w.sum()
    return [float(np.interp(q / 100.0, c, v)) for q in qs]


def describe(rows, keys, weighted=True):
    out = {}
    for k in keys:
        ok = [r for r in rows if isinstance(r.get(k), (int, float)) and not math.isnan(r[k])]
        v = np.array([r[k] for r in ok], dtype=float)
        w = np.array([r.get("weight", 1.0) if weighted else r.get("match_weight", 1.0) for r in ok], dtype=float)
        out[k] = (len(v), *(wpercentile(v, w, [10, 50, 90]) if len(v) and w.sum() > 0 else (math.nan,) * 3))
    return out


def match_table(rows):
    """Per match: players, gun mix, sniper share (sniper-heavy flagged)."""
    out = []
    for demo in sorted(set(r["demo"] for r in rows)):
        m = [r for r in rows if r["demo"] == demo]
        guns = [r for r in m if r["gun_class"] in TARGET_MIX]
        mix = collections.Counter(r["gun_class"] for r in guns)
        sn = mix["sniper"] / max(len(guns), 1)
        out.append([demo, m[0]["map"], m[0]["mode"], len(m), sn, ", ".join(f"{k} {v}" for k, v in mix.most_common()),
                    f"{m[0]['match_weight'] * len(m):.1f}", "**sniper-heavy**" if sn >= SNIPER_HEAVY else ""])
    return table(["demo", "map", "mode", "players", "sniper share", "gun mix", "weight", ""], out)


def fmt(v):
    if isinstance(v, float):
        if math.isnan(v):
            return "-"
        return f"{v:.2f}" if abs(v) < 100 else f"{v:.0f}"
    return str(v)


def table(head, rows):
    s = "| " + " | ".join(head) + " |\n|" + "---|" * len(head) + "\n"
    for r in rows:
        s += "| " + " | ".join(fmt(x) for x in r) + " |\n"
    return s


NUMERIC = [
    "rank", "kd", "kills_pm", "sprint_share", "speed_p50", "still_share", "crouch_share", "prone_share", "holds_pm",
    "hold_p50", "camp_share", "angle_hold_p50", "cells_pm", "aggression", "enemy_dist_p50", "fight_share",
    "placement_err_p50", "first_shot_p50", "prefire_rate", "blind_shot_share", "ads_fight_share", "move_fire_share",
    "quickscope_share", "noscope_share", "scope_time_p50", "kill_dist_p50", "headshot_share", "utility_pm", "frag_pm",
    "smoke_pm", "flash_pm", "stun_pm", "claymore_pm", "c4_pm", "tube_pm", "streaks_called", "best_streak", "revenge_share",
    "tilt_sprint_delta", "streak_still_delta", "captures_pm", "on_flag_share", "near_flag_share", "jumps_pm",
]


def summary(rows, path):
    s = "# Real player traits from CoD4X demos\n\n"
    s += f"{len(rows)} player-matches from {len(set(r['demo'] for r in rows))} demos, {sum(r['alive_min'] for r in rows):.0f} alive minutes.\n\n"
    s += "Pooled figures are weighted so no single lobby type dominates: each match counts for at most "
    s += f"{MATCH_CAP} players, and gun classes are eased towards a typical public lobby's mix ({', '.join(f'{k} {v:.0%}' for k, v in TARGET_MIX.items())}). "
    s += "The per-gun-class table is unweighted between classes (the sniper players are the sniper personas' source).\n\n"
    s += "## Matches\n\n" + match_table(rows) + "\n"
    s += "## All players (p10 / p50 / p90)\n\n"
    d = describe(rows, NUMERIC)
    s += table(["trait", "n", "p10", "p50", "p90"], [[k, *d[k]] for k in NUMERIC])
    for title, key in (("mode", "mode"), ("map", "map")):
        groups = sorted(set(r[key] for r in rows))
        s += f"\n## Per {title} (p50)\n\n"
        ds = {g: describe([r for r in rows if r[key] == g], NUMERIC) for g in groups}
        s += table(["trait"] + [f"{g} ({sum(1 for r in rows if r[key] == g)})" for g in groups], [[k] + [ds[g][k][2] for g in groups] for k in NUMERIC])
    s += "\n## By experience (rank) (p50)\n\n"
    ds = {g: describe([r for r in rows if lo <= r["rank"] < hi], NUMERIC) for g, lo, hi in SKILL_GROUPS}
    s += table(["trait"] + [f"{g} ({sum(1 for r in rows if lo <= r['rank'] < hi)})" for g, lo, hi in SKILL_GROUPS], [[k] + [ds[g][k][2] for g, _, _ in SKILL_GROUPS] for k in NUMERIC])
    s += "\n## By gun class (p50)\n\n"
    classes = [c for c in ("smg", "rifle", "lmg", "shotgun", "sniper") if any(r["gun_class"] == c for r in rows)]
    ds = {c: describe([r for r in rows if r["gun_class"] == c], NUMERIC, weighted=False) for c in classes}
    s += table(["trait"] + [f"{c} ({sum(1 for r in rows if r['gun_class'] == c)})" for c in classes], [[k] + [ds[c][k][2] for c in classes] for k in NUMERIC])
    s += "\n## Guns, attachments, perks\n\n"
    for key in ("gun_class", "gun", "attachment", "pistol"):
        c = collections.Counter(r[key] for r in rows)
        cw = collections.Counter()
        for r in rows:
            cw[r[key]] += r["weight"]
        tw = sum(cw.values())
        s += f"- {key} (players, weighted share): " + ", ".join(f"{k} {v} ({cw[k] / tw:.0%})" for k, v in c.most_common(14)) + "\n"
    pc = collections.Counter(p for r in rows for p in r["perks"].split("+") if p)
    s += "- perks: " + ", ".join(f"{k} {v}" for k, v in pc.most_common()) + "\n"
    sc = collections.Counter()
    for r in rows:
        for part in filter(None, r["streak_kinds"].split("+")):
            k, _, v = part.rpartition("x")
            sc[k] += int(v)
    s += "- streaks called: " + ", ".join(f"{k} {v}" for k, v in sc.most_common()) + "\n"
    s += "\n## How traits go together (Spearman, |r| >= 0.3, n >= 30)\n\n"
    pairs = []
    keys = [k for k in NUMERIC if k not in ("frag_pm", "smoke_pm", "flash_pm", "stun_pm", "claymore_pm", "c4_pm")]
    for i, a in enumerate(keys):
        for b in keys[i + 1:]:
            v = [(r[a], r[b], r["weight"]) for r in rows if all(isinstance(r[x], (int, float)) and not math.isnan(r[x]) for x in (a, b))]
            if len(v) < 30:
                continue
            x, y = np.array([p[:2] for p in v]).T
            wt = np.array([p[2] for p in v])
            rx, ry = np.argsort(np.argsort(x)).astype(float), np.argsort(np.argsort(y)).astype(float)
            # Weighted (lobby-balanced) correlation of the ranks.
            mx, my = np.average(rx, weights=wt), np.average(ry, weights=wt)
            cov = np.average((rx - mx) * (ry - my), weights=wt)
            c = float(cov / math.sqrt(np.average((rx - mx) ** 2, weights=wt) * np.average((ry - my) ** 2, weights=wt)))
            if abs(c) >= 0.3:
                pairs.append((a, b, c, len(v)))
    pairs.sort(key=lambda p: -abs(p[2]))
    s += table(["a", "b", "r", "n"], pairs[:40])
    open(path, "w", encoding="utf-8").write(s)


PERSONA_COLUMNS = [
    "rank", "prestige", "skill", "kd", "kills_pm", "gun_class", "gun", "attachment", "pistol", "perks",
    "sprint_share", "speed_p50", "still_share", "crouch_share", "prone_share", "holds_pm", "hold_p50", "camp_share",
    "angle_hold_p50", "cells_pm", "aggression", "enemy_dist_p50", "prefire_rate", "placement_err_p50", "first_shot_p50",
    "ads_fight_share", "move_fire_share", "quickscope_share", "noscope_share", "scope_time_p50", "kill_dist_p50",
    "headshot_share", "frag_pm", "smoke_pm", "flash_pm", "stun_pm", "claymore_pm", "c4_pm", "tube_pm", "utility_pm",
    "streaks_called", "best_streak", "revenge_share", "tilt_sprint_delta", "streak_still_delta", "captures_pm",
    "near_flag_share", "jumps_pm", "alive_min", "weight",
]


def export_personas(rows, path):
    """The bots' persona table (`crates/game/src/bots/personas.csv`): one
    row per real player-match, no names, with a skill index (0..1: where
    their K/D and kills a minute sit among everyone's)."""
    def pct(key):
        v = np.array([r[key] for r in rows], dtype=float)
        order = np.argsort(np.argsort(v))
        return order / max(len(v) - 1, 1)
    skill = 0.5 * pct("kd") + 0.5 * pct("kills_pm")
    skill = np.argsort(np.argsort(skill)) / max(len(rows) - 1, 1)
    by_name = {name: f"specialty_{key}" for key, name in PERKS}
    with open(path, "w", newline="", encoding="utf-8") as f:
        f.write("# Real CoD4X public-match players, one per row (tools/demo_traits.py --export). Do not edit.\n")
        wr = csv.writer(f)
        wr.writerow(["source", "map", "mode"] + PERSONA_COLUMNS)
        for r, sk in zip(rows, skill):
            out = [f"{r['demo']}#{r['cn']}", r["map"], r["mode"]]
            for k in PERSONA_COLUMNS:
                v = sk if k == "skill" else r[k]
                if k == "perks":
                    v = "+".join(by_name[p] for p in v.split("+") if p in by_name)
                out.append(f"{v:.4g}" if isinstance(v, float) and not math.isnan(v) else ("" if isinstance(v, float) else v))
            wr.writerow(out)


def render_map(k, path, size=720):
    """A picture of a map's knowledge: where people are (grey), die (blue),
    hold and which way (green), prefire from (red), throw from (yellow)."""
    try:
        from PIL import Image, ImageDraw
    except ImportError:
        return
    occ = k["occupancy"]
    if not occ:
        return
    xs = [o[0] for o in occ]
    ys = [o[1] for o in occ]
    x0, x1, y0, y1 = min(xs) - 1, max(xs) + 2, min(ys) - 1, max(ys) + 2
    scale = size / max(x1 - x0, y1 - y0)
    cell = k["cell"]
    img = Image.new("RGB", (int((x1 - x0) * scale), int((y1 - y0) * scale)), (16, 16, 20))
    d = ImageDraw.Draw(img)
    top = math.log1p(max(o[2] for o in occ))
    px = lambda x, y: ((x / cell - x0) * scale, (y1 - y / cell) * scale)
    for x, y, c in occ:
        g = int(30 + 170 * math.log1p(c) / top)
        d.rectangle([(x - x0) * scale, (y1 - y - 1) * scale, (x - x0 + 1) * scale - 1, (y1 - y) * scale - 1], fill=(g, g, g))
    for x, y, _ in k["deaths"]:
        X, Y = px(x, y)
        d.point((X, Y), fill=(70, 120, 255))
    for x, y, z, yaw, dur in k["holds"]:
        X, Y = px(x, y)
        a = math.radians(yaw + 90.0)
        L = min(4 + dur * 2, 18)
        d.line([X, Y, X + math.cos(a) * L, Y - math.sin(a) * L], fill=(60, 220, 90))
    for x, y, z, yaw in k["prefires"]:
        X, Y = px(x, y)
        d.ellipse([X - 2, Y - 2, X + 2, Y + 2], fill=(255, 60, 60))
    for _, x, y, z in k["throws"]:
        X, Y = px(x, y)
        d.point((X, Y), fill=(255, 220, 40))
    d.text((6, 4), f"{k['map']}: {k['matches']} matches, {len(k['holds'])} held angles, {len(k['prefires'])} prefires", fill=(255, 255, 255))
    img.save(path)


# --------------------------------------------------------------------- main


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--demos", default=DEMOS)
    ap.add_argument("--work", default=os.path.join(os.path.dirname(__file__), "..", "data", "demo_traits"))
    ap.add_argument("--min-alive", type=float, default=1.25, help="minutes alive for a player-match to count")
    ap.add_argument("--export", nargs="?", const=os.path.join(os.path.dirname(__file__), "..", "crates", "game", "src", "bots", "personas.csv"),
                    help="also write the bots' persona table (default crates/game/src/bots/personas.csv)")
    a = ap.parse_args()
    dirs = extract(a.demos, a.work)
    rows = []
    knowledge = collections.defaultdict(lambda: {"holds": [], "prefires": [], "occ": collections.Counter(), "kills": [], "deaths": [], "throws": [], "matches": 0})
    for d in dirs:
        meta, players, events, tracks = load_demo(d)
        demo = os.path.basename(d)
        map_name, mode = meta.get("map", "?"), meta.get("gametype", "?")
        # Where everyone is at each snapshot (for distances, directions).
        where = collections.defaultdict(dict)
        for name, t in tracks.items():
            for i in range(len(t["t"])):
                where[int(t["ms"][i])][int(t["cn"][i])] = (t["x"][i], t["y"][i], t["z"][i], int(t["team"][i]))
        flags = flags_for(map_name) if mode in ("dom",) else []
        info = {p["name"]: p for p in players}
        k = knowledge[map_name]
        k["matches"] += 1
        for e in events:
            if e["kind"] == "kill" and e["x"]:
                k["deaths"].append((float(e["x"]), float(e["y"]), float(e["z"])))
            if e["kind"] in ("throw", "equip") and e["x"]:
                k["throws"].append((e["weapon"].replace("_mp", ""), float(e["x"]), float(e["y"]), float(e["z"])))
        for name, t in tracks.items():
            p = info.get(name)
            if p is None or p.get("bot") == "1":
                continue
            # The recorder only counts while actually playing.
            if name == RECORDER and int(np.bincount(t["team"].astype(int)).argmax()) == 3:
                continue
            dur = float(np.diff(t["t"]).clip(0, 0.1).sum()) / 60.0
            if dur < a.min_alive:
                continue
            tr, kn = player_traits(name, t, (meta, events, where, flags))
            perks = int(p["perks"] or 0)
            row = {
                "demo": demo, "map": map_name, "mode": mode, "player": name,
                "rank": int(p["rank"] or 0) + 1, "prestige": int(p["prestige"] or 0),
                "perks": "+".join(PERKS[b][1] for b in range(len(PERKS)) if perks >> b & 1),
                "score": p["score"], **tr,
            }
            rows.append(row)
            k["holds"] += kn["holds"]
            k["prefires"] += kn["prefires"]
            for x, y, c in kn["occ"]:
                k["occ"][(x, y)] += c
        print(f"{demo}: {map_name} {mode}, {sum(1 for r in rows if r['demo'] == demo)} player-matches", flush=True)
    if not rows:
        sys.exit("no players")
    weigh(rows)
    keys = list(rows[0].keys())
    for r in rows:
        for kk in r:
            if kk not in keys:
                keys.append(kk)
    with open(os.path.join(a.work, "players.csv"), "w", newline="", encoding="utf-8") as f:
        wr = csv.DictWriter(f, fieldnames=keys)
        wr.writeheader()
        for r in rows:
            wr.writerow({kk: (f"{v:.4g}" if isinstance(v, float) else v) for kk, v in r.items()})
    summary(rows, os.path.join(a.work, "summary.md"))
    if a.export:
        export_personas(rows, a.export)
        print("personas ->", a.export)
    os.makedirs(os.path.join(a.work, "maps"), exist_ok=True)
    for m, k in knowledge.items():
        out = {
            "map": m, "matches": k["matches"], "cell": OCC_CELL,
            "occupancy": [[x, y, c] for (x, y), c in k["occ"].most_common()],
            "holds": [[round(v, 1) for v in h] for h in k["holds"]],
            "prefires": [[round(v, 1) for v in h] for h in k["prefires"]],
            "deaths": [[round(v) for v in h] for h in k["deaths"]],
            "throws": [[h[0]] + [round(v) for v in h[1:]] for h in k["throws"]],
        }
        json.dump(out, open(os.path.join(a.work, "maps", f"{m}.json"), "w"))
        render_map(out, os.path.join(a.work, "maps", f"{m}.png"))
    print(f"{len(rows)} player-matches -> {os.path.join(a.work, 'players.csv')}, summary.md, maps/")


if __name__ == "__main__":
    main()
