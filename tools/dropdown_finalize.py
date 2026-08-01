# Finalize dropdown assets: scale, compute nine-slice insets, simulate render.
from PIL import Image
import numpy as np
import os, json, shutil

os.makedirs("tools/out", exist_ok=True)

bar = Image.open("tools/out/bar_keyed.png")     # 884x187
panel = Image.open("tools/out/panel_keyed.png") # 636x377

# --- locate gold arrow in bar (gold: r high, g mid, b low) ---
a = np.asarray(bar).astype(np.int16)
r, g, b, al = a[...,0], a[...,1], a[...,2], a[...,3]
gold = (al > 200) & (r > 150) & (g > 100) & (b < 90) & (r > b + 80)
ys, xs = np.where(gold)
ax0, ax1, ay0, ay1 = xs.min(), xs.max(), ys.min(), ys.max()
print("arrow bbox:", ax0, ax1, ay0, ay1)

# --- corner extents: scan top row alpha to find where the top edge starts/ends
def corner_run(img):
    al = np.asarray(img)[..., 3]
    h, w = al.shape
    top = al[0] > 128
    left = np.argmax(top)              # first opaque px on top row = left corner width
    right = w - np.argmax(top[::-1])   # right corner width
    col = al[:, 0] > 128
    topc = np.argmax(col)
    botc = h - np.argmax(col[::-1])
    return left, w - right, topc, h - botc

bl, br, bt, bb = corner_run(bar)
pl, pr, pt, pb = corner_run(panel)
print("bar corners L/R/T/B:", bl, br, bt, bb)
print("panel corners L/R/T/B:", pl, pr, pt, pb)

# --- scale: bar to height 64 (displayed at 32), panel same factor
S = 64 / bar.height
bar_s = bar.resize((round(bar.width*S), 64), Image.LANCZOS)
PS = 128 / panel.height
panel_s = panel.resize((round(panel.width*PS), 128), Image.LANCZOS)
print("bar final:", bar_s.size, "panel final:", panel_s.size)

# --- slices in final-image px ---
def s(v, f): return int(round(v*f))
bar_slices = {
    "top": s(bt, S) + 6,
    "right": s(bar.width - ax0 + 10, S),   # include whole arrow + margin
    "bottom": s(bb, S) + 6,
    "left": s(bl, S) + 6,
}
panel_slices = {
    "top": s(pt, PS) + 6,
    "right": s(pr, PS) + 6,
    "bottom": s(pb, PS) + 6,
    "left": s(pl, PS) + 6,
}
print("bar slices:", bar_slices)
print("panel slices:", panel_slices)

bar_s.save("tools/out/dropdown_bar.png")
panel_s.save("tools/out/dropdown_panel.png")

# --- nine-slice simulation at display size ---
def nine_slice(img, sl, out_w, out_h, bw):
    # bw = border-width (T R B L) in output px
    t, r, b, l = sl["top"], sl["right"], sl["bottom"], sl["left"]
    wt, wr, wb, wl = bw
    w, h = img.size
    out = Image.new("RGBA", (out_w, out_h))
    def piece(box, size):
        p = img.crop(box)
        if p.size != size and size[0] > 0 and size[1] > 0:
            p = p.resize(size, Image.LANCZOS)
        return p
    # corners
    out.paste(piece((0,0,l,t), (wl,wt)), (0,0))
    out.paste(piece((w-r,0,w,t), (wr,wt)), (out_w-wr,0))
    out.paste(piece((0,h-b,l,h), (wl,wb)), (0,out_h-wb))
    out.paste(piece((w-r,h-b,w,h), (wr,wb)), (out_w-wr,out_h-wb))
    # edges
    out.paste(piece((l,0,w-r,t), (out_w-wl-wr,wt)), (wl,0))
    out.paste(piece((l,h-b,w-r,h), (out_w-wl-wr,wb)), (wl,out_h-wb))
    out.paste(piece((0,t,l,h-b), (wl,out_h-wt-wb)), (0,wt))
    out.paste(piece((w-r,t,w,h-b), (wr,out_h-wt-wb)), (out_w-wr,wt))
    # center
    out.paste(piece((l,t,w-r,h-b), (out_w-wl-wr,out_h-wt-wb)), (wl,wt))
    return out

sim_bar = nine_slice(bar_s, bar_slices, 300, 64, (12,12,12,12))
sim_panel = nine_slice(panel_s, panel_slices, 300, 200, (14,14,14,14))
bg = Image.new("RGBA", (320, 320), (24,24,32,255))
bg.paste(sim_bar, (10,10), sim_bar)
bg.paste(sim_panel, (10,100), sim_panel)
bg.save("tools/out/sim.png")

with open("tools/out/slices.json", "w") as f:
    json.dump({"bar": bar_slices, "panel": panel_slices,
               "bar_size": bar_s.size, "panel_size": panel_s.size}, f, indent=2)
