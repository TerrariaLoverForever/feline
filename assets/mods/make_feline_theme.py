"""Builds the "Feline Theme" mod: Feline-style icons for Roblox's top bar.

Every image is drawn here from scratch (simple shapes, Feline's colors); no
Roblox artwork and no other theme's artwork is used. The file names and
sizes are the ones Roblox uses for its top-bar icons (1x, @2x, @3x), with the
same padding, so the icons sit exactly where Roblox's own would.

Run from the repository root with Pillow installed:

    python assets/mods/make_feline_theme.py

Run make_feline_icons.py first (it adds the icon font patches). This writes
assets/mods/feline-theme/ (the mod) and
assets/mods/feline_theme_files.rs (the list Feline builds in). Both are
committed.
"""
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "assets" / "mods" / "feline-theme"
LIST = ROOT / "assets" / "mods" / "feline_theme_files.rs"
TOPBAR = "content/textures/ui/TopBar"

# Feline's colors (crates/feline/ui/theme.slint).
LAVENDER = (236, 232, 255, 255)  # outlines, like Roblox's white
ACCENT = (157, 140, 255, 255)  # filled "on" icons
DEEP = (36, 26, 64, 255)  # details on filled shapes
SS = 8  # supersampling: draw 8x larger, then shrink smoothly


class Pen:
    """Draws in a 24x24 unit box placed inside the image (like Feline's own
    UI icons), at SS times the final size."""

    def __init__(self, draw: ImageDraw.ImageDraw, box: tuple[float, float, float]):
        self.d = draw
        self.x0, self.y0, self.unit = box  # top-left and size of one unit, in px

    def p(self, x: float, y: float) -> tuple[float, float]:
        return (self.x0 + x * self.unit, self.y0 + y * self.unit)

    def w(self, units: float) -> int:
        return max(1, round(units * self.unit))

    def rect(self, x1, y1, x2, y2, r, fill=None, outline=None, width=2.0):
        self.d.rounded_rectangle(
            [self.p(x1, y1), self.p(x2, y2)],
            radius=r * self.unit,
            fill=fill,
            outline=outline,
            width=self.w(width) if outline else 0,
        )

    def poly(self, points, fill=None, outline=None, width=2.0):
        pts = [self.p(x, y) for x, y in points]
        if fill:
            self.d.polygon(pts, fill=fill)
        if outline:
            self.d.line(pts + [pts[0]], fill=outline, width=self.w(width), joint="curve")
            for pt in pts:  # round corners
                self.dot_px(pt, self.w(width) / 2, outline)

    def line(self, points, color, width=2.0):
        pts = [self.p(x, y) for x, y in points]
        self.d.line(pts, fill=color, width=self.w(width), joint="curve")
        for pt in (pts[0], pts[-1]):
            self.dot_px(pt, self.w(width) / 2, color)

    def dot(self, x, y, r, fill):
        self.dot_px(self.p(x, y), r * self.unit, fill)

    def dot_px(self, center, r, fill):
        cx, cy = center
        self.d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=fill)

    def ellipse(self, x1, y1, x2, y2, fill=None, outline=None, width=2.0):
        self.d.ellipse(
            [self.p(x1, y1), self.p(x2, y2)],
            fill=fill,
            outline=outline,
            width=self.w(width) if outline else 0,
        )


# ---------------------------------------------------------------------------
# The icons. `on` = the filled, active look; otherwise an outline.
# ---------------------------------------------------------------------------

def cat_head(pen: Pen, on: bool) -> None:
    """The menu button: a cat's head with glowing eyes."""
    head = [
        (3.4, 13), (3.2, 8), (3.6, 2.5), (9, 6), (15, 6), (20.4, 2.5), (20.8, 8), (20.6, 13),
        (19.6, 17), (17, 20.2), (13.5, 21.6), (10.5, 21.6), (7, 20.2), (4.4, 17),
    ]
    pen.poly(head, fill=ACCENT)
    pen.ellipse(6.5, 11.5, 11, 15.5, fill=LAVENDER)
    pen.ellipse(13, 11.5, 17.5, 15.5, fill=LAVENDER)
    pen.ellipse(8, 12.2, 9.6, 15, fill=DEEP)
    pen.ellipse(14.4, 12.2, 16, 15, fill=DEEP)


def chat(pen: Pen, on: bool) -> None:
    """A speech bubble with cat ears."""
    fill = ACCENT if on else None
    pen.poly([(4.5, 6), (6.5, 1.5), (10, 5)], fill=fill or LAVENDER)
    pen.poly([(14, 5), (17.5, 1.5), (19.5, 6)], fill=fill or LAVENDER)
    pen.rect(2, 4.5, 22, 18, 5, fill=fill, outline=None if on else LAVENDER)
    pen.poly([(6, 17), (5.5, 22.5), (11, 17)], fill=fill or LAVENDER)
    dots = DEEP if on else LAVENDER
    for x in (7.5, 12, 16.5):
        pen.dot(x, 11.3, 1.3, dots)


def paw(pen: Pen, on: bool) -> None:
    """Emotes: a paw."""
    color = ACCENT if on else None
    for x, y in [(4.6, 9.2), (8.8, 4.6), (15.2, 4.6), (19.4, 9.2)]:
        if on:
            pen.dot(x, y, 2.4, ACCENT)
        else:
            pen.ellipse(x - 2.2, y - 2.2, x + 2.2, y + 2.2, outline=LAVENDER, width=1.7)
    pen.ellipse(6, 10.5, 18, 21.5, fill=color, outline=None if on else LAVENDER, width=2)


def backpack(pen: Pen, on: bool) -> None:
    """Inventory: a backpack with cat ears."""
    fill = ACCENT if on else None
    pen.line([(9, 6), (9, 3.8), (15, 3.8), (15, 6)], LAVENDER if not on else ACCENT, 1.8)
    pen.poly([(5, 8.5), (5.5, 4.2), (8.8, 6.6)], fill=fill or LAVENDER)
    pen.poly([(15.2, 6.6), (18.5, 4.2), (19, 8.5)], fill=fill or LAVENDER)
    pen.rect(4.5, 6, 19.5, 21.5, 4, fill=fill, outline=None if on else LAVENDER)
    pocket = DEEP if on else LAVENDER
    pen.rect(8, 13.5, 16, 18.5, 1.6, outline=pocket, width=1.6)


def podium(pen: Pen, on: bool) -> None:
    """Leaderboard: a podium with a star above first place."""
    fill = ACCENT if on else None
    out = None if on else LAVENDER
    pen.rect(2.5, 14, 8.5, 21.5, 1.2, fill=fill, outline=out, width=1.8)
    pen.rect(9, 10, 15, 21.5, 1.2, fill=fill, outline=out, width=1.8)
    pen.rect(15.5, 16.5, 21.5, 21.5, 1.2, fill=fill, outline=out, width=1.8)
    star = [(12, 1.8), (13.1, 4.6), (16, 4.9), (13.8, 6.8), (14.5, 9.4),
            (12, 8), (9.5, 9.4), (10.2, 6.8), (8, 4.9), (10.9, 4.6)]
    pen.poly(star, fill=ACCENT if on else LAVENDER)


def beans(pen: Pen, on: bool) -> None:
    """More: three toe beans."""
    color = ACCENT if on else LAVENDER
    for x in (4.5, 12, 19.5):
        pen.ellipse(x - 2.6, 9.6, x + 2.6, 14.4, fill=color)


def close_button(draw: ImageDraw.ImageDraw, size: int) -> None:
    """Close: a purple rounded square with a light X (fills the image)."""
    pen = Pen(draw, (0, 0, size / 24))
    pen.rect(0.5, 0.5, 23.5, 23.5, 6, fill=ACCENT)
    pen.line([(8, 8), (16, 16)], LAVENDER, 2.6)
    pen.line([(16, 8), (8, 16)], LAVENDER, 2.6)


def icon_base(draw: ImageDraw.ImageDraw, size: int) -> None:
    """The soft square behind top-bar icons: Feline's deep purple, see-through."""
    pen = Pen(draw, (0, 0, size / 24))
    pen.rect(0, 0, 24, 24, 7, fill=(30, 20, 56, 150))


# name -> (drawing, 1x canvas size, 1x content box: (left, top, size)),
# matching the padding of Roblox's own icons.
ICONS = {
    "coloredlogo": (cat_head, 24, (2, 2, 20), None),
    "chat": (chat, 20, (0, 0, 20), True),
    "emotes": (paw, 36, (8, 8, 20), True),
    "inventory": (backpack, 36, (8, 8, 20), True),
    "leaderboard": (podium, 36, (8, 8, 20), True),
    "more": (beans, 24, (2, 2, 20), True),
}


def render(draw_fn, canvas: int, box, on: bool, scale: int) -> Image.Image:
    size = canvas * scale
    big = Image.new("RGBA", (size * SS, size * SS), (0, 0, 0, 0))
    left, top, inner = (v * scale * SS for v in box)
    draw_fn(Pen(ImageDraw.Draw(big), (left, top, inner / 24)), on)
    return big.resize((size, size), Image.LANCZOS)


def save(img: Image.Image, rel: str, written: list[str]) -> None:
    path = OUT / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(path, optimize=True)
    written.append(rel)


def main() -> None:
    written: list[str] = []
    for name, (fn, canvas, box, has_states) in ICONS.items():
        states = [("Off", False), ("On", True)] if has_states else [("", False)]
        for suffix, on in states:
            for scale, tag in [(1, ""), (2, "@2x"), (3, "@3x")]:
                # Roblox's chat icon is 20 px at 1x but 48/72 at 2x/3x.
                c = canvas
                b = box
                if name == "chat" and scale > 1:
                    c, b = 24, (2, 2, 20)
                save(render(fn, c, b, on, scale), f"{TOPBAR}/{name}{suffix}{tag}.png", written)
    for name, base, fn in [("close", 32, close_button), ("iconBase", 32, icon_base)]:
        for scale, tag in [(1, ""), (2, "@2x"), (3, "@3x")]:
            size = base * scale
            big = Image.new("RGBA", (size * SS, size * SS), (0, 0, 0, 0))
            fn(ImageDraw.Draw(big), size * SS)
            save(big.resize((size, size), Image.LANCZOS), f"{TOPBAR}/{name}{tag}.png", written)

    # Old outputs of earlier versions of this script must not linger.
    for old in OUT.rglob("*.png"):
        if old.relative_to(OUT).as_posix() not in written:
            old.unlink()
    # The font patches come from make_feline_icons.py; list them too.
    written += sorted(
        f.relative_to(OUT).as_posix() for f in OUT.rglob("*.ttf")
    )

    lines = [
        "// Generated by assets/mods/make_feline_theme.py. Don't edit by hand.",
        "&[",
    ]
    for rel in written:
        lines.append(
            f'    ("{rel}", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), '
            f'"/../../assets/mods/feline-theme/{rel}"))),'
        )
    lines.append("]")
    LIST.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"{len(written)} images")


if __name__ == "__main__":
    main()
