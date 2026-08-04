# Extract a 4-row x 8-col walk-cycle sprite sheet from the generated green-screen image.
# 1) chroma-key green -> transparent (with despill)
# 2) slice into 4x8 cells (196x168 each)
# 3) trim each frame to its content bbox
# 4) re-pack into uniform cells (bottom-center aligned) for CSS background-position animation
from PIL import Image
import numpy as np

SRC = r"C:\Users\sitr2_tbayoh1\Downloads\generated-image-1 (7).png"
OUT = "public/assets/ui/monitor/footman_walk.png"
ROWS, COLS = 4, 8  # row 0=down, 1=up, 2=left, 3=right

img = Image.open(SRC).convert("RGB")
a = np.asarray(img).astype(np.int16)
H, W = a.shape[:2]
cw, ch = W // COLS, H // ROWS
assert W % COLS == 0 and H % ROWS == 0, f"unexpected size {W}x{H}"

r, g, b = a[..., 0], a[..., 1], a[..., 2]

# greenness: how much g dominates the other channels
dom = g - np.maximum(r, b)
# alpha: fully opaque where dom <= 20, fully clear where dom >= 80
alpha = np.clip((80 - dom) / 60.0, 0.0, 1.0)
# despill: clamp g near the max of r,b
g2 = np.minimum(g, np.maximum(r, b) + 40)
keyed = np.stack([r, g2, b, (alpha * 255).astype(np.int16)], axis=-1).astype(np.uint8)

# slice + trim each frame
frames = []
maxw = maxh = 0
for row in range(ROWS):
    for col in range(COLS):
        cell = keyed[row * ch:(row + 1) * ch, col * cw:(col + 1) * cw]
        m = cell[..., 3] > 128
        ys, xs = np.where(m)
        y0, y1, x0, x1 = ys.min(), ys.max() + 1, xs.min(), xs.max() + 1
        f = cell[y0:y1, x0:x1]
        frames.append(f)
        maxw = max(maxw, f.shape[1])
        maxh = max(maxh, f.shape[0])

# re-pack: bottom-center aligned so feet stay on the same baseline
pad = 4
tw, th = maxw + pad * 2, maxh + pad * 2
sheet = np.zeros((th * ROWS, tw * COLS, 4), dtype=np.uint8)
for i, f in enumerate(frames):
    row, col = divmod(i, COLS)
    fh, fw = f.shape[:2]
    y = row * th + (th - pad - fh)
    x = col * tw + (tw - fw) // 2
    sheet[y:y + fh, x:x + fw] = f

Image.fromarray(sheet, "RGBA").save(OUT)
print(f"saved {OUT}: sheet {sheet.shape[1]}x{sheet.shape[0]}, cell {tw}x{th}, {ROWS}x{COLS}")
