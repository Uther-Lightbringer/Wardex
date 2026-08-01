from PIL import Image
import json

sl = json.load(open("tools/out/slices.json"))
bar_s = Image.open("tools/out/dropdown_bar.png")
panel_s = Image.open("tools/out/dropdown_panel.png")

def nine_slice(img, sl, out_w, out_h, bw):
    t, r, b, l = sl["top"], sl["right"], sl["bottom"], sl["left"]
    wt, wr, wb, wl = bw
    w, h = img.size
    out = Image.new("RGBA", (out_w, out_h))
    def piece(box, size):
        p = img.crop(box)
        if p.size != size and size[0] > 0 and size[1] > 0:
            p = p.resize(size, Image.LANCZOS)
        return p
    out.paste(piece((0,0,l,t), (wl,wt)), (0,0))
    out.paste(piece((w-r,0,w,t), (wr,wt)), (out_w-wr,0))
    out.paste(piece((0,h-b,l,h), (wl,wb)), (0,out_h-wb))
    out.paste(piece((w-r,h-b,w,h), (wr,wb)), (out_w-wr,out_h-wb))
    out.paste(piece((l,0,w-r,t), (out_w-wl-wr,wt)), (wl,0))
    out.paste(piece((l,h-b,w-r,h), (out_w-wl-wr,wb)), (wl,out_h-wb))
    out.paste(piece((0,t,l,h-b), (wl,out_h-wt-wb)), (0,wt))
    out.paste(piece((w-r,t,w,h-b), (wr,out_h-wt-wb)), (out_w-wr,wt))
    out.paste(piece((l,t,w-r,h-b), (max(out_w-wl-wr,1),max(out_h-wt-wb,1))), (wl,wt))
    return out

sim_bar = nine_slice(bar_s, sl["bar"], 280, 32, (12,38,12,13))
sim_bar2 = nine_slice(bar_s, sl["bar"], 180, 32, (12,38,12,13))
sim_panel = nine_slice(panel_s, sl["panel"], 280, 160, (13,14,12,14))
bg = Image.new("RGBA", (300, 300), (24,24,32,255))
bg.paste(sim_bar, (10,10), sim_bar)
bg.paste(sim_bar2, (10,52), sim_bar2)
bg.paste(sim_panel, (10,94), sim_panel)
bg.save("tools/out/sim2.png")
print("ok")
