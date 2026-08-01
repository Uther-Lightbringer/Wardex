# Split the gold arrow out of the dropdown bar:
#  - bar without arrow (arrow region filled by horizontal mirror of the clean left side)
#  - arrow as its own transparent PNG
#  - recompute slices (bar now arrow-free: plain corner slices)
from PIL import Image, ImageFilter
import numpy as np
import json, os

bar = Image.open("tools/out/bar_keyed.png").convert("RGBA")  # 884x187
a = np.asarray(bar).astype(np.int16)
r, g, b, al = a[...,0], a[...,1], a[...,2], a[...,3]

gold = (al > 200) & (r > 150) & (g > 100) & (b < 90) & (r > b + 80)
ys, xs = np.where(gold)
ax0, ax1, ay0, ay1 = int(xs.min()), int(xs.max()), int(ys.min()), int(ys.max())
print("arrow bbox:", ax0, ax1, ay0, ay1)

# --- arrow PNG: alpha = dilated gold mask (keeps the dark outline) ---
mask = np.zeros(gold.shape, np.uint8)
mask[gold] = 255
mimg = Image.fromarray(mask).filter(ImageFilter.MaxFilter(7)).filter(ImageFilter.GaussianBlur(1.2))
arrow_alpha = np.asarray(mimg)
m = 8
crop = (max(ax0-m,0), max(ay0-m,0), min(ax1+m, bar.width), min(ay1+m, bar.height))
arrow = np.asarray(bar.crop(crop)).copy()
arrow[...,3] = np.minimum(arrow[...,3], arrow_alpha[crop[1]:crop[3], crop[0]:crop[2]])
S = 64 / bar.height
arrow_img = Image.fromarray(arrow, "RGBA")
arrow_img = arrow_img.resize((round(arrow_img.width*S), round(arrow_img.height*S)), Image.LANCZOS)
arrow_img.save("tools/out/dropdown_arrow.png")
print("arrow final:", arrow_img.size)

# --- erase arrow from bar: mirror the clean left half onto the arrow region ---
out = np.asarray(bar).copy()
w = bar.width
x0, x1 = crop[0], crop[2]
for x in range(x0, x1):
    src = w - 1 - x
    out[:, x, :3] = np.asarray(bar)[:, src, :3]
bar_clean = Image.fromarray(out, "RGBA")
bar_s = bar_clean.resize((round(w*S), 64), Image.LANCZOS)
bar_s.save("tools/out/dropdown_bar.png")

# slices: corners only (no arrow on the right any more)
def corner_run(img):
    aa = np.asarray(img)[..., 3]
    h, wd = aa.shape
    top = aa[0] > 128
    l = int(np.argmax(top)); rr = wd - int(np.argmax(top[::-1]))
    col = aa[:, 0] > 128
    t = int(np.argmax(col)); bb = h - int(np.argmax(col[::-1]))
    return l, wd-rr, t, h-bb
bl, br, bt, bb = corner_run(bar_clean)
sl = {"top": round(bt*S)+6, "right": round(br*S)+6, "bottom": round(bb*S)+6, "left": round(bl*S)+6}
print("bar slices:", sl)
json.dump({"bar": sl}, open("tools/out/slices_bar.json", "w"))

# --- verify: nine-slice sim + arrow overlay ---
def nine_slice(img, s, ow, oh, bw):
    t, rr, bb2, l = s["top"], s["right"], s["bottom"], s["left"]
    wt, wr, wb, wl = bw
    iw, ih = img.size
    o = Image.new("RGBA", (ow, oh))
    def piece(box, size):
        p = img.crop(box)
        if p.size != size and size[0] > 0 and size[1] > 0:
            p = p.resize(size, Image.LANCZOS)
        return p
    o.paste(piece((0,0,l,t), (wl,wt)), (0,0))
    o.paste(piece((iw-rr,0,iw,t), (wr,wt)), (ow-wr,0))
    o.paste(piece((0,ih-bb2,l,ih), (wl,wb)), (0,oh-wb))
    o.paste(piece((iw-rr,ih-bb2,iw,ih), (wr,wb)), (ow-wr,oh-wb))
    o.paste(piece((l,0,iw-rr,t), (ow-wl-wr,wt)), (wl,0))
    o.paste(piece((l,ih-bb2,iw-rr,ih), (ow-wl-wr,wb)), (wl,oh-wb))
    o.paste(piece((0,t,l,ih-bb2), (wl,oh-wt-wb)), (0,wt))
    o.paste(piece((iw-rr,t,iw,ih-bb2), (wr,oh-wt-wb)), (ow-wr,wt))
    o.paste(piece((l,t,iw-rr,ih-bb2), (max(ow-wl-wr,1),max(oh-wt-wb,1))), (wl,wt))
    return o

bg = Image.new("RGBA", (300, 120), (24,24,32,255))
for i, (wd, y) in enumerate([(180, 10), (280, 55)]):
    sim = nine_slice(bar_s, sl, wd, 30, (12,14,12,13))
    bg.paste(sim, (10, y), sim)
    ah = 12
    aw = round(arrow_img.width * ah / arrow_img.height)
    ar = arrow_img.resize((aw, ah), Image.LANCZOS)
    bg.paste(ar, (10 + wd - aw - 10, y + (30-ah)//2), ar)
bg.save("tools/out/sim3.png")
print("ok")
