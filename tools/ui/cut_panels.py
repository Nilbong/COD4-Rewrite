"""Cut the user's green panel sheets (assets/ui/greenicons*.png) into
transparent pieces in assets/ui/next/panels/.

solid: the piece's background (connected to the crop's edge, darker than
its outline) keyed out; lines: only the bright outline and glow kept."""
import sys
import numpy as np
from PIL import Image
from scipy import ndimage

UI = sys.argv[1]
PIECES = {
    # name: (sheet, x0, y0, x1, y1, mode)
    "row_focus": ("greenicons2.png", 36, 166, 486, 254, "solid"),
    "row_plain": ("greenicons2.png", 36, 272, 486, 362, "solid"),
    "card_sas": ("greenicons2.png", 1048, 546, 1504, 690, "solid"),
    "card_map": ("greenicons2.png", 1046, 708, 1504, 824, "solid"),
    "card_world": ("greenicons.png", 1026, 784, 1500, 926, "solid"),
    "card_grid": ("greenicons2.png", 518, 398, 1020, 498, "solid"),
    "divider": ("greenicons2.png", 1050, 204, 1404, 232, "lines"),
    "box_plus": ("greenicons2.png", 1050, 76, 1162, 184, "solid"),
    "box_brackets": ("greenicons2.png", 1183, 76, 1292, 184, "solid"),
    "frame_big": ("greenicons.png", 551, 286, 1120, 542, "lines"),
    "panel_big": ("greenicons.png", 551, 286, 1120, 542, "solid"),
    "row_accent": ("greenicons.png", 1162, 34, 1498, 108, "solid"),
    "footer_tab": ("greenicons.png", 1034, 938, 1492, 990, "solid"),
    "progress": ("greenicons.png", 1060, 730, 1380, 766, "solid"),
    "header": ("greenicons.png", 551, 34, 1114, 184, "solid"),
    "reticle": ("greenicons.png", 1320, 564, 1468, 724, "lines"),
    "btn_chevron": ("greenicons.png", 608, 860, 1004, 917, "solid"),
    "row_tick": ("greenicons.png", 1164, 36, 1494, 106, "solid"),
    "row_glow": ("greenicons2.png", 36, 166, 486, 250, "solid"),
    "box_oct": ("greenicons.png", 610, 574, 746, 702, "solid"),
    "box_square": ("greenicons.png", 80, 576, 210, 702, "solid"),
}

def lum(a):
    return a[..., 0] * 0.2126 + a[..., 1] * 0.7152 + a[..., 2] * 0.0722

for name, (sheet, x0, y0, x1, y1, mode) in PIECES.items():
    src = np.array(Image.open(f"{UI}/{sheet}").convert("RGBA"), dtype=np.float32)[y0:y1, x0:x1]
    a, alpha = src[..., :3], src[..., 3] / 255.0
    if alpha.min() > 0.9:
        # An old sheet without transparency: key its background out.
        l = lum(a)
        edge = np.concatenate([l[0], l[-1], l[:, 0], l[:, -1]])
        dark = l < np.percentile(edge, 60) + 18
        lab, _ = ndimage.label(dark)
        border = set(np.unique(np.concatenate([lab[0], lab[-1], lab[:, 0], lab[:, -1]]))) - {0}
        alpha = ndimage.gaussian_filter((~np.isin(lab, list(border))).astype(np.float32), 0.7)
    if mode == "lines":
        # Only the frame: its edge band and corner brackets, never the
        # panel inside (it would cover whatever the frame holds).
        h, w = alpha.shape
        yy, xx = np.mgrid[0:h, 0:w]
        edge = np.minimum(np.minimum(xx, w - 1 - xx), np.minimum(yy, h - 1 - yy))
        corner = (np.minimum(xx, w - 1 - xx) < 70) & (np.minimum(yy, h - 1 - yy) < 70)
        bright = np.clip((lum(a) - 40) / 60, 0, 1)
        alpha = alpha * np.where(edge < 10, 1.0, np.where(corner, bright, 0.0))
    out = np.dstack([a, alpha * 255]).astype(np.uint8)
    Image.fromarray(out, "RGBA").save(f"{UI}/next/panels/{name}.png", optimize=True)
    print(name, out.shape[1], out.shape[0])
