"""Stick-figure preview of solved clips next to the video, before running
the game: the arms and the phone box seen from the eye (CoD4's 65 degree
view at 16:9), and a close-up of the hand.

preview.py <config.json> <out.jpg> [clip] [step]
"""
import json
import os
import sys

import cv2
import numpy as np

W, H = 640, 360
HFOV43 = 80.0  # the game's view here (an overlay on a screenshot matched 80)


def project(p, f, cx, cy):
    # CoD view (x forward, y left, z up) to the image.
    x, y, z = p[..., 0], p[..., 1], p[..., 2]
    return np.stack([cx - f * y / x, cy - f * z / x], axis=-1)


def box_corners(lo, hi):
    return np.array([[x, y, z] for x in (lo[0], hi[0]) for y in (lo[1], hi[1]) for z in (lo[2], hi[2])])


EDGES = [(0, 1), (0, 2), (1, 3), (2, 3), (4, 5), (4, 6), (5, 7), (6, 7), (0, 4), (1, 5), (2, 6), (3, 7)]


def clip_near(a, b, near=2.0):
    """A segment cut at the near plane (x = near), or None behind it."""
    if a[0] < near and b[0] < near:
        return None
    if a[0] < near:
        a = a + (b - a) * (near - a[0]) / (b[0] - a[0])
    elif b[0] < near:
        b = b + (a - b) * (near - b[0]) / (a[0] - b[0])
    return a, b


def draw(img, frame, names, parent, box, f, cx, cy, thick=2, hand_only=False):
    pos = np.array(frame["pos"])
    keep = [i for i, n in enumerate(names) if n.startswith("j_") and ("_ri" in n or "_le" in n) and "sleave" not in n]
    if hand_only:
        keep = [i for i in keep if names[i].endswith(("_ri", "_ri_0", "_ri_1", "_ri_2", "_ri_3")) and "shoulder" not in names[i] and "elbow" not in names[i]]
    for i in keep:
        p = parent[i]
        if p < 0 or p not in keep:
            continue
        col = (40, 40, 220) if "_ri" in names[i] else (200, 120, 40)
        if "thumb" in names[i]:
            col = (0, 200, 255)
        seg = clip_near(pos[p], pos[i])
        if seg is None:
            continue
        a, b = project(np.array(seg), f, cx, cy)
        cv2.line(img, tuple(map(int, a)), tuple(map(int, b)), col, thick)
    Pr, Po = np.array(frame["phone_rot"]), np.array(frame["phone_org"])
    c = box_corners(*map(np.array, box)) @ Pr.T + Po
    pc = project(c, f, cx, cy)
    for a, b in EDGES:
        cv2.line(img, tuple(map(int, pc[a])), tuple(map(int, pc[b])), (60, 160, 60), 1)
    # The screen face (-y).
    face = [k for k in range(8) if (k // 2) % 2 == 0]
    cv2.polylines(img, [pc[[face[0], face[1], face[3], face[2]]].astype(np.int32)], True, (0, 220, 0), 2)


def anat(w, i, m, p):
    y = m - w
    y /= np.linalg.norm(y)
    x = i - p
    x -= (x @ y) * y
    x /= np.linalg.norm(x)
    return np.stack([x, y, np.cross(x, y)], axis=1)


def hand_views(frame, dbg, size=H, px=26.0):
    """The hand in its own frame: palm-on (x across, y to the fingers) and
    side-on (y to the fingers, z out of the palm), orthographic."""
    names, parent = dbg["names"], dbg["parent"]
    pos = np.array(frame["pos"])
    ix = names.index
    w = pos[ix("j_wrist_ri")]
    A = anat(w, pos[ix("j_index_ri_0")], pos[ix("j_mid_ri_0")], pos[ix("j_pinky_ri_0")])
    loc = (pos - w) @ A
    Pr, Po = np.array(frame["phone_rot"]), np.array(frame["phone_org"])
    corners = (box_corners(*map(np.array, dbg["phone_box"])) @ Pr.T + Po - w) @ A
    out = []
    for ax_h, ax_v, label in ((0, 1, "palm-on"), (1, 2, "side-on")):
        img = np.full((size, size, 3), 250, np.uint8)
        def pt(p):
            return (int(size / 2 + p[ax_h] * px), int(size * 0.85 - (p[ax_v] + (0 if ax_v == 1 else 0)) * px)) if ax_v == 1 else (int(size * 0.15 + p[ax_h] * px), int(size / 2 - p[ax_v] * px))
        for a, b in EDGES:
            cv2.line(img, pt(corners[a]), pt(corners[b]), (60, 170, 60), 1)
        for k, n in enumerate(names):
            if not (n.startswith("j_") and "_ri" in n) or any(s in n for s in ("shoulder", "elbow", "sleave", "twist", "webbing")):
                continue
            p = parent[k]
            if p < 0 or "elbow" in names[p]:
                continue
            col = (0, 170, 255) if "thumb" in n else (40, 40, 220)
            cv2.line(img, pt(loc[p]), pt(loc[k]), col, 2)
        cv2.putText(img, label, (6, size - 8), 0, 0.5, (0, 0, 0), 1)
        out.append(img)
    return np.hstack(out)


def main():
    cfg_path, out = sys.argv[1], sys.argv[2]
    only = sys.argv[3] if len(sys.argv) > 3 and sys.argv[3] != "all" else None
    step = int(sys.argv[4]) if len(sys.argv) > 4 else 6
    cfg = json.load(open(cfg_path))
    here = os.path.dirname(os.path.abspath(cfg_path))
    dbg = json.load(open(os.path.normpath(os.path.join(here, cfg["debug"]))))
    video = os.path.normpath(os.path.join(here, "../../..", cfg["video"]))
    cap = cv2.VideoCapture(video)
    frames = []
    while True:
        ok, fr = cap.read()
        if not ok:
            break
        frames.append(fr)
    vf = W / 2 / np.tan(np.radians(HFOV43 / 2)) * (4 / 3) / (16 / 9) * (16 / 9) / (4 / 3)
    # 65 degrees across a 4:3 view, the same height at 16:9.
    f = (W * 0.75 / 2) / np.tan(np.radians(HFOV43 / 2))
    tiles = []
    for clip in cfg["clips"]:
        if only and clip["name"] != only:
            continue
        a, b = clip["video"]
        seq = dbg["clips"][clip["name"]]
        n = len(seq)
        for i in range(0, n, step):
            u = i / (n - 1)
            v = int(round(a + (b - a) * u))
            game = np.full((H, W, 3), 235, np.uint8)
            draw(game, seq[i], dbg["names"], dbg["parent"], dbg["phone_box"], f, W / 2, H / 2)
            vid = cv2.resize(frames[min(v, len(frames) - 1)], (W, H))
            row = np.hstack([vid, game, hand_views(seq[i], dbg)])
            cv2.putText(row, f"{clip['name']} {u * clip['seconds']:.2f}s  video {v}", (8, 24), 0, 0.7, (0, 0, 0), 2)
            tiles.append(row)
    sheet = np.vstack(tiles)
    scale = min(1.0, 2400 / sheet.shape[0])
    cv2.imwrite(out, cv2.resize(sheet, None, fx=scale, fy=scale), [cv2.IMWRITE_JPEG_QUALITY, 85])
    print(out, sheet.shape)


if __name__ == "__main__":
    main()
