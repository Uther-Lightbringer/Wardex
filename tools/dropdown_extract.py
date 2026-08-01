# Extract dropdown bar + panel from the generated green-screen image.
# 1) chroma-key green -> transparent (with despill)
# 2) split the two components (top = bar, bottom = panel) via row projection
# 3) crop to content bbox, save keyed PNGs for inspection
from PIL import Image
import numpy as np

SRC = r"C:\Users\sitr2_tbayoh1\Downloads\generated-image-1 (4).png"
OUT_BAR = "tools/out/bar_keyed.png"
OUT_PANEL = "tools/out/panel_keyed.png"

img = Image.open(SRC).convert("RGB")
a = np.asarray(img).astype(np.int16)
r, g, b = a[..., 0], a[..., 1], a[..., 2]

# greenness: how much g dominates the other channels
dom = g - np.maximum(r, b)
# alpha: fully opaque where dom <= 20, fully clear where dom >= 80
alpha = np.clip((80 - dom) / 60.0, 0.0, 1.0)

# despill: clamp g to the max of r,b where keyed partially
g2 = np.minimum(g, np.maximum(r, b) + 40)
out = np.stack([r, g2, b, (alpha * 255).astype(np.int16)], axis=-1).astype(np.uint8)

mask = alpha > 0.5
rows = mask.any(axis=1)
# find row ranges of content
ranges = []
in_run = False
for i, v in enumerate(rows):
    if v and not in_run:
        start = i; in_run = True
    elif not v and in_run:
        ranges.append((start, i)); in_run = False
if in_run:
    ranges.append((start, len(rows)))
# merge ranges separated by tiny gaps (<10px)
merged = []
for s, e in ranges:
    if merged and s - merged[-1][1] < 10:
        merged[-1] = (merged[-1][0], e)
    else:
        merged.append((s, e))
print("row ranges:", merged)
assert len(merged) >= 2, "expected 2 components"

def crop(row_range, path):
    s, e = row_range
    sub = mask[s:e]
    cols = sub.any(axis=0)
    c0, c1 = np.argmax(cols), len(cols) - np.argmax(cols[::-1])
    tile = out[s:e, c0:c1]
    Image.fromarray(tile, "RGBA").save(path)
    print(path, tile.shape[1], "x", tile.shape[0])

crop(merged[0], OUT_BAR)
crop(merged[1], OUT_PANEL)
