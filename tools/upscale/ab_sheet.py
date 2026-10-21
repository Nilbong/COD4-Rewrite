"""Before/after sheet from two COD4RW_SHOT_VIEWS runs (same views):

    python -I tools/upscale/ab_sheet.py <original dir> <upscaled dir> <out.png> <label,label,...>

Each row: the original and the remastered view side by side, then a 2x
close-up of the middle of each.
"""

import os
import sys

from PIL import Image, ImageDraw


def main():
    a_dir, b_dir, out, labels = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4].split(",")
    rows = []
    for i, label in enumerate(labels):
        pa, pb = os.path.join(a_dir, f"view{i}.png"), os.path.join(b_dir, f"view{i}.png")
        if not (os.path.exists(pa) and os.path.exists(pb)):
            continue
        a, b = Image.open(pa).convert("RGB"), Image.open(pb).convert("RGB")
        w, h = a.size
        box = (w * 3 // 8, h * 3 // 8, w * 5 // 8, h * 5 // 8)
        za, zb = a.crop(box).resize((w, h), Image.NEAREST), b.crop(box).resize((w, h), Image.NEAREST)
        row = Image.new("RGB", (w * 4 + 30, h + 28), (16, 16, 18))
        for k, im in enumerate([a, b, za, zb]):
            row.paste(im, (k * (w + 10), 28))
        d = ImageDraw.Draw(row)
        for k, t in enumerate(["original", "remastered", "original (4x crop)", "remastered (4x crop)"]):
            d.text((k * (w + 10) + 6, 8), f"{label}: {t}", fill=(235, 235, 235))
        rows.append(row)
    sheet = Image.new("RGB", (rows[0].width, sum(r.height for r in rows)), (16, 16, 18))
    y = 0
    for r in rows:
        sheet.paste(r, (0, y))
        y += r.height
    os.makedirs(os.path.dirname(out), exist_ok=True)
    sheet.save(out)
    print(out, sheet.size)


if __name__ == "__main__":
    main()
