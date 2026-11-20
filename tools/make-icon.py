# Draws String's icon: a plucked string between two pegs on a dark rounded square.
# Each size is drawn on its own (8x, then shrunk) so small sizes get thicker lines instead of a blur.
#   python tools\make-icon.py      -> src\Assets\String.ico and src\Assets\String.png
import os
from PIL import Image, ImageDraw, ImageFilter

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, '..', 'src', 'Assets')
SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]


def bezier(p0, p1, p2, n=160):
    pts = []
    for i in range(n + 1):
        t = i / n
        x = (1 - t) ** 2 * p0[0] + 2 * (1 - t) * t * p1[0] + t * t * p2[0]
        y = (1 - t) ** 2 * p0[1] + 2 * (1 - t) * t * p1[1] + t * t * p2[1]
        pts.append((x, y))
    return pts


def stroke(d, pts, width, fill):
    # a smooth thick curve: round dabs every half pixel (ImageDraw.line leaves notches at the joins)
    r = width / 2
    for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
        n = max(1, int(((x1 - x0) ** 2 + (y1 - y0) ** 2) ** .5 * 2))
        for i in range(n):
            x, y = x0 + (x1 - x0) * i / n, y0 + (y1 - y0) * i / n
            d.ellipse([x - r, y - r, x + r, y + r], fill=fill)


def draw(size):
    k = 8
    S = size * k
    u = S / 1024  # design units: a 1024 square
    small = size <= 32
    img = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    inset = 40 * u if not small else 8 * u
    d.rounded_rectangle([inset, inset, S - inset, S - inset], radius=(210 if not small else 170) * u, fill=(27, 27, 29, 255))

    a, b = (250 * u, 720 * u), (774 * u, 304 * u)        # the pegs, low left to high right
    mid = ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2)
    # bow at right angles to the string
    nx, ny = (b[1] - a[1]), -(b[0] - a[0])
    ln = (nx * nx + ny * ny) ** .5
    nx, ny = nx / ln, ny / ln
    bow = 150 * u
    wire = (70 if small else 34) * u
    ghost_w = (44 if small else 18) * u

    # the ghost of the last swing, faint, on the other side
    if not small or size >= 24:
        g = Image.new('RGBA', (S, S), (0, 0, 0, 0))
        gd = ImageDraw.Draw(g)
        gc = (mid[0] - nx * bow * .8, mid[1] - ny * bow * .8)
        stroke(gd, bezier(a, gc, b), ghost_w, (255, 255, 255, 255))
        g.putalpha(g.getchannel('A').point(lambda v: v * 80 // 255))
        img.alpha_composite(g)
    # the string, with a soft glow
    glow = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    gl = ImageDraw.Draw(glow)
    c = (mid[0] + nx * bow, mid[1] + ny * bow)
    stroke(gl, bezier(a, c, b, 60), wire * 2.4, (255, 255, 255, 255))
    glow.putalpha(glow.getchannel('A').point(lambda v: v * 120 // 255))
    glow = glow.filter(ImageFilter.GaussianBlur(26 * u if not small else 18 * u))
    img.alpha_composite(glow)
    d = ImageDraw.Draw(img)
    stroke(d, bezier(a, c, b), wire, (244, 244, 246, 255))
    # the pegs: rings on the dark tile
    pr = (66 if not small else 96) * u
    ring = (26 if not small else 52) * u
    for p in (a, b):
        d.ellipse([p[0] - pr, p[1] - pr, p[0] + pr, p[1] + pr], fill=(27, 27, 29, 255), outline=(244, 244, 246, 255), width=int(ring))
    return img.resize((size, size), Image.LANCZOS)


if __name__ == '__main__':
    os.makedirs(OUT, exist_ok=True)
    imgs = [draw(s) for s in SIZES]
    big = imgs[-1]
    big.save(os.path.join(OUT, 'String.png'))
    big.save(os.path.join(OUT, 'String.ico'), sizes=[(s, s) for s in SIZES], append_images=imgs[:-1])
    print('wrote', os.path.normpath(OUT), [s for s in SIZES])
