"""Blind test: can you tell the bots from real players? Builds one HTML page
of anonymous clips, half real public-match players (CoD4X demos) and half
bots from a sim on the same map, each a top-down replay of one player:
where they went, where they looked (the cone), when they fired (flash),
crouched or went prone (smaller dot), and enemies in their sight (red).
Guess Human or Bot for each; the page scores you at the end and shows which
real player or persona each clip was.

    python tools/blind_test.py --map mp_crossfire --bots <sim dir> [--clips 12] [--secs 25] [--out page.html]

Real tracks come from data/demo_traits/raw (tools/demo_traits.py), bot
tracks from a sim recorded with COD4RW_RECORD=<dir>/bots.csv
COD4RW_RECORD_WHO=all.
"""

import argparse
import csv
import glob
import json
import os
import random
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import demo_traits as dt  # noqa: E402

WORK = os.path.join(os.path.dirname(__file__), "..", "data", "demo_traits")


def clips_from(tracks, n, secs, rng, label):
    """Up to `n` stretches of `secs` alive, with some action, one per player."""
    out = []
    names = list(tracks)
    rng.shuffle(names)
    for name in names:
        t = tracks[name]
        for a, b in rng.sample(dt.lives(t), len(dt.lives(t))):
            if t["t"][b - 1] - t["t"][a] < secs:
                continue
            start = rng.randint(a, max(a, b - int(secs * 20)))
            end = start + int(secs * 20)
            if end > b or np.any(np.diff(t["t"][start:end]) > 0.3):
                continue
            moved = np.hypot(np.diff(t["x"][start:end]), np.diff(t["y"][start:end])).sum()
            if moved < 300:
                continue
            seg = slice(start, end)
            out.append({
                "label": label, "who": name,
                "x": [round(v) for v in t["x"][seg]], "y": [round(v) for v in t["y"][seg]],
                "yaw": [round(v) for v in t["yaw"][seg]],
                "fire": [int(v > 0) for v in (t["shots"][seg] + t["fire"][seg])],
                "stance": [int(v) for v in t["stance"][seg]],
                "sprint": [int(v) for v in t["sprint"][seg]],
                "ms": [int(v) for v in t["ms"][seg]],
                "cn": int(t["cn"][start]),
                "vis": [sorted(v) for v in t["vis"][seg]],
            })
            break
        if len(out) >= n:
            break
    return out


def enemies_for(clips, tracks):
    """Where each clip's visible enemies were, frame by frame."""
    where = {}
    for t in tracks.values():
        for i in range(len(t["t"])):
            where[(int(t["ms"][i]), int(t["cn"][i]))] = (round(t["x"][i]), round(t["y"][i]))
    for c in clips:
        c["enemies"] = [[where[(ms, e)] for e in vis if (ms, e) in where] for ms, vis in zip(c["ms"], c["vis"])]
        del c["vis"], c["ms"]


PAGE = """<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Bot or Not</title>
<style>
:root { --bg:#111317; --panel:#1b1e24; --text:#e8e8ea; --dim:#9aa0aa; --accent:#e0b04a; --human:#5fb3ff; --bot:#ff7a59; }
body { background:var(--bg); color:var(--text); font:15px/1.4 system-ui,sans-serif; margin:0; padding:16px; }
h1 { font-size:20px; margin:0 0 4px; } p { color:var(--dim); margin:4px 0 12px; max-width:760px; }
.grid { display:grid; grid-template-columns:repeat(auto-fill,minmax(300px,1fr)); gap:14px; }
.card { background:var(--panel); border-radius:8px; padding:10px; }
canvas { width:100%; aspect-ratio:1; background:#0b0c0f; border-radius:6px; display:block; }
.row { display:flex; gap:8px; margin-top:8px; align-items:center; }
button { flex:1; padding:8px; border:1px solid #333; border-radius:6px; background:#23272f; color:var(--text); cursor:pointer; font-weight:600; }
button.sel.h { background:var(--human); color:#000; } button.sel.b { background:var(--bot); color:#000; }
.reveal { font-size:13px; color:var(--dim); min-height:18px; margin-top:6px; }
.ok { color:#7ddc7d; } .bad { color:#ff6b6b; }
#score { position:sticky; top:0; background:var(--bg); padding:8px 0; font-weight:600; z-index:1; }
#done { margin:16px 0; padding:10px 18px; background:var(--accent); color:#000; border:0; flex:none; }
</style></head><body>
<h1>Bot or Not: __MAP__</h1>
<p>Each clip replays one player from above for __SECS__ seconds: the dot is them (smaller when crouched or prone, ringed while
sprinting), the cone is where they looked, a flash is a shot, red dots are enemies they could see. Half are real players from
public CoD4X matches, half are bots. Pick Human or Bot for each, then Score.</p>
<div id="score">0 / __N__ answered</div>
<div class="grid" id="grid"></div>
<button id="done">Score</button>
<script>
const CLIPS = __CLIPS__;
const OCC = __OCC__;
const CELL = __CELL__;
const answers = {};
const grid = document.getElementById('grid');
let bounds = {x0:Infinity,x1:-Infinity,y0:Infinity,y1:-Infinity};
for (const [x,y] of OCC) { bounds.x0=Math.min(bounds.x0,x*CELL); bounds.x1=Math.max(bounds.x1,(x+1)*CELL); bounds.y0=Math.min(bounds.y0,y*CELL); bounds.y1=Math.max(bounds.y1,(y+1)*CELL); }
const cards = CLIPS.map((c, i) => {
  const card = document.createElement('div'); card.className = 'card';
  card.innerHTML = `<canvas width="600" height="600"></canvas><div class="row"><b>#${i+1}</b>
    <button class="h">Human</button><button class="b">Bot</button></div><div class="reveal"></div>`;
  grid.appendChild(card);
  const [h, b] = card.querySelectorAll('button');
  const pick = v => { answers[i] = v; h.classList.toggle('sel', v==='human'); b.classList.toggle('sel', v==='bot');
    document.getElementById('score').textContent = `${Object.keys(answers).length} / ${CLIPS.length} answered`; };
  h.onclick = () => pick('human'); b.onclick = () => pick('bot');
  return { c, canvas: card.querySelector('canvas'), reveal: card.querySelector('.reveal') };
});
function view(c) {
  // Zoom on the clip's own area (with room round it).
  let x0=Math.min(...c.x), x1=Math.max(...c.x), y0=Math.min(...c.y), y1=Math.max(...c.y);
  const size = Math.max(x1-x0, y1-y0, 1500) * 1.25, cx=(x0+x1)/2, cy=(y0+y1)/2;
  return { cx, cy, s: 600/size };
}
const views = CLIPS.map(view);
let frame = 0;
function draw() {
  cards.forEach(({c, canvas}, i) => {
    const g = canvas.getContext('2d'), v = views[i];
    const P = (x, y) => [300 + (x - v.cx) * v.s, 300 - (y - v.cy) * v.s];
    g.fillStyle = '#0b0c0f'; g.fillRect(0, 0, 600, 600);
    g.fillStyle = '#2a2e36';
    for (const [x,y] of OCC) { const [a,b] = P(x*CELL, (y+1)*CELL); g.fillRect(a, b, CELL*v.s+0.5, CELL*v.s+0.5); }
    const k = frame % c.x.length;
    g.strokeStyle = 'rgba(224,176,74,0.5)'; g.lineWidth = 2; g.beginPath();
    for (let j = Math.max(0, k-200); j <= k; j++) { const [a,b] = P(c.x[j], c.y[j]); j===Math.max(0,k-200) ? g.moveTo(a,b) : g.lineTo(a,b); }
    g.stroke();
    const [px, py] = P(c.x[k], c.y[k]);
    const yaw = (c.yaw[k] + 90) * Math.PI / 180;
    g.fillStyle = 'rgba(255,255,255,0.13)'; g.beginPath(); g.moveTo(px, py);
    g.arc(px, py, 160, -yaw - 0.5, -yaw + 0.5); g.closePath(); g.fill();
    for (const [ex, ey] of c.enemies[k] || []) { const [a,b] = P(ex, ey); g.fillStyle = '#ff5050'; g.beginPath(); g.arc(a, b, 6, 0, 7); g.fill(); }
    if (c.fire[k]) { g.fillStyle = '#fff6b0'; g.beginPath(); g.arc(px + Math.cos(-yaw)*16, py + Math.sin(-yaw)*16, 6, 0, 7); g.fill(); }
    g.fillStyle = '#e0b04a'; g.beginPath(); g.arc(px, py, c.stance[k] ? 5 : 8, 0, 7); g.fill();
    if (c.sprint[k]) { g.strokeStyle = '#e0b04a'; g.lineWidth = 2; g.beginPath(); g.arc(px, py, 12, 0, 7); g.stroke(); }
  });
  frame++;
}
setInterval(draw, 50);
document.getElementById('done').onclick = () => {
  let right = 0;
  cards.forEach(({c, reveal}, i) => {
    const truth = c.label, ok = answers[i] === truth; right += ok;
    reveal.innerHTML = `<span class="${ok ? 'ok' : 'bad'}">${truth === 'bot' ? 'Bot' : 'Human'}</span> &middot; ${c.who}`;
  });
  document.getElementById('score').textContent = `You got ${right} / ${CLIPS.length} (chance is ${CLIPS.length/2}). Bots fooled you ${cards.filter(({c},i)=>c.label==='bot'&&answers[i]==='human').length} times.`;
};
</script></body></html>"""


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--map", required=True)
    ap.add_argument("--bots", required=True, help="sim dir with bots-*.csv")
    ap.add_argument("--clips", type=int, default=12)
    ap.add_argument("--secs", type=float, default=25.0)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("--out", default=None)
    a = ap.parse_args()
    rng = random.Random(a.seed)
    real_tracks = {}
    for d in sorted(glob.glob(os.path.join(WORK, "raw", "*"))):
        meta_path = os.path.join(d, "meta.txt")
        if not os.path.exists(meta_path):
            continue
        meta = dict(line.rstrip("\n").split("=", 1) for line in open(meta_path) if "=" in line)
        if meta.get("map") != a.map:
            continue
        _, players, _, tracks = dt.load_demo(d)
        bots = {p["name"] for p in players if p.get("bot") == "1"} | {dt.RECORDER}
        demo = os.path.basename(d)
        part = {f"{demo} {n}": t for n, t in tracks.items() if n not in bots}
        real_tracks.update(part)
    if not real_tracks:
        sys.exit(f"no real tracks on {a.map} in {WORK}/raw")
    bot_tracks, _, _ = dt.load_bot_run(sorted(glob.glob(os.path.join(a.bots, "*.csv"))))
    if not bot_tracks:
        sys.exit(f"no bot recordings in {a.bots}")
    half = a.clips // 2
    real = clips_from(real_tracks, half, a.secs, rng, "human")
    bot = clips_from(bot_tracks, a.clips - len(real), a.secs, rng, "bot")
    # Enemy positions from each side's own tracks (keyed per demo/run).
    by_demo = {}
    for name, t in real_tracks.items():
        by_demo.setdefault(name.split(" ")[0], {})[name] = t
    for c in real:
        enemies_for([c], by_demo[c["who"].split(" ")[0]])
        c["who"] = f"real player ({c['who'].split(' ')[0]})"
    enemies_for(bot, bot_tracks)
    sheet = {}
    for f in glob.glob(os.path.join(a.bots, "*-personas.csv")):
        for r in csv.DictReader(open(f, encoding="utf-8")):
            sheet["".join(ch if ch.isalnum() else "_" for ch in r["name"])] = r
    for c in bot:
        p = sheet.get(c["who"], {})
        c["who"] = f"bot {c['who']}, persona {p.get('source', '?')} ({p.get('style', '?')}, level {int(p.get('rank') or 0) + 1})"
    clips = real + bot
    rng.shuffle(clips)
    occ_path = os.path.join(WORK, "maps", f"{a.map}.json")
    occ, cell = [], 64
    if os.path.exists(occ_path):
        m = json.load(open(occ_path))
        occ, cell = [[o[0], o[1]] for o in m["occupancy"]], m["cell"]
    page = (PAGE.replace("__MAP__", a.map).replace("__SECS__", f"{a.secs:.0f}").replace("__N__", str(len(clips)))
            .replace("__CLIPS__", json.dumps(clips, separators=(",", ":"))).replace("__OCC__", json.dumps(occ, separators=(",", ":")))
            .replace("__CELL__", str(cell)))
    out = a.out or os.path.join(WORK, f"blind_{a.map}.html")
    open(out, "w", encoding="utf-8").write(page)
    print(f"{len(real)} real + {len(bot)} bot clips -> {out}")


if __name__ == "__main__":
    main()
