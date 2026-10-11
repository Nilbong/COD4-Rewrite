"""Follow the hand back from the first frame MediaPipe found it, to where it
came into view (a hand cut off by the frame's edge isn't detected): optical
flow on the hand and what it holds, the median shift per frame.

track_entry.py <video> <track.npz> <out.npz> [frames back]
"""
import sys

import cv2
import numpy as np


def main():
    video, track, out = sys.argv[1], sys.argv[2], sys.argv[3]
    back = int(sys.argv[4]) if len(sys.argv) > 4 else 30
    d = np.load(track)
    first = int(d["first"])
    found = ~np.isnan(d["score"])
    k0 = int(np.argmax(found))
    f0 = first + k0
    p0 = d["img"][k0][:, :2]
    cap = cv2.VideoCapture(video)
    frames = {}
    i = 0
    while True:
        ok, fr = cap.read()
        if not ok or i > f0:
            break
        if i >= f0 - back:
            frames[i] = cv2.cvtColor(fr, cv2.COLOR_BGR2GRAY)
        i += 1
    lo, hi = p0.min(0) - 40, p0.max(0) + 40
    mask = np.zeros_like(frames[f0])
    mask[int(max(lo[1], 0)):int(hi[1]), int(max(lo[0], 0)):int(hi[0])] = 255
    pts = cv2.goodFeaturesToTrack(frames[f0], 200, 0.01, 6, mask=mask)
    shift = {f0: np.zeros(2)}
    total = np.zeros(2)
    for f in range(f0 - 1, f0 - back - 1, -1):
        if pts is None or len(pts) < 6:
            break
        nxt, st, _ = cv2.calcOpticalFlowPyrLK(frames[f + 1], frames[f], pts, None, winSize=(31, 31), maxLevel=3)
        good = st.ravel() == 1
        # Points gone past the bottom edge are lost.
        good &= nxt[:, 0, 1] < frames[f].shape[0] - 4
        if good.sum() < 6:
            break
        total = total + np.median((nxt - pts)[good, 0], axis=0)
        shift[f] = total.copy()
        pts = nxt[good]
    fs = sorted(shift)
    np.savez(out, frames=np.array(fs), shift=np.array([shift[f] for f in fs]), anchor=f0)
    print(f"entry: followed back from {f0} to {fs[0]}; shift there {shift[fs[0]].round(1)} px")


if __name__ == "__main__":
    main()
