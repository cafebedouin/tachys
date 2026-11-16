#!/usr/bin/env python3
"""Makes the app's launcher icons from img/tachys-logo.jpg (needs Pillow: pip install pillow).

The logo file is a picture of an icon on a wide backdrop, with the name under the emblem. A launcher icon is small
and the launcher writes the name itself, so only the winged sandal is taken. It is cut out of the middle of the
picture, where what surrounds it is the icon's own dark blue, and set on a square of that blue with its edges
faded in, so there is no seam.

  res/mipmap-*/ic_launcher_foreground.png   the emblem within the middle 66/108 of the square (Android 8+ masks the rest)
  res/mipmap-*/ic_launcher.png, _round.png  the same as a rounded square and a circle, for Android 7 (the Note 2)
  img/tachys-icon-512.png                   a full square with no rounding, as a store listing wants it
"""
import os
from PIL import Image, ImageDraw, ImageFilter

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RES = os.path.join(ROOT, "android/app/src/main/res")
src = Image.open(os.path.join(ROOT, "img/tachys-logo.jpg")).convert("RGB")

# the emblem: the part of the picture that holds the sandal and its wings, inside the icon's border and above the name
LEFT, TOP, RIGHT, BOTTOM = 905, 205, 1945, 1052
art = src.crop((LEFT, TOP, RIGHT, BOTTOM))
W, H = art.size
# the icon's own blue, from the darker edge of that part
edge = [art.getpixel(p) for p in [(6, 6), (W - 7, 6), (6, H // 2), (W - 7, H // 2), (6, H - 7), (W - 7, H - 7)]]
BLUE = tuple(sorted(c[i] for c in edge)[len(edge) // 2] for i in range(3))

def square(art_share):
    """The emblem in the middle of a square of blue, its width `art_share` of the side, its edges faded into the blue."""
    side = round(W / art_share)
    canvas = Image.new("RGB", (side, side), BLUE)
    fade = Image.new("L", (W, H), 0)
    # the lower edge is cut close (the name is just below), so it fades over less
    ImageDraw.Draw(fade).rectangle((90, 90, W - 91, H - 31), fill=255)
    fade = fade.filter(ImageFilter.GaussianBlur(28))
    ImageDraw.Draw(fade).rectangle((0, H - 3, W, H), fill=0)
    canvas.paste(art, ((side - W) // 2, (side - H) // 2), fade)
    return canvas

def masked(img, shape):
    big = img.size[0]
    mask = Image.new("L", img.size, 0)
    d = ImageDraw.Draw(mask)
    if shape == "circle": d.ellipse((0, 0, big - 1, big - 1), fill=255)
    else: d.rounded_rectangle((0, 0, big - 1, big - 1), radius=round(big * 0.18), fill=255)
    out = img.convert("RGBA"); out.putalpha(mask)
    return out

def save(img, path, px):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    img.resize((px, px), Image.LANCZOS).save(path, optimize=True)

foreground = square(0.74)   # the margins inside the cut bring the sandal itself within the 61% that is always shown
legacy = square(0.96)
for density, scale in [("mdpi", 1), ("hdpi", 1.5), ("xhdpi", 2), ("xxhdpi", 3), ("xxxhdpi", 4)]:
    d = os.path.join(RES, f"mipmap-{density}")
    save(foreground, os.path.join(d, "ic_launcher_foreground.png"), round(108 * scale))
    save(masked(legacy, "rounded"), os.path.join(d, "ic_launcher.png"), round(48 * scale))
    save(masked(legacy, "circle"), os.path.join(d, "ic_launcher_round.png"), round(48 * scale))
save(legacy, os.path.join(ROOT, "img/tachys-icon-512.png"), 512)

os.makedirs(os.path.join(RES, "mipmap-anydpi-v26"), exist_ok=True)
adaptive = '<?xml version="1.0" encoding="utf-8"?>\n<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">\n    <background android:drawable="@color/ic_launcher_background"/>\n    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>\n</adaptive-icon>\n'
for name in ["ic_launcher.xml", "ic_launcher_round.xml"]:
    open(os.path.join(RES, "mipmap-anydpi-v26", name), "w").write(adaptive)
os.makedirs(os.path.join(RES, "values"), exist_ok=True)
open(os.path.join(RES, "values/ic_launcher_background.xml"), "w").write(
    '<?xml version="1.0" encoding="utf-8"?>\n<resources>\n    <color name="ic_launcher_background">#%02X%02X%02X</color>\n</resources>\n' % BLUE)
print("icons written; background #%02X%02X%02X" % BLUE)
