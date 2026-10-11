"""Track the hand in a phone video with MediaPipe Hands (21 keypoints).

track.py <video> <out.npz> [first last]

Saves per frame: image landmarks (pixels), world landmarks (metres, hand
centred), handedness score, and the hand's pose in camera space from
solvePnP (world landmarks against image landmarks), plus the video's fps
and size. Frames without a hand are NaN.
"""
import sys

import cv2
import mediapipe as mp
import numpy as np

# iPhone main camera, 1x video: ~26 mm equivalent, 69 degrees across.
HFOV_DEG = 69.4


def main():
    video, out = sys.argv[1], sys.argv[2]
    first = int(sys.argv[3]) if len(sys.argv) > 3 else 0
    last = int(sys.argv[4]) if len(sys.argv) > 4 else 10**9
    cap = cv2.VideoCapture(video)
    fps = cap.get(cv2.CAP_PROP_FPS)
    w, h = int(cap.get(3)), int(cap.get(4))
    f = (w / 2) / np.tan(np.radians(HFOV_DEG / 2))
    K = np.array([[f, 0, w / 2], [0, f, h / 2], [0, 0, 1]], np.float64)
    hands = mp.solutions.hands.Hands(static_image_mode=False, max_num_hands=1, model_complexity=1,
                                     min_detection_confidence=0.4, min_tracking_confidence=0.4)
    n_total = int(cap.get(cv2.CAP_PROP_FRAME_COUNT))
    img, world, score, rvec, tvec = [], [], [], [], []
    i = 0
    while True:
        ok, frame = cap.read()
        if not ok:
            break
        if i < first or i > last:
            i += 1
            continue
        res = hands.process(cv2.cvtColor(frame, cv2.COLOR_BGR2RGB))
        nan21 = np.full((21, 3), np.nan)
        if res.multi_hand_landmarks:
            lm = res.multi_hand_landmarks[0].landmark
            wl = res.multi_hand_world_landmarks[0].landmark
            p = np.array([[l.x * w, l.y * h, l.z * w] for l in lm])
            q = np.array([[l.x, l.y, l.z] for l in wl])
            s = res.multi_handedness[0].classification[0]
            ok, rv, tv = cv2.solvePnP(np.ascontiguousarray(q), np.ascontiguousarray(p[:, :2]), K, None, flags=cv2.SOLVEPNP_SQPNP)
            img.append(p)
            world.append(q)
            score.append(s.score if s.label == "Left" else -s.score)  # mirrored: "Left" = right hand on a rear camera
            rvec.append(rv.ravel() if ok else np.full(3, np.nan))
            tvec.append(tv.ravel() if ok else np.full(3, np.nan))
        else:
            img.append(nan21)
            world.append(nan21)
            score.append(np.nan)
            rvec.append(np.full(3, np.nan))
            tvec.append(np.full(3, np.nan))
        i += 1
    np.savez(out, img=np.array(img), world=np.array(world), score=np.array(score), rvec=np.array(rvec),
             tvec=np.array(tvec), fps=fps, size=(w, h), K=K, first=first, n_total=n_total)
    found = ~np.isnan(np.array(score))
    print(f"{len(score)} frames from {first}, hand in {found.sum()}; fps {fps:.2f}")


if __name__ == "__main__":
    main()
