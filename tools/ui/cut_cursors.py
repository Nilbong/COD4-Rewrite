"""Cut the user's cursor pack (assets/ui/cursorpack.png) into
assets/ui/next/cursors/. Each piece is the glyph's alpha bounds (plus its
glow) inside a rough box; prints its size and the hotspot inside it."""
import sys
import numpy as np
from PIL import Image

UI = sys.argv[1]
# name: rough box (x0, y0, x1, y1), hotspot in sheet pixels.
PIECES = {
    "arrow": ((95, 130, 275, 360), (132, 158)),
    "arrow_select": ((380, 125, 585, 350), (430, 160)),
    "hand": ((80, 405, 245, 615), (150, 432)),
    "busy": ((385, 680, 585, 880), (410, 702)),
    "move": ((555, 415, 750, 615), (655, 513)),
    "text": ((1290, 690, 1385, 860), (1337, 775)),
}
sheet = np.array(Image.open(f"{UI}/cursorpack.png").convert("RGBA"))
for name, ((x0, y0, x1, y1), (hx, hy)) in PIECES.items():
    a = sheet[y0:y1, x0:x1]
    ys, xs = np.nonzero(a[..., 3] > 8)
    bx0, by0, bx1, by1 = xs.min(), ys.min(), xs.max() + 1, ys.max() + 1
    piece = a[by0:by1, bx0:bx1]
    Image.fromarray(piece).save(f"{UI}/next/cursors/{name}.png", optimize=True)
    print(f'"{name}": ({bx1 - bx0}, {by1 - by0}), hotspot ({hx - x0 - bx0}, {hy - y0 - by0})')
