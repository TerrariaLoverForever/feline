"""Builds Feline's icon fonts: Feline-style versions of the icons in Roblox's
top bar and in-game menu.

Roblox draws those icons from an icon font (BuilderIcons), one named glyph per
icon. Feline never ships Roblox's font. It ships these two small fonts of its
own drawings instead, with glyphs named like the Roblox icons they stand in
for; when the Feline Theme is applied, Feline swaps them into the user's own
copy of Roblox's font (crates/feline-core/src/fontpatch.rs).

Every shape here is drawn from scratch on Roblox's icon grid (24 units of 80
font units; glyphs start at x = 0 like Roblox's), so the icons sit where the
originals would.

Run from the repository root (needs `fonttools` and `skia-pathops`):

    python assets/mods/make_feline_icons.py

Writes them into the Feline Theme mod as font patches, named after the Roblox
font each one patches: assets/mods/feline-theme/font-patches/<path>. Then run
make_feline_theme.py to refresh the list Feline builds in.
"""
import math
from pathlib import Path

import pathops
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.cu2quPen import Cu2QuPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen

ROOT = Path(__file__).resolve().parents[2]
OUT = (ROOT / "assets" / "mods" / "feline-theme" / "font-patches" / "ExtraContent" / "LuaPackages"
       / "Packages" / "_Index" / "BuilderIcons" / "BuilderIcons" / "Font")
UNIT = 80  # font units per grid unit
TOP = 1536  # y of the grid's top edge
STROKE = 2.0  # line width of the outline (regular) style, in grid units
K = 0.5523  # cubic circle constant


# ---------------------------------------------------------------------------
# Shapes on the 24-unit grid (y grows downwards, like the UI icons)
# ---------------------------------------------------------------------------

def P(x, y):
    return (x * UNIT, TOP - y * UNIT)


def poly(points, closed=True):
    p = pathops.Path()
    p.moveTo(*P(*points[0]))
    for pt in points[1:]:
        p.lineTo(*P(*pt))
    if closed:
        p.close()
    return p


def circle(cx, cy, r):
    p = pathops.Path()
    k = r * K
    p.moveTo(*P(cx + r, cy))
    p.cubicTo(*P(cx + r, cy + k), *P(cx + k, cy + r), *P(cx, cy + r))
    p.cubicTo(*P(cx - k, cy + r), *P(cx - r, cy + k), *P(cx - r, cy))
    p.cubicTo(*P(cx - r, cy - k), *P(cx - k, cy - r), *P(cx, cy - r))
    p.cubicTo(*P(cx + k, cy - r), *P(cx + r, cy - k), *P(cx + r, cy))
    p.close()
    return p


def ellipse(cx, cy, rx, ry):
    p = pathops.Path()
    kx, ky = rx * K, ry * K
    p.moveTo(*P(cx + rx, cy))
    p.cubicTo(*P(cx + rx, cy + ky), *P(cx + kx, cy + ry), *P(cx, cy + ry))
    p.cubicTo(*P(cx - kx, cy + ry), *P(cx - rx, cy + ky), *P(cx - rx, cy))
    p.cubicTo(*P(cx - rx, cy - ky), *P(cx - kx, cy - ry), *P(cx, cy - ry))
    p.cubicTo(*P(cx + kx, cy - ry), *P(cx + rx, cy - ky), *P(cx + rx, cy))
    p.close()
    return p


def rrect(x1, y1, x2, y2, r):
    p = pathops.Path()
    k = r * (1 - K)
    p.moveTo(*P(x1 + r, y1))
    p.lineTo(*P(x2 - r, y1))
    p.cubicTo(*P(x2 - k, y1), *P(x2, y1 + k), *P(x2, y1 + r))
    p.lineTo(*P(x2, y2 - r))
    p.cubicTo(*P(x2, y2 - k), *P(x2 - k, y2), *P(x2 - r, y2))
    p.lineTo(*P(x1 + r, y2))
    p.cubicTo(*P(x1 + k, y2), *P(x1, y2 - k), *P(x1, y2 - r))
    p.lineTo(*P(x1, y1 + r))
    p.cubicTo(*P(x1, y1 + k), *P(x1 + k, y1), *P(x1 + r, y1))
    p.close()
    return p


def line(points, width=STROKE):
    """An open polyline drawn with round ends."""
    return stroked(poly(points, closed=False), width)


def arc(cx, cy, r, start_deg, end_deg, width=STROKE, steps=24):
    pts = []
    for i in range(steps + 1):
        a = math.radians(start_deg + (end_deg - start_deg) * i / steps)
        pts.append((cx + r * math.cos(a), cy + r * math.sin(a)))
    return line(pts, width)


def stroked(path, width=STROKE):
    p = pathops.Path(path)
    p.stroke(width * UNIT, pathops.LineCap.ROUND_CAP, pathops.LineJoin.ROUND_JOIN, 4)
    p.convertConicsToQuads()
    return p


def union(*paths):
    out = pathops.Path()
    for p in paths:
        out = pathops.op(out, p, pathops.PathOp.UNION)
    return out


def minus(a, *bs):
    for b in bs:
        a = pathops.op(a, b, pathops.PathOp.DIFFERENCE)
    return a


def outline(shape):
    """The regular style of a closed shape: its outline, drawn inside a bit."""
    return stroked(shape)


def paw(cx, cy, s):
    """A paw print centred near (cx, cy); `s` scales it (1 = 10 units wide)."""
    toes = [(-3.4, -1.8), (-1.3, -4.2), (1.3, -4.2), (3.4, -1.8)]
    parts = [ellipse(cx + x * s, cy + y * s, 1.15 * s, 1.35 * s) for x, y in toes]
    parts.append(ellipse(cx, cy + 1.5 * s, 3.0 * s, 2.5 * s))
    return union(*parts)


def ears(x1, x2, y, h=3.2):
    """Two cat ears standing on the line y between x1 and x2."""
    w = (x2 - x1) * 0.32
    return union(
        poly([(x1, y + 0.6), (x1 + 0.4, y - h), (x1 + w, y + 0.2)]),
        poly([(x2 - w, y + 0.2), (x2 - 0.4, y - h), (x2, y + 0.6)]),
    )


def cat_head(x1, y1, x2, y2):
    """A cat's head (with ears) filling the box."""
    w, h = x2 - x1, y2 - y1
    def at(u, v):
        return (x1 + u * w, y1 + v * h)
    pts = [at(0.02, 0.55), at(0.0, 0.32), at(0.03, 0.0), at(0.3, 0.17), at(0.7, 0.17),
           at(0.97, 0.0), at(1.0, 0.32), at(0.98, 0.55), at(0.92, 0.75), at(0.78, 0.92),
           at(0.6, 1.0), at(0.4, 1.0), at(0.22, 0.92), at(0.08, 0.75)]
    return stroked(poly(pts), 1.0) if False else union(poly(pts), stroked(poly(pts), 1.0))


def cat_eyes(x1, y1, x2, y2):
    w, h = x2 - x1, y2 - y1
    return union(
        ellipse(x1 + 0.32 * w, y1 + 0.55 * h, 0.085 * w, 0.12 * h),
        ellipse(x1 + 0.68 * w, y1 + 0.55 * h, 0.085 * w, 0.12 * h),
    )


# ---------------------------------------------------------------------------
# The icons, by the name of the Roblox icon each one replaces.
# Each returns (regular, filled).
# ---------------------------------------------------------------------------

def logo():
    # The button at the top left: a cat's head, solid in both styles (the
    # Roblox logo it replaces is solid too).
    head = cat_head(1.5, 1.5, 22.5, 21.5)
    shape = minus(head, cat_eyes(1.5, 1.5, 22.5, 21.5))
    return shape, shape


def menu():
    bars = [line([(3, y), (21, y)]) for y in (8, 13.5, 19)]
    top_ears = ears(3.5, 20.5, 6.6, 3.4)
    shape = union(*bars, top_ears)
    filled = union(*[line([(3, y), (21, y)], 2.6) for y in (8, 13.5, 19)], ears(3.5, 20.5, 6.4, 3.6))
    return shape, filled


def chat():
    bubble = union(rrect(2, 5, 22, 18, 4.5), poly([(6, 17), (5.6, 22), (11, 17.5)]))
    text = union(line([(8, 10), (16, 10)]), line([(9.5, 13.5), (14.5, 13.5)]))
    regular = union(outline(bubble), ears(3.5, 20.5, 5, 3.4), text)
    filled = minus(union(bubble, ears(3.5, 20.5, 5.2, 3.6)), text)
    return regular, filled


def microphone():
    capsule = rrect(8, 2.5, 16, 14.5, 4)
    stand = union(arc(12, 10.5, 7, 0, 180), line([(12, 17.5), (12, 21.5)]), line([(8, 21.5), (16, 21.5)]))
    capsule_ears = ears(8.4, 15.6, 3.2, 2.6)
    regular = union(outline(capsule), capsule_ears, stand)
    filled = union(capsule, capsule_ears, stand)
    return regular, filled


def two_people():
    back = cat_head(11.5, 2.5, 22.5, 13.5)
    front = cat_head(1.5, 8.5, 15.5, 21.5)
    gap = stroked(front, 2.0)
    regular = union(
        minus(outline(back), union(front, gap)),
        outline(front),
        cat_eyes(1.5, 8.5, 15.5, 21.5),
    )
    filled = union(minus(back, gap), minus(front, cat_eyes(1.5, 8.5, 15.5, 21.5)))
    return regular, filled


def gear():
    teeth = []
    for i in range(8):
        a = math.radians(i * 45)
        cx, cy = 12 + 8.3 * math.cos(a), 12.5 + 8.3 * math.sin(a)
        p = pathops.Path(rrect(cx - 1.8, cy - 1.8, cx + 1.8, cy + 1.8, 0.8))
        teeth.append(p)
    body = union(circle(12, 12.5, 8), *teeth)
    centre = paw(12, 12.3, 0.62)
    regular = union(outline(body), centre)
    filled = minus(body, centre)
    return regular, filled


def image():
    frame = rrect(2, 4, 22, 20.5, 3.5)
    inside = paw(12, 12.6, 0.72)
    regular = union(outline(frame), inside)
    filled = minus(frame, inside)
    return regular, filled


def flag():
    pole = line([(4.5, 2.5), (4.5, 22)])
    cloth = poly([(4.5, 3), (20.5, 3), (17, 8.2), (20.5, 13.5), (4.5, 13.5)])
    print_ = paw(11.5, 8.4, 0.42)
    regular = union(pole, outline(cloth), print_)
    filled = union(pole, minus(cloth, print_))
    return regular, filled


def help_():
    face = union(circle(12, 13, 9), ears(4, 20, 7.4, 4.0))
    question = union(
        arc(12, 10.6, 2.8, 190, 360 + 75, 2.0),
        line([(12.75, 13.3), (12, 14.6), (12, 15.4)], 2.0),
        circle(12, 18.4, 1.25),
    )
    regular = union(outline(circle(12, 13, 9)), ears(4, 20, 6.2, 4.0), question)
    filled = minus(face, question)
    return regular, filled


ICONS = {
    "tilt": logo,
    "three-bars-horizontal": menu,
    "speech-bubble-align-center": chat,
    "microphone": microphone,
    "two-people": two_people,
    "gear": gear,
    "image": image,
    "flag": flag,
    "circle-question": help_,
}


# ---------------------------------------------------------------------------
# Building the fonts
# ---------------------------------------------------------------------------

def to_glyph(path):
    """A TrueType glyph from a path, moved so it starts at x = 0 (like
    Roblox's icons, whose left side bearing is 0)."""
    bounds = path.bounds
    x_min = bounds[0] if bounds else 0
    pen = TTGlyphPen(None)
    path.draw(TransformPen(Cu2QuPen(pen, max_err=1.0, reverse_direction=True), (1, 0, 0, 1, -x_min, 0)))
    return pen.glyph()


def build(style: int, filename: str) -> None:
    names = [".notdef"] + list(ICONS)
    glyphs = {".notdef": TTGlyphPen(None).glyph()}
    for name, make in ICONS.items():
        shape = make()[style]
        shape.simplify()
        glyphs[name] = to_glyph(shape)
    fb = FontBuilder(2048, isTTF=True)
    fb.setupGlyphOrder(names)
    fb.setupCharacterMap({})
    fb.setupGlyf(glyphs)
    fb.setupHorizontalMetrics({n: (2048, glyphs[n].xMin if hasattr(glyphs[n], "xMin") else 0) for n in names})
    fb.setupHorizontalHeader(ascent=1664, descent=-384)
    family = "Feline Icons"
    fb.setupNameTable({
        "familyName": family,
        "styleName": "Filled" if style else "Regular",
        "copyright": "Copyright the Feline contributors. GPL-3.0-or-later.",
    })
    fb.setupOS2(sTypoAscender=1664, sTypoDescender=-384, usWinAscent=1664, usWinDescent=384)
    fb.setupPost(keepGlyphNames=True)
    OUT.mkdir(parents=True, exist_ok=True)
    fb.save(OUT / filename)


def main() -> None:
    build(0, "BuilderIcons-Regular.ttf")
    build(1, "BuilderIcons-Filled.ttf")
    print(f"{len(ICONS)} icons, regular and filled")


if __name__ == "__main__":
    main()
