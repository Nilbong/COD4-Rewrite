"""Train the aim model on aim_data.py's samples and check it against the
real player on sightings it never saw.

The model: a small MLP from the inputs (aim_data.FEATURES) to a Gaussian
over the view's angular velocity for the next step (mean and log standard
deviation, yaw and pitch), so a bot aims with the player's own spread.
Noise is drawn once per NOISE_HOLD seconds at play time (a hand's errors
aren't independent from frame to frame).

The check replays each held-out sighting's target motion and lets the
model aim at it from the player's starting view, closed loop, comparing
with what the player did: time to get within 2 degrees, overshoot past the
target, and tracking error once settled.

Usage: python train_aim.py aim.npz -o aim_model.json
"""

import argparse
import json

import numpy as np
import torch
from torch import nn

HIDDEN = 64
NOISY = True
NOISE_HOLD = 0.05
SETTLE_DEG = 2.0


class Net(nn.Module):
    def __init__(self, n_in):
        super().__init__()
        self.body = nn.Sequential(nn.Linear(n_in, HIDDEN), nn.Tanh(), nn.Linear(HIDDEN, HIDDEN), nn.Tanh(), nn.Linear(HIDDEN, 4))

    def forward(self, x):
        o = self.body(x)
        return o[:, :2], o[:, 2:].clamp(-4.0, 3.0)


def split(groups, frac, seed):
    ids = np.unique(groups)
    rng = np.random.default_rng(seed)
    rng.shuffle(ids)
    test = set(ids[: max(1, int(len(ids) * frac))])
    return np.array([g in test for g in groups]), sorted(test)


def train(X, Y, epochs, seed):
    torch.manual_seed(seed)
    xm, xs = X.mean(0), X.std(0) + 1e-6
    ym, ys = Y.mean(0), Y.std(0) + 1e-6
    xt = torch.tensor((X - xm) / xs)
    yt = torch.tensor((Y - ym) / ys)
    net = Net(X.shape[1])
    opt = torch.optim.Adam(net.parameters(), lr=3e-3, weight_decay=1e-4)
    # Input noise on the error and the view's own velocity, so the model
    # learns to recover from states its own mistakes lead to (closed loop).
    jitter = torch.zeros(X.shape[1])
    jitter[0:2] = torch.tensor(1.0 / xs[0:2])
    jitter[4:6] = torch.tensor(15.0 / xs[4:6])
    for ep in range(epochs):
        perm = torch.randperm(len(xt))
        for i in range(0, len(xt), 256):
            b = perm[i : i + 256]
            xb = xt[b] + torch.randn(len(b), X.shape[1]) * jitter if NOISY else xt[b]
            mu, ls = net(xb)
            loss = (0.5 * ((yt[b] - mu) / ls.exp()) ** 2 + ls).mean()
            opt.zero_grad()
            loss.backward()
            opt.step()
    return net, (xm, xs, ym, ys)


def predict(net, norm, x):
    xm, xs, ym, ys = norm
    with torch.no_grad():
        mu, ls = net(torch.tensor(((x - xm) / xs)[None].astype(np.float32)))
    return mu[0].numpy() * ys + ym, ls[0].exp().numpy() * ys


def metrics(err):
    """Settle time (s), overshoot (deg) and settled RMS error (deg) of a yaw/pitch error track."""
    mag = np.hypot(err[:, 0], err[:, 1])
    inside = np.nonzero(mag < SETTLE_DEG)[0]
    settle = inside[0] * DT if len(inside) else np.nan
    # Overshoot: past the target along the first error's direction.
    d0 = err[0] / (np.linalg.norm(err[0]) + 1e-9)
    along = err @ d0
    over = max(0.0, -along.min()) if np.linalg.norm(err[0]) > 4 else np.nan
    rms = np.sqrt((mag[inside[0]:] ** 2).mean()) if len(inside) else np.nan
    return settle, over, rms


def rollout(net, norm, X, Y, rng):
    """The model aiming at a held-out sighting's target, from the player's start."""
    n = len(X)
    tgt = np.cumsum(np.vstack([[0, 0], X[:-1, 2:4] * DT]), 0) + X[0, 0:2]  # target relative to start view
    view = np.zeros(2)
    prev_v = X[0, 4:6].copy()
    errs, noise, hold = [], np.zeros(2), 0.0
    for i in range(n):
        x = X[i].copy()
        x[0:2] = tgt[i] - view
        x[4:6] = prev_v
        mu, sd = predict(net, norm, x)
        if hold <= 0:
            noise, hold = rng.standard_normal(2), NOISE_HOLD
        hold -= DT
        v = mu + sd * noise
        errs.append(tgt[i] - view)
        view = view + v * DT
        prev_v = v
    return np.array(errs)


def human(X):
    return X[:, 0:2]


def summary(rows):
    a = np.array(rows, float)
    return {k: float(np.nanmedian(a[:, i])) for i, k in enumerate(["settle_s", "overshoot_deg", "rms_deg"])}


def main():
    global DT
    ap = argparse.ArgumentParser()
    ap.add_argument("data")
    ap.add_argument("-o", "--out", default="aim_model.json")
    ap.add_argument("--epochs", type=int, default=300)
    ap.add_argument("--seed", type=int, default=1)
    a = ap.parse_args()
    d = np.load(a.data)
    DT = float(d["dt"])
    X, Y, R, G = d["X"], d["Y"], d["recoil"], d["sighting"]
    test, test_ids = split(G, 0.25, a.seed)
    tr = ~test & ~R
    net, norm = train(X[tr], Y[tr], a.epochs, a.seed)
    rng = np.random.default_rng(a.seed)
    model_rows, human_rows = [], []
    for g in test_ids:
        m = G == g
        # Only the start of each sighting, before the first shot's recoil.
        first_recoil = np.argmax(R[m]) if R[m].any() else m.sum()
        if first_recoil < 6:
            continue
        Xg, Yg = X[m][:first_recoil], Y[m][:first_recoil]
        model_rows.append(metrics(rollout(net, norm, Xg, Yg, rng)))
        human_rows.append(metrics(human(Xg)))
    print(f"held-out sightings: {len(human_rows)}")
    print("player:", summary(human_rows))
    print("model: ", summary(model_rows))
    xm, xs, ym, ys = norm
    layers = [m for m in net.body if isinstance(m, nn.Linear)]
    json.dump(
        {
            "features": [str(f) for f in d["features"]],
            "dt": DT,
            "noise_hold": NOISE_HOLD,
            "x_mean": xm.tolist(),
            "x_std": xs.tolist(),
            "y_mean": ym.tolist(),
            "y_std": ys.tolist(),
            "layers": [{"w": l.weight.detach().numpy().tolist(), "b": l.bias.detach().numpy().tolist()} for l in layers],
            "activation": "tanh",
        },
        open(a.out, "w"),
    )
    print("wrote", a.out)


if __name__ == "__main__":
    main()
