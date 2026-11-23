"""Draw String's three icon options. Run with python native/icon/make-icon.py."""

from math import cos, radians, sin
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


OUT = Path(__file__).resolve().parent
SIZES = (16, 20, 24, 32, 40, 48, 64, 96, 128, 256)
TILE = "#141414"
THREAD = "#e7e7e7"
SUPERSAMPLE = 8


def curve(start, *segments):
    """Trace joined cubic curves. Each segment has two handles and an end."""
    points = [start]
    for a, b, end in segments:
        for i in range(1, 49):
            t = i / 48
            u = 1 - t
            points.append(tuple(u**3 * p + 3*u*u*t * q + 3*u*t*t * r + t**3 * s
                                for p, q, r, s in zip(start, a, b, end)))
        start = end
    return points


class Tile:
    """Use a 64-unit drawing grid, with weights chosen for the output size."""

    def __init__(self, size):
        self.size = size
        pixels = size * SUPERSAMPLE
        self.scale = pixels / 64
        self.image = Image.new("RGB", (pixels, pixels), TILE)
        self.pen = ImageDraw.Draw(self.image)

    def weight(self, units, minimum_px):
        return max(units, minimum_px * 64 / self.size)

    def coords(self, points):
        return [(round(x * self.scale), round(y * self.scale)) for x, y in points]

    def fill(self, points, ink):
        self.pen.polygon(self.coords(points), fill=ink)

    def dot(self, point, diameter, ink):
        x, y = point
        r = diameter / 2
        self.pen.ellipse(self.coords([(x - r, y - r), (x + r, y + r)]), fill=ink)

    def stroke(self, points, width, ink=THREAD):
        self.pen.line(self.coords(points), fill=ink, width=round(width * self.scale),
                      joint="curve")
        self.dot(points[0], width, ink)
        self.dot(points[-1], width, ink)

    def finish(self):
        # Average corner coverage separately so the filter cannot leave a halo.
        alpha = Image.new("L", self.image.size)
        pixels = alpha.width
        ImageDraw.Draw(alpha).rounded_rectangle((0, 0, pixels - 1, pixels - 1),
                                                radius=round(pixels * .22), fill=255)
        frame = self.image.resize((self.size, self.size), Image.Resampling.LANCZOS)
        frame.putalpha(alpha.resize(frame.size, Image.Resampling.BOX))
        return frame


# 1. A length of thread lying in a loose S. One end passes over the return.
def loose_loop(tile):
    weight = tile.weight(2.8, 1.45)
    if tile.size <= 24:
        # Spend the center pixels on the open eye; shorten both free ends.
        start = (49, 15)
        upper = curve(start, ((32, 6), (13, 12), (14, 24)),
                      ((15, 37), (48, 44), (48, 27)))
        bridge = curve(upper[-1], ((48, 16), (30, 20), (30, 33)))
    else:
        start = (48, 15.5)
        upper = curve(start, ((35, 9), (17, 10), (15.5, 22)),
                      ((13.5, 36), (45, 43), (46, 28)))
        bridge = curve(upper[-1], ((47, 18), (31, 20), (30.5, 33)))
    lower = curve(bridge[-1], ((30, 40), (43, 41), (43, 47)),
                  ((43, 55), (25, 56), (14, 48.5)))
    tile.stroke(upper, weight)
    # Clear the underpass only near the crossing, away from the connected loop.
    crossover = bridge[-13:] + lower[1:26]
    tile.stroke(crossover, weight + 2 * tile.weight(1.1, .70), TILE)
    tile.stroke(bridge + lower[1:], weight)
    if tile.size >= 40:
        # Two short plies come apart at the cut end.
        tile.stroke(curve(lower[-1], ((13, 48), (11.5, 46), (10.8, 46))),
                    1.25, THREAD)
        tile.stroke(curve(lower[-1], ((12.6, 48.3), (11, 48.6), (10, 48.4))),
                    1.15, THREAD)


# 2. The sidebar's two fixed points, with a plectrum displacing the string.
def downstroke(tile):
    small = tile.size <= 24
    a, b = ((8.8, 29.5), (55.2, 29.5)) if small else ((9, 29), (55, 29))
    bend = (38.1, 45) if small else (38.5, 43.2)
    string = curve((bend[0] - 1.2, bend[1] - .6),
                   (bend, (bend[0] + .3, bend[1]), (bend[0] + 1.1, bend[1] - .7)))
    tile.stroke([a, *string, b], tile.weight(2.4, 1.35))
    for point in (a, b):
        tile.dot(point, tile.weight(5.8, 2.65), THREAD)
    # Present the broad shoulder and one working tip, angled into the downstroke.
    pick = curve((24.8, 23.6), ((24, 19), (31.9, 16.5), (37.7, 16.8)),
                 ((43.6, 17.1), (42.1, 30.5), (39.2, 38.6)),
                 ((38.9, 39.7), (38.3, 40.1), (37.2, 39.4)),
                 ((31.8, 35.9), (25.7, 28.7), (24.8, 23.6)))
    tile.fill(pick, "#b9b9b9")


# 3. A tilted bobbin. The last winding runs straight out into the loose tail.
def bobbin(tile):
    small = tile.size <= 24
    angle = radians(8 if small else 16)

    def tilt(points):
        return [(29 + x*cos(angle) - y*sin(angle), 29 + x*sin(angle) + y*cos(angle))
                for x, y in points]

    # The end plates project past the thread; the slot is the hollow spindle.
    tile.fill(tilt([(-9, -15), (9, -15), (9, 15), (-9, 15)]), "#464646")
    flange = tile.weight(3.2, 1.35)
    tile.stroke(tilt([(-15, 15), (15, 15)]), flange, "#a6a6a6")
    if small:
        tile.stroke(tilt([(-15, -15), (15, -15)]), flange, "#c5c5c5")
    else:
        rim = curve((-16, -14.8), ((-16, -19.8), (16, -19.8), (16, -14.8)),
                    ((16, -10.2), (-16, -10.2), (-16, -14.8)))
        tile.fill(tilt(rim), "#b8b8b8")
        if tile.size >= 40:
            slot = curve((-4.5, -15), ((-4.5, -16.7), (4.5, -16.7), (4.5, -15)),
                         ((4.5, -13.3), (-4.5, -13.3), (-4.5, -15)))
            tile.fill(tilt(slot), TILE)

    turns = (-6, 5.5) if small else (-7.8, -2.2, 3.4, 9)
    winding = tile.weight(4.1, 1.65)
    for y in turns:
        turn = curve((-11, y - 1), ((-6, y + 1.6), (6, y + .9), (11, y - 1.3)))
        tile.stroke(tilt(turn), winding)
    tail_start = tilt([(11, turns[-1] - 1.3)])[0]
    tail = curve(tail_start, ((45, 38.5), (38, 51), (45, 52)),
                 ((49.5, 52.7), (52.5, 48.5), (55, 45)))
    tile.stroke(tail, tile.weight(2.3, 1.25))


OPTIONS = (
    ("1-loose-loop", "Loose loop", loose_loop),
    ("2-downstroke", "Downstroke", downstroke),
    ("3-bobbin", "Bobbin", bobbin),
)


def draw_frames(draw):
    frames = {}
    for size in SIZES:
        tile = Tile(size)
        draw(tile)
        frames[size] = tile.finish()
    return frames


def preview(rows):
    sheet = Image.new("RGB", (800, 888), "#050506")
    pen = ImageDraw.Draw(sheet)
    label = ImageFont.load_default(size=13)
    name = ImageFont.load_default(size=17)
    grey = "#989898"
    actual = (16, 24, 32, 48)
    centers = (333, 387, 445, 510)
    for index, (title, frames) in enumerate(rows):
        top = 296 * index
        if index:
            pen.line((24, top, 775, top), fill="#292929")
        pen.text((304, top + 49), f"{index + 1}   {title}", font=name, fill="#bdbdbd")
        sheet.paste(frames[256], (24, top + 20), frames[256])
        pen.text((152, top + 280), "256 px", font=label, fill=grey, anchor="mt")
        pen.text((304, top + 94), "Actual pixels", font=label, fill=grey)
        pen.rectangle((304, top + 119, 550, top + 191), fill="#1c1c1c")
        for x, size in zip(centers, actual):
            frame = frames[size]
            sheet.paste(frame, (x - size//2, top + 155 - size//2), frame)
            pen.text((x, top + 206), f"{size} px", font=label, fill=grey, anchor="mt")
        for x, size in ((604, 16), (713, 32)):
            frame = frames[size].resize((4*size, 4*size), Image.Resampling.NEAREST)
            sheet.paste(frame, (x - 2*size, top + 155 - 2*size), frame)
            pen.text((x, top + 230), f"{size} px (4x)", font=label, fill=grey, anchor="mt")
    sheet.save(OUT / "preview.png")


def main():
    rows = []
    for folder, title, draw in OPTIONS:
        frames = draw_frames(draw)
        path = OUT / "options" / folder
        path.mkdir(parents=True, exist_ok=True)
        frames[256].save(path / "String.png")
        frames[256].save(path / "String.ico", sizes=[(s, s) for s in SIZES],
                         append_images=[frames[s] for s in SIZES[:-1]])
        rows.append((title, frames))
    preview(rows)


if __name__ == "__main__":
    main()
