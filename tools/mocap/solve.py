"""Retarget a tracked phone take onto CoD4's viewmodel arms and write the
call-in clips (raise, press, put away) for crates/game.

solve.py <config.json>

The config names the inputs (track/entry npz, rig dump), the video frames
each clip spans, and the fitting numbers (see takes/phone_take1.json).

What it does, per video frame:
  * the hand's 21 keypoints in camera space (MediaPipe's world landmarks
    placed by solvePnP), its entry (cut off by the frame's edge) from the
    optical flow, before and after extrapolated; zero-phase low-pass
    filtered (offline, so no lag, unlike a One Euro filter);
  * the wrist: the hand's own frame (fingers, across the knuckles, palm
    normal) turned from camera to view axes as it is (the take was filmed
    from the eye), its position pinned so the hold lands where CoD4's phone
    hold has it, displacements scaled for the viewmodel's nearer drawing;
  * the arm: two-bone IK from CoD4's shoulder (fixed lengths), the elbow
    bending in the hold pose's plane, half the forearm's twist on
    `j_wristtwist_ri`;
  * the fingers: each bone swung to the take's direction in the hand's
    frame, then curled (or opened) until it touches the phone, held in the
    palm as the take holds the remote (screen out of the palm, along the
    knuckles, fingers over its face);
  * the phone: rigid in the hand, through `tag_clip2` (what crates/game's
    PropAnchor follows), the prop's default knob offsets taken out;
  * the left arm held at CoD4's hold (down, out of sight), the torso tags
    from the C4 anim the
    clip replaces, retimed.
"""
import json
import os
import sys

import numpy as np
from scipy.signal import butter, filtfilt
from scipy.spatial.transform import Rotation as R

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rig import Rig  # noqa: E402

M_TO_IN = 39.3701
# Camera (x right, y down, z forward) to CoD view axes (x forward, y left, z up).
CAM_TO_COD = np.array([[0, 0, 1], [-1, 0, 0], [0, -1, 0]], float)

FINGERS = {
    "thumb": (["j_thumb_ri_0", "j_thumb_ri_1", "j_thumb_ri_2", "j_thumb_ri_3"], [1, 2, 3, 4]),
    "index": (["j_index_ri_0", "j_index_ri_1", "j_index_ri_2", "j_index_ri_3"], [5, 6, 7, 8]),
    "mid": (["j_mid_ri_0", "j_mid_ri_1", "j_mid_ri_2", "j_mid_ri_3"], [9, 10, 11, 12]),
    "ring": (["j_ring_ri_0", "j_ring_ri_1", "j_ring_ri_2", "j_ring_ri_3"], [13, 14, 15, 16]),
    "pinky": (["j_pinky_ri_0", "j_pinky_ri_1", "j_pinky_ri_2", "j_pinky_ri_3"], [17, 18, 19, 20]),
}


def anat(wrist, index, mid, pinky):
    """The hand's frame: x across the knuckles to the index, y to the
    fingers, z out of the palm (right hand), as matrix columns."""
    y = mid - wrist
    y /= np.linalg.norm(y)
    x = index - pinky
    x -= (x @ y) * y
    x /= np.linalg.norm(x)
    return np.stack([x, y, np.cross(x, y)], axis=1)


def lowpass(x, fs, cutoff, axis=0):
    b, a = butter(2, cutoff / (fs / 2))
    return filtfilt(b, a, x, axis=axis, padlen=min(30, x.shape[axis] - 1))


def box_sd(p, lo, hi):
    """Signed distance from points to an axis-aligned box."""
    c, h = (lo + hi) / 2, (hi - lo) / 2
    q = np.abs(p - c) - h
    return np.linalg.norm(np.maximum(q, 0), axis=-1) + np.minimum(q.max(axis=-1), 0)


def ease(t):
    t = np.clip(t, 0, 1)
    return t * t * (3 - 2 * t)


class Take:
    """The tracked hand over a range of video frames, gaps filled."""

    def __init__(self, cfg):
        d = np.load(cfg["track"])
        e = np.load(cfg["entry"])
        self.fps = float(d["fps"])
        K = d["K"]
        first = int(d["first"])
        img, world = d["img"], d["world"]
        n = len(img)
        cam = np.full((n, 21, 3), np.nan)
        for k in range(n):
            if np.isnan(d["score"][k]) or np.isnan(d["rvec"][k]).any():
                continue
            Rk = R.from_rotvec(d["rvec"][k]).as_matrix()
            pts = world[k] @ Rk.T + d["tvec"][k]
            # Reprojection check: a bad fit is dropped.
            proj = pts @ K.T
            err = np.linalg.norm(proj[:, :2] / proj[:, 2:] - img[k][:, :2], axis=1).mean()
            if err < cfg.get("max_reproj_px", 12):
                cam[k] = pts
        v0, v1 = cfg["frames"]
        self.frames = np.arange(v0, v1 + 1)
        L = np.full((len(self.frames), 21, 3), np.nan)
        for i, v in enumerate(self.frames):
            if 0 <= v - first < n:
                L[i] = cam[v - first]
        ok = ~np.isnan(L[:, 0, 0])
        tracked = np.flatnonzero(ok)
        a, b = tracked[0], tracked[-1]
        # Gaps inside: linear.
        for j in range(21):
            for c in range(3):
                L[a:b + 1, j, c] = np.interp(np.arange(a, b + 1), tracked, L[tracked, j, c])
        # The entry: the first found pose, moved by the optical flow's shift
        # (at its depth).
        f = K[0, 0]
        anchor = int(e["anchor"])
        shift = dict(zip(e["frames"].tolist(), e["shift"]))
        entry_first = max(cfg.get("entry_from", min(shift)), min(shift))
        ia = int(np.flatnonzero(self.frames == anchor)[0])
        z = L[ia, :, 2]
        for v in range(entry_first, anchor):
            i = v - v0
            if i < 0:
                continue
            s = shift[v]
            L[i] = L[ia]
            L[i, :, 0] += s[0] * z / f
            L[i, :, 1] += s[1] * z / f
        ent = entry_first - v0
        # Before: on along the entry's velocity (backwards), and after the
        # last tracked frame along its velocity, the hand's shape held.
        wv = cfg.get("extrap_window", 4)
        vel_in = (L[ent + wv] - L[ent]) / wv
        for i in range(ent):
            L[i] = L[ent] - vel_in.mean(axis=0) * (ent - i)
        vel_out = (L[b] - L[b - wv]) / wv
        # (In the picture only: depth from one camera drifts.)
        vel_out[:, 2] = 0.0
        grow = cfg.get("exit_accel", 1.0)
        for i in range(b + 1, len(self.frames)):
            k = i - b
            L[i] = L[b] + vel_out.mean(axis=0) * (k + 0.5 * (grow - 1) * k * k / max(1, wv))
        self.tracked = (a + v0, b + v0)
        self.entry_first = entry_first
        self.L = L

    def signals(self, cfg):
        """Smoothed wrist position (camera, m), hand frame (as quats) and
        finger directions in the hand's frame, per frame."""
        L = self.L
        n = len(L)
        A = np.array([anat(l[0], l[5], l[9], l[17]) for l in L])
        # The hand's shape (frame, fingers) before the entry's pose is the
        # entry's (the flow only moves it); after tracking, the last.
        a = self.tracked[0] - self.frames[0]
        b = self.tracked[1] - self.frames[0]
        A[:a] = A[a]
        A[b + 1:] = A[b]
        q = R.from_matrix(A).as_quat()
        for i in range(1, n):
            if q[i] @ q[i - 1] < 0:
                q[i] = -q[i]
        q = lowpass(q, self.fps, cfg.get("rot_hz", 4.0))
        q /= np.linalg.norm(q, axis=1, keepdims=True)
        pos = lowpass(L[:, 0], self.fps, cfg.get("pos_hz", 5.0))
        segs = {}
        for name, (_, mp) in FINGERS.items():
            dirs = []
            for k in range(3):
                v = L[:, mp[k + 1]] - L[:, mp[k]]
                v = np.einsum("nji,nj->ni", A, v)  # into the hand's frame
                v[:a] = v[a]
                v[b + 1:] = v[b]
                dirs.append(v / np.linalg.norm(v, axis=1, keepdims=True))
            d = lowpass(np.array(dirs), self.fps, cfg.get("finger_hz", 4.0), axis=1)
            segs[name] = d / np.linalg.norm(d, axis=2, keepdims=True)
        return pos, q, segs


class Solver:
    def __init__(self, cfg, rig):
        self.cfg, self.rig = cfg, rig
        ix = rig.idx
        self.ix = ix
        hold = rig.anim(cfg["hold_anim"])
        self.hold_local = rig.local_pose(hold, cfg.get("hold_frame", 0))
        gr, gp = rig.fk(*self.hold_local)
        self.hold_g = (gr, gp)
        bgr, bgp = rig.fk(*rig.local_pose(None))
        # The CoD hand's frame against its wrist joint (bind pose).
        A_bind = anat(bgp[ix["j_wrist_ri"]], bgp[ix["j_index_ri_0"]], bgp[ix["j_mid_ri_0"]], bgp[ix["j_pinky_ri_0"]])
        self.M = bgr[ix["j_wrist_ri"]].as_matrix().T @ A_bind
        # Its proportions (inches), in the hand's frame.
        Wb = bgp[ix["j_wrist_ri"]]
        self.palm = {n: A_bind.T @ (bgp[ix[n]] - Wb) for n in ("j_index_ri_0", "j_mid_ri_0", "j_ring_ri_0", "j_pinky_ri_0", "j_thumb_ri_0")}

    # The phone in the hand's frame: its axes and origin (bottom of the back).
    def grip(self):
        g = self.cfg["grip"]
        if getattr(self, "fitted", None) is not None:
            return self.fitted
        # Phone +z (its length) along +x (to the index), its screen (-y) out
        # of the palm (+z), so its +x is to the wrist (-y).
        rot = np.stack([[0, -1, 0], [0, 0, -1], [1, 0, 0]], axis=1).astype(float)
        mid = self.palm["j_mid_ri_0"]
        pinky = self.palm["j_pinky_ri_0"]
        origin = np.array([pinky[0] + g["bottom_from_pinky"], mid[1] + g["centre_from_knuckles"], g["palm_offset"]])
        return rot, origin

    def fit_grip(self, base_local, wrist_pos, A_hand, segs):
        """Place the phone against the take's own fingers at the hold: its
        back resting on the index-to-ring fingers' first two bones, its far
        edge just inside their last knuckles (the tips wrap round it), its
        bottom by the pinky."""
        g = self.cfg["grip"]
        saved = self.cfg["contact"]
        self.cfg["contact"] = dict(saved, close=[0, 0, 0], close_thumb=[0, 0, 0], open=0, lift_thumb=0)
        self.fitted = (np.eye(3), np.array([0.0, 0.0, -100.0]))  # out of the way
        rots, poss, _, _, _ = self.solve_frame(base_local, wrist_pos, A_hand, segs)
        self.cfg["contact"] = saved
        rig, ix = self.rig, self.ix
        gr, gp = rig.fk(rots, poss)
        W = gp[ix["j_wrist_ri"]]
        A = gr[ix["j_wrist_ri"]].as_matrix() @ self.M
        loc = lambda n: A.T @ (gp[ix[n]] - W)
        fingers = ("index", "mid", "ring")
        # The plane the fingers lie in (knuckles to last knuckles), tilted
        # up from the palm about the across axis.
        v = np.mean([loc(f"j_{f}_ri_2") - loc(f"j_{f}_ri_0") for f in fingers], axis=0)
        tilt = np.arctan2(v[2], v[1])
        ey = np.array([0, np.cos(tilt), np.sin(tilt)])
        ez = np.array([0, -np.sin(tilt), np.cos(tilt)])
        ex = np.array([1.0, 0, 0])
        far = np.mean([loc(f"j_{f}_ri_2") @ ey for f in fingers]) - g["far_inset"]
        back = max(loc(f"j_{f}_ri_{k}") @ ez for f in fingers for k in (0, 1)) + self.cfg["contact"]["radius"] + g["back_gap"]
        lo, hi = self.phone_box()
        half_w = (hi[0] - lo[0]) / 2
        pinky = loc("j_pinky_ri_0")
        # Phone +z along the knuckles, its screen (-y) out of the fingers' plane.
        rot = np.stack([-ey, -ez, ex], axis=1)
        origin = ex * (pinky[0] + g["bottom_from_pinky"]) + ey * (far - half_w) + ez * back
        self.report_tilt = float(np.degrees(tilt))
        # Turned about its screen's normal (round its middle) so that, seen
        # from the eye, it leans as the remote does in the take's hold.
        if "video_phone_lean_deg" in g:
            centre = np.array([0.0, (lo[1] + hi[1]) / 2, (lo[2] + hi[2]) / 2])
            mid = origin + rot @ centre

            def turned(spin):
                c, s_ = np.cos(np.radians(spin)), np.sin(np.radians(spin))
                ry = np.array([[c, 0, s_], [0, 1, 0], [-s_, 0, c]])
                r2 = rot @ ry
                return r2, mid - r2 @ centre

            def lean(spin):
                r2, o2 = turned(spin)
                a = W + A @ o2
                b = W + A @ (o2 + r2[:, 2] * hi[2])
                pa, pb = np.array([-a[1] / a[0], -a[2] / a[0]]), np.array([-b[1] / b[0], -b[2] / b[0]])
                # Its bottom is at z = 0, its +z runs up to the antenna.
                # Image x right, y down; lean from upright, + right.
                d = pb - pa
                return np.degrees(np.arctan2(d[0], -d[1]))

            spins = np.arange(-120, 121, 1.0)
            want = g["video_phone_lean_deg"]
            err = lambda sp: abs(((lean(sp) - want) + 180) % 360 - 180)
            # The least turn that leans it so (within 2 degrees).
            near = [sp for sp in spins if err(sp) < 2.0]
            best = min(near, key=abs) if near else min(spins, key=err)
            self.report_spin = (float(best), float(lean(0.0)), float(lean(best)))
            rot, origin = turned(best)
        self.fitted = (rot, origin)
        return origin

    def phone_box(self):
        s = self.cfg["phone"]["scale"]
        lo = np.array(self.cfg["phone"]["min"]) * s
        hi = np.array(self.cfg["phone"]["max"]) * s
        return lo, hi

    def solve_frame(self, base_local, wrist_pos, A_hand, segs, press=0.0, prev_curl=None):
        """Locals for one frame. `base_local`: the C4 anim's pose there."""
        rig, ix, cfg = self.rig, self.ix, self.cfg
        rots, poss = [list(base_local[0]), [p.copy() for p in base_local[1]]]
        # The left arm stays as CoD4 holds it (down, out of sight): the C4's
        # two-handed pullout and putaway would bring it up holding nothing.
        if cfg.get("left_arm_from_hold", True):
            for k, n in enumerate(rig.names):
                if "_le" in n:
                    rots[k] = self.hold_local[0][k]
                    poss[k] = self.hold_local[1][k].copy()
        # Right arm off the hold's (the C4 anim's right arm is replaced).
        for n in ("j_shoulder_ri", "j_elbow_ri", "j_wrist_ri", "j_wristtwist_ri"):
            rots[ix[n]] = self.hold_local[0][ix[n]]
            poss[ix[n]] = self.hold_local[1][ix[n]].copy()
        gr, gp = rig.fk(rots, poss)
        S = gp[ix["j_shoulder_ri"]]
        hgr, hgp = self.hold_g
        Sh, Eh, Wh = hgp[ix["j_shoulder_ri"]], hgp[ix["j_elbow_ri"]], hgp[ix["j_wrist_ri"]]
        L1, L2 = np.linalg.norm(Eh - Sh), np.linalg.norm(Wh - Eh)
        T = wrist_pos
        d = T - S
        dist = np.linalg.norm(d)
        reach = (L1 + L2) * 0.995
        clamped = dist > reach
        dist_c = np.clip(dist, abs(L1 - L2) + 0.5, reach)
        u = d / dist
        T = S + u * dist_c
        # Elbow: in the hold's bend plane, turned with the arm.
        pole = (Eh - Sh) - ((Eh - Sh) @ ((Wh - Sh) / np.linalg.norm(Wh - Sh))) * (Wh - Sh) / np.linalg.norm(Wh - Sh)
        pole = pole + np.array(cfg.get("elbow_pole_bias", [0, 0, 0]))
        pole -= (pole @ u) * u
        pole /= np.linalg.norm(pole)
        a = (L1 * L1 - L2 * L2 + dist_c * dist_c) / (2 * dist_c)
        h = np.sqrt(max(L1 * L1 - a * a, 0))
        E = S + u * a + pole * h

        def frame(dir_, n):
            dir_ = dir_ / np.linalg.norm(dir_)
            n = n - (n @ dir_) * dir_
            n /= np.linalg.norm(n)
            return np.stack([dir_, n, np.cross(dir_, n)], axis=1)

        n_ref = np.cross(Eh - Sh, Wh - Eh)
        n_new = np.cross(E - S, T - E)
        if np.linalg.norm(n_new) < 1e-6:
            n_new = n_ref
        Rs = R.from_matrix(frame(E - S, n_new) @ frame(Eh - Sh, n_ref).T) * hgr[ix["j_shoulder_ri"]]
        Re = R.from_matrix(frame(T - E, n_new) @ frame(Wh - Eh, n_ref).T) * hgr[ix["j_elbow_ri"]]
        Rw = R.from_matrix(A_hand @ self.M.T)
        par_s = gr[rig.parent[ix["j_shoulder_ri"]]]
        rots[ix["j_shoulder_ri"]] = par_s.inv() * Rs
        rots[ix["j_elbow_ri"]] = Rs.inv() * Re
        wl = Re.inv() * Rw
        rots[ix["j_wrist_ri"]] = wl
        # Forearm twist: half of the wrist's about the forearm, on the twist bone.
        axis = poss[ix["j_wrist_ri"]] / np.linalg.norm(poss[ix["j_wrist_ri"]])
        hold_wl = self.hold_local[0][ix["j_wrist_ri"]]
        rel = (wl * hold_wl.inv()).as_quat()
        tw = np.array([*(axis * (rel[:3] @ axis)), rel[3]])
        tw = R.from_quat(tw / np.linalg.norm(tw)) if np.linalg.norm(tw) > 1e-6 else R.identity()
        half = R.from_rotvec(tw.as_rotvec() * cfg.get("twist_share", 0.5))
        rots[ix["j_wristtwist_ri"]] = half * self.hold_local[0][ix["j_wristtwist_ri"]]

        # Fingers: swing each bone to the take's direction, from the hold's
        # finger pose (not the replaced anim's, whose own twists would leak
        # into the take's motion).
        for js, _ in FINGERS.values():
            for jn in js:
                rots[ix[jn]] = self.hold_local[0][ix[jn]]
        gr, gp = rig.fk(rots, poss)
        A_cod = Rw.as_matrix() @ self.M
        grot, gorg = self.grip()
        P_rot = A_cod @ grot
        P_org = gp[ix["j_wrist_ri"]] + A_cod @ gorg
        lo, hi = self.phone_box()
        curls = {}
        for name, (joints, _) in FINGERS.items():
            base = [rots[ix[j]] for j in joints]
            for k in range(3):
                j = ix[joints[k]]
                g = gr[rig.parent[j]] * rots[j]
                cur = g.apply(poss[ix[joints[k + 1]]])
                want = A_cod @ segs[name][k]
                cur /= np.linalg.norm(cur)
                ax = np.cross(cur, want)
                s = np.linalg.norm(ax)
                ang = np.arctan2(s, cur @ want)
                swing = R.from_rotvec(ax / s * ang) if s > 1e-8 else R.identity()
                rots[j] = gr[rig.parent[j]].inv() * swing * g
                gr, gp = rig.fk(rots, poss)
        # The grip (fitted once at the hold, see `optimize_grip`), with a
        # share of the take's own finger motion against its hold on top,
        # and the thumb's press.
        if getattr(self, "grip_pose", None) is not None:
            share = cfg.get("finger_motion", 0.3)
            for name, (joints, _) in FINGERS.items():
                for k in range(3):
                    j = ix[joints[k]]
                    rel = (self.mocap_hold[j].inv() * rots[j]).as_rotvec() * share
                    cap = np.radians(cfg.get("finger_motion_max_deg", 15.0))
                    if np.linalg.norm(rel) > cap:
                        rel *= cap / np.linalg.norm(rel)
                    rots[j] = self.grip_pose[j] * R.from_rotvec(rel)
            if press:
                gr, gp = rig.fk(rots, poss)
                rots = self.curl_finger(rots, poss, FINGERS["thumb"][0], [0.0, press * cfg["contact"]["press_deg"] * self.press_sign, press * cfg["contact"]["press_deg"] * self.press_sign * 0.7], A_cod[:, 2])
        return rots, poss, curls, clamped, (P_rot, P_org)

    def curl_finger(self, rots, poss, joints, deltas, palm_n):
        """The finger's joints each closed by its delta (degrees), all about
        one hinge axis: across the finger's first bone and the palm normal
        (closing turns it toward the palm, and folds the rest on over)."""
        rig, ix = self.rig, self.ix
        rots = list(rots)
        pj = rig.parent[ix[joints[0]]]
        gr, _ = rig.fk(rots, poss)
        g_par = gr[pj]
        seg0 = (g_par * rots[ix[joints[0]]]).apply(poss[ix[joints[1]]])
        ax = np.cross(seg0 / np.linalg.norm(seg0), palm_n)
        if np.linalg.norm(ax) < 1e-6:
            return rots
        ax /= np.linalg.norm(ax)
        for k in range(3):
            j = ix[joints[k]]
            g = g_par * rots[j]
            if deltas[k]:
                g = R.from_rotvec(ax * np.radians(deltas[k])) * g
            rots[j] = g_par.inv() * g
            g_par = g
        return rots

    def finger_sd(self, rots, poss, joints, k0, P_rot, P_org, lo, hi, k1=3):
        """Nearest approach to the phone of the finger's bones from `k0` on
        (the knuckle's first half lies under the phone's back, on the palm)."""
        rig, ix = self.rig, self.ix
        _, gp = rig.fk(rots, poss)
        pts = []
        for k in range(k0, k1):
            a0, a1 = gp[ix[joints[k]]], gp[ix[joints[k + 1]]]
            for t in np.linspace(0.5 if k == 0 else 0.0, 1.0, 6):
                pts.append(a0 + (a1 - a0) * t)
        local = (np.array(pts) - P_org) @ P_rot
        return box_sd(local, lo, hi).min()

    def optimize_grip(self, base_local, wrist_pos, A_hand, segs):
        """The fingers' grip on the phone at the hold: each finger's three
        joints closed (or opened) about its hinge so that nothing goes
        through the phone, its middle and tip bones rest on it, and it stays
        as near the take as that allows."""
        from scipy.optimize import minimize
        rig, ix, cfg = self.rig, self.ix, self.cfg
        rots, poss, _, _, P = self.solve_frame(base_local, wrist_pos, A_hand, segs)
        self.mocap_hold = list(rots)
        P_rot, P_org = P
        lo, hi = self.phone_box()
        gr, _ = rig.fk(rots, poss)
        z = (gr[ix["j_wrist_ri"]].as_matrix() @ self.M)[:, 2]
        c = cfg["contact"]
        r = c["radius"]
        out = list(rots)
        self.grip_report = {}
        for name, (joints, _) in FINGERS.items():
            def cost(d):
                rr = self.curl_finger(rots, poss, joints, list(d), z)
                _, gp = rig.fk(rr, poss)
                total = c.get("reg", 0.0005) * float(np.sum(np.square(d)))
                for k in range(3):
                    a0, a1 = gp[ix[joints[k]]], gp[ix[joints[k + 1]]]
                    pts = a0 + (a1 - a0) * np.linspace(0.5 if k == 0 else 0.0, 1.0, 8)[:, None]
                    sd = box_sd((pts - P_org) @ P_rot, lo, hi)
                    total += 200.0 * float(np.sum(np.maximum(r - sd, 0) ** 2))
                    if k >= 1:
                        total += c.get("touch", 5.0) * (sd.min() - r) ** 2
                return total
            best = None
            for start in ([0, 0, 0], [10, 30, 40], [-10, -20, -20], [20, 60, 80]):
                res = minimize(cost, np.array(start, float), method="Powell", options={"xtol": 0.5, "ftol": 1e-4})
                if best is None or res.fun < best.fun:
                    best = res
            d = np.clip(best.x, -60, 110)
            rr = self.curl_finger(rots, poss, joints, list(d), z)
            for j in joints:
                out[ix[j]] = rr[ix[j]]
            self.grip_report[name] = {"close_deg": np.round(d, 1).tolist(), "cost": round(float(best.fun), 4)}
        self.grip_pose = out
        # The thumb's press: toward the phone's face.
        tj = FINGERS["thumb"][0]
        def tip_sd(sign):
            rr = self.curl_finger(out, poss, tj, [0.0, 15.0 * sign, 10.0 * sign], z)
            _, gp = rig.fk(rr, poss)
            return box_sd(((gp[ix[tj[3]]] - P_org) @ P_rot)[None], lo, hi)[0]
        self.press_sign = 1.0 if tip_sd(1.0) < tip_sd(-1.0) else -1.0
        return self.grip_report

    def apply_curls(self, rots, poss, curls):
        """The fingers posed with given (smoothed) closing angles."""
        gr, _ = self.rig.fk(rots, poss)
        z = (gr[self.ix["j_wrist_ri"]].as_matrix() @ self.M)[:, 2]
        for name, (joints, _) in FINGERS.items():
            rots = self.curl_finger(rots, poss, joints, curls[name], z)
        return rots

    def phone_tag(self, rots, poss, P):
        """`tag_clip2`'s local so the prop (its default knob offsets on top)
        lands at P."""
        rig, ix = self.rig, self.ix
        knob = self.cfg["knob"]
        K_rot = R.from_euler("z", knob["yaw"], degrees=True)
        K_pos = np.array([knob["x"], knob["y"], knob["z"]])
        P_rot, P_org = P
        tag_rot = R.from_matrix(P_rot) * K_rot.inv()
        tag_pos = P_org - tag_rot.apply(K_pos)
        gr, gp = rig.fk(rots, poss)
        j = ix["tag_clip2"]
        par = rig.parent[j]
        return gr[par].inv() * tag_rot, gr[par].inv().apply(tag_pos - gp[par])


def main():
    cfg = json.load(open(sys.argv[1]))
    here = os.path.dirname(os.path.abspath(sys.argv[1]))
    for k in ("track", "entry", "rig", "out_dir", "debug"):
        if k in cfg:
            cfg[k] = os.path.normpath(os.path.join(here, cfg[k]))
    rig = Rig(cfg["rig"])
    take = Take(cfg)
    pos_cam, q_cam, segs = take.signals(cfg)
    solver = Solver(cfg, rig)
    fps_v = take.fps
    v0 = take.frames[0]
    # Pin: the take's hold wrist onto the C4 hold's.
    hv = cfg["hold_video_frame"] - v0
    hold_cod = CAM_TO_COD @ pos_cam[hv] * M_TO_IN
    Wh = solver.hold_g[1][rig.idx["j_wrist_ri"]]
    s = cfg.get("displacement_scale", 1.0)

    def wrist_at(fv):
        i0 = int(np.floor(fv))
        t = fv - i0
        p = pos_cam[i0] * (1 - t) + pos_cam[min(i0 + 1, len(pos_cam) - 1)] * t
        out = Wh + s * (CAM_TO_COD @ p * M_TO_IN - hold_cod) + np.array(cfg.get("hold_offset", [0, 0, 0]))
        # Past the take's last tracked frame: on down out of sight (the
        # game holds the phone lower in view than the take did).
        past = fv + v0 - take.tracked[1]
        if past > 0 and "exit_drop" in cfg:
            out = out + np.array(cfg["exit_drop"]) * ease(past / cfg["exit_drop_frames"])
        return out

    def hand_at(fv):
        i0 = int(np.floor(fv))
        t = fv - i0
        i1 = min(i0 + 1, len(q_cam) - 1)
        qa, qb = q_cam[i0], q_cam[i1]
        q = qa * (1 - t) + qb * t
        A = R.from_quat(q / np.linalg.norm(q)).as_matrix()
        return CAM_TO_COD @ A

    def segs_at(fv):
        i0 = int(np.floor(fv))
        t = fv - i0
        i1 = min(i0 + 1, len(q_cam) - 1)
        out = {}
        for n, d in segs.items():
            v = d[:, i0] * (1 - t) + d[:, i1] * t
            out[n] = v / np.linalg.norm(v, axis=1, keepdims=True)
        return out

    os.makedirs(cfg["out_dir"], exist_ok=True)
    report = {"tracked": [int(x) for x in take.tracked], "entry_from": int(take.entry_first), "clips": {}}
    hold_A = hand_at(hv)
    hold_cod_A = solver.hold_g[0][rig.idx["j_wrist_ri"]].as_matrix() @ solver.M
    report["hold_rotation_vs_c4_deg"] = float(np.degrees(R.from_matrix(hold_A @ hold_cod_A.T).magnitude()))
    debug = {}
    fps = cfg.get("fps", 60)
    if cfg["grip"].get("fit"):
        hold_clip = next(c for c in cfg["clips"] if c["video"][0] <= cfg["hold_video_frame"] <= c["video"][1])
        hb = rig.anim(hold_clip["base_anim"])
        o = solver.fit_grip(rig.local_pose(hb, 0), wrist_at(hv), hand_at(hv), segs_at(hv))
        report["grip_origin_in_hand_frame"] = np.round(o, 2).tolist()
        report["grip_tilt_deg"] = round(solver.report_tilt, 1)
        report["grip_spin_deg (spin, lean before, lean after)"] = getattr(solver, "report_spin", None)
    hold_clip = next(c for c in cfg["clips"] if c["video"][0] <= cfg["hold_video_frame"] <= c["video"][1])
    report["grip"] = solver.optimize_grip(rig.local_pose(rig.anim(hold_clip["base_anim"]), 0), wrist_at(hv), hand_at(hv), segs_at(hv))
    for clip in cfg["clips"]:
        base = rig.anim(clip["base_anim"])
        a, b = clip["video"]
        dur = clip["seconds"]
        n = int(round(dur * fps)) + 1
        rows, clamps = [], 0
        for i in range(n):
            u = i / (n - 1)
            fv = a + (b - a) * u - v0
            press = 0.0
            if "press" in clip:
                p0, p1 = clip["press"]
                press = float(np.sin(np.pi * np.clip((u - p0) / (p1 - p0), 0, 1)) ** 2)
            base_local = rig.local_pose(base, u * base["frames"])
            rots, poss, curls, clamped, P = solver.solve_frame(base_local, wrist_at(fv), hand_at(fv), segs_at(fv), press)
            rows.append((rots, poss, P))
            clamps += clamped
        bones = {}
        names = set(rig.animated(base)) | {j for js, _ in FINGERS.values() for j in js} | {"j_shoulder_ri", "j_elbow_ri", "j_wrist_ri", "j_wristtwist_ri", "tag_clip2"}
        tracks = {nm: ([], []) for nm in names}
        dbg = []
        for i, (rots, poss, P) in enumerate(rows):
            tr, tp = solver.phone_tag(rots, poss, P)
            j = rig.idx["tag_clip2"]
            rots[j], poss[j] = tr, tp
            for nm in names:
                k = rig.idx[nm]
                tracks[nm][0].append(rots[k].as_quat())
                tracks[nm][1].append(poss[k] - rig.anim_base[k])
            gr, gp = rig.fk(rots, poss)
            dbg.append({"pos": gp.tolist(), "phone_rot": P[0].tolist(), "phone_org": P[1].tolist()})
        for nm, (qs, ts) in tracks.items():
            qs = np.array(qs)
            for k in range(1, len(qs)):
                if qs[k] @ qs[k - 1] < 0:
                    qs[k] = -qs[k]
            ts = np.array(ts)
            const_q = np.abs(qs - qs[0]).max() < 1e-5
            const_t = np.abs(ts - ts[0]).max() < 1e-4
            bones[nm] = {
                "name": nm,
                "rot": [[round(float(c), 6) for c in q] for q in (qs[:1] if const_q else qs)],
                "trans": [[round(float(c), 4) for c in t] for t in (ts[:1] if const_t else ts)],
            }
        out = {
            "format": "cod4rw-mocap-clip",
            "version": 1,
            "name": clip["name"],
            "fps": fps,
            "frames": n,
            "space": "xanim locals: CoD axes (x forward, y left, z up), inches; rot = local quaternion x,y,z,w; "
                     "trans = local translation minus the joint's anim_base (as an xanim's). A track of one key is constant.",
            "source": {"video": cfg["video"], "frames": [a, b], "base_anim": clip["base_anim"], "rig": os.path.basename(cfg["rig"])},
            "bones": [bones[k] for k in sorted(bones)],
        }
        path = os.path.join(cfg["out_dir"], clip["name"] + ".json")
        json.dump(out, open(path, "w"), separators=(",", ":"))
        report["clips"][clip["name"]] = {"frames": n, "seconds": dur, "reach_clamped_frames": int(clamps),
                                         "bytes": os.path.getsize(path)}
        debug[clip["name"]] = dbg
    if "debug" in cfg:
        json.dump({"names": rig.names, "parent": rig.parent, "clips": debug, "phone_box": [x.tolist() for x in solver.phone_box()]},
                  open(cfg["debug"], "w"))
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
