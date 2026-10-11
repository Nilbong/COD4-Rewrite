"""Contact sheet: the take's video frames beside in-game shots at matching
times (COD4RW_MOCAPSHOT names shots <part>_<ms into it>.png).

compare.py <config.json> <shots dir> <out.jpg> [per part] [old shots dir]

With an old shots dir (the same run with hands.phone.mocap = 0), a third
column shows CoD4's C4 anims at the same moments.
"""
import glob
import json
import os
import sys

import cv2
import numpy as np

TW, TH = 480, 270
PARTS = {"raise": "phone_raise", "press": "phone_press", "putaway": "phone_putaway"}


def shots(d, call="c1"):
    out = {}
    for p in glob.glob(os.path.join(d, f"{call}_*.png")):
        _, part, ms = os.path.splitext(os.path.basename(p))[0].split("_")
        out.setdefault(part, []).append((int(ms), p))
    return {k: sorted(v) for k, v in out.items()}


def nearest(lst, ms):
    return min(lst, key=lambda x: abs(x[0] - ms)) if lst else None


def main():
    cfg_path, shot_dir, out = sys.argv[1], sys.argv[2], sys.argv[3]
    per = int(sys.argv[4]) if len(sys.argv) > 4 else 4
    old_dir = sys.argv[5] if len(sys.argv) > 5 else None
    cfg = json.load(open(cfg_path))
    here = os.path.dirname(os.path.abspath(cfg_path))
    cap = cv2.VideoCapture(os.path.normpath(os.path.join(here, "../../..", cfg["video"])))
    frames = []
    while True:
        ok, f = cap.read()
        if not ok:
            break
        frames.append(f)
    new, old = shots(shot_dir), shots(old_dir) if old_dir else {}
    clips = {c["name"]: c for c in cfg["clips"]}
    rows = []
    for part, name in PARTS.items():
        if part not in new:
            continue
        c = clips[name]
        span = new[part][-1][0] or 1
        for k in range(per):
            ms = span * k / max(per - 1, 1)
            got = nearest(new[part], ms)
            u = min(got[0] / 1000.0 / c["seconds"], 1.0)
            v = int(round(c["video"][0] + (c["video"][1] - c["video"][0]) * u))
            tiles = [cv2.resize(frames[min(v, len(frames) - 1)], (TW, TH))]
            cv2.putText(tiles[0], f"video {v}", (8, 22), 0, 0.6, (0, 0, 0), 2)
            g = cv2.resize(cv2.imread(got[1]), (TW, TH))
            cv2.putText(g, f"mocap {part} {got[0] / 1000:.2f}s", (8, 22), 0, 0.6, (255, 255, 255), 2)
            tiles.append(g)
            if old_dir:
                o = nearest(old.get(part, []), got[0])
                t = cv2.resize(cv2.imread(o[1]), (TW, TH)) if o else np.zeros((TH, TW, 3), np.uint8)
                cv2.putText(t, f"CoD4 C4 {part} {(o[0] if o else 0) / 1000:.2f}s", (8, 22), 0, 0.6, (255, 255, 255), 2)
                tiles.append(t)
            rows.append(np.hstack(tiles))
    cv2.imwrite(out, np.vstack(rows), [cv2.IMWRITE_JPEG_QUALITY, 88])
    print(out, len(rows), "rows")


if __name__ == "__main__":
    main()
