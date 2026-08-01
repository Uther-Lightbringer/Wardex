# Crop all three button states with ONE shared rect so they align pixel-perfect
# when swapped (avoids hover/press size jitter from object-fit: contain).
from PIL import Image

RECT = (95, 18, 533, 90)  # tight: blue band + glow rim, dark frame excluded
for state in ["normal", "hover", "pressed"]:
    im = Image.open(f"public/assets/ui/buttons/btn_{state}.png").convert("RGBA")
    crop = im.crop(RECT)
    crop.save(f"tools/out/btn_blue_{state}.png")
    print(state, crop.size, "aspect", round(crop.width / crop.height, 2))
