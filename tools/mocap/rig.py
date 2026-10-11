"""CoD4 viewmodel rig: the viewhands' and a gun's skeletons from mocapref's
dump, sampled anims and forward kinematics, the way crates/game does it
(models.rs `sample`): a bone a track names takes the track's rotation (else
its bind rotation) and its `anim_base` plus the track's translation; a bone
no track names keeps its bind pose. CoD axes (x forward, y left, z up),
inches, quaternions x, y, z, w (scipy's order).
"""
import json

import numpy as np
from scipy.spatial.transform import Rotation as R


class Rig:
    def __init__(self, path):
        d = json.load(open(path))
        self.names, self.parent, self.anim_base = [], [], []
        g_rot, g_pos = [], []
        for model, offset in ((d["hands"], 0), (d["gun"], None)):
            base = len(self.names)
            for b in model["bones"]:
                self.names.append(b["name"])
                p = b["parent"]
                if p < 0:
                    # The gun hangs from the hands' tag_weapon.
                    p = -1 if offset == 0 else self.names.index("tag_weapon")
                else:
                    p += base
                self.parent.append(p)
                g_rot.append(R.from_quat(b["q"]))
                g_pos.append(np.array(b["t"], float))
                self.anim_base.append(np.array(b["anim_base"], float))
        self.idx = {n: i for i, n in enumerate(self.names)}
        # Bind locals from the model-space bind matrices (content.rs
        # prepare_model); the gun's root keeps its own (identity).
        self.bind_rot, self.bind_pos = [], []
        hands_n = len(d["hands"]["bones"])
        for i, p in enumerate(self.parent):
            if p < 0 or (i >= hands_n and self.names[p] == "tag_weapon" and i == hands_n):
                self.bind_rot.append(g_rot[i])
                self.bind_pos.append(g_pos[i])
            else:
                self.bind_rot.append(g_rot[p].inv() * g_rot[i])
                self.bind_pos.append(g_rot[p].inv().apply(g_pos[i] - g_pos[p]))
        self.anims = {a["name"].lower(): a for a in d["anims"]}

    def anim(self, name):
        return self.anims[name.lower()]

    def local_pose(self, anim=None, frame=0.0):
        """Local (rotation, translation) per bone of `anim` at `frame`."""
        rots = list(self.bind_rot)
        poss = [p.copy() for p in self.bind_pos]
        if anim is None:
            return rots, poss
        n = anim["frames"]
        f = float(np.clip(frame, 0, n))
        i0 = int(np.floor(f))
        i1 = min(i0 + 1, n)
        t = f - i0
        for b in anim["bones"]:
            j = self.idx.get(b["name"])
            if j is None:
                continue
            r0, r1 = b["rot"][i0], b["rot"][i1]
            if r0 is not None:
                q0, q1 = np.array(r0), np.array(r1)
                if q0 @ q1 < 0:
                    q1 = -q1
                q = q0 + (q1 - q0) * t
                rots[j] = R.from_quat(q / np.linalg.norm(q))
            tr0, tr1 = b["trans"][i0], b["trans"][i1]
            d = np.zeros(3) if tr0 is None else np.array(tr0) + (np.array(tr1) - np.array(tr0)) * t
            poss[j] = self.anim_base[j] + d
        return rots, poss

    def animated(self, anim):
        """Names of the bones `anim` has tracks for."""
        return [b["name"] for b in anim["bones"] if b["name"] in self.idx]

    def fk(self, rots, poss):
        """Global (model space) rotations and positions from locals."""
        n = len(self.names)
        gr, gp = [None] * n, [None] * n
        for i in range(n):
            p = self.parent[i]
            if p < 0:
                gr[i], gp[i] = rots[i], poss[i].copy()
            else:
                gr[i] = gr[p] * rots[i]
                gp[i] = gp[p] + gr[p].apply(poss[i])
        return gr, np.array(gp)

    def children(self, i):
        return [k for k, p in enumerate(self.parent) if p == i]
