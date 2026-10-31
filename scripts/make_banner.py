"""Draws the README banner in the Cutaway colours, in a dark and a light
version: the mark, the name, a tagline, and a screenshot of the review.

Writes docs/banner-dark.png and docs/banner-light.png from
docs/screenshots/editor.png and editor-light.png: frames of the review
screen in the Editor style, taken with the app's screenshot hook
(SCENE_SPLITTER_SCREENSHOT) and turned into PNGs by scripts/frame_to_png.py.
Requires Pillow and the Segoe UI fonts that come with Windows.
Usage: python scripts/make_banner.py
"""
from dataclasses import dataclass
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

from make_icon import CORAL, VIOLET, icon

ROOT = Path(__file__).resolve().parent.parent
SCREENSHOTS = ROOT / "docs" / "screenshots"
FONTS = Path(r"C:\Windows\Fonts")
WIDTH, HEIGHT = 1600, 640
SCALE = 2  # drawn at twice the size, then scaled down for smooth edges

# The banner is two columns: the words on the left, the screenshot on the
# right. The chips wrap before they reach the screenshot.
TEXT_LEFT = 96
SHOT_LEFT = 800
TEXT_RIGHT = SHOT_LEFT - 60
SHOT_TOP = 92
SHOT_WIDTH = 860
SHOT_RADIUS = 18
# The screenshot runs off the bottom edge, fading into the page over this height.
SHOT_FADE = 110

TAGLINE = ["Cut a long video into its scenes,", "without encoding it again."]
CHIPS = ["Finds the black screens", "No quality lost", "Checked before it recycles", "Free and open source"]

Color = tuple[int, int, int, int]


@dataclass(frozen=True)
class Look:
    name: str
    background: Color
    glow: Color
    ink: Color
    ink_soft: Color
    chip: Color
    screenshot: str


LOOKS = (
    Look(
        name="dark",
        background=(0x15, 0x19, 0x1E, 255),
        glow=(0x2A, 0x30, 0x38, 255),
        ink=(255, 255, 255, 255),
        ink_soft=(0xB4, 0xBA, 0xC2, 255),
        chip=(0x26, 0x2B, 0x33, 255),
        screenshot="editor.png",
    ),
    Look(
        name="light",
        background=(0xF3, 0xF4, 0xF7, 255),
        glow=(0xFF, 0xFF, 0xFF, 255),
        ink=(0x22, 0x25, 0x2B, 255),
        ink_soft=(0x56, 0x5B, 0x64, 255),
        chip=(0xFF, 0xFF, 0xFF, 255),
        screenshot="editor-light.png",
    ),
)


def font(name: str, size: int) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(str(FONTS / name), size * SCALE)


def draw_glow(canvas: Image.Image, look: Look) -> None:
    """A soft lighter glow behind the screenshot, and a violet one behind the mark."""
    s = SCALE
    glow = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(glow)
    draw.ellipse((820 * s, -120 * s, 1760 * s, 760 * s), fill=look.glow)
    draw.ellipse((40 * s, 60 * s, 360 * s, 380 * s), fill=VIOLET[:3] + (70,))
    canvas.alpha_composite(glow.filter(ImageFilter.GaussianBlur(120 * s)))


def draw_title(canvas: Image.Image, look: Look) -> None:
    s = SCALE
    left = TEXT_LEFT * s
    mark = icon().resize((132 * s, 132 * s), Image.LANCZOS)
    canvas.alpha_composite(mark, (left, 110 * s))
    draw = ImageDraw.Draw(canvas)
    draw.text((left, 270 * s), "Cutaway", font=font("segoeuib.ttf", 84), fill=look.ink)
    tagline_font = font("segoeui.ttf", 30)
    for line, words in enumerate(TAGLINE):
        draw.text((left, (385 + 40 * line) * s), words, font=tagline_font, fill=look.ink_soft)


def draw_chips(canvas: Image.Image, look: Look) -> None:
    """The chips in rows, each with a dot, wrapping before the screenshot."""
    s = SCALE
    draw = ImageDraw.Draw(canvas)
    chip_font = font("seguisb.ttf", 17)
    x, y = TEXT_LEFT * s, 500 * s
    for index, label in enumerate(CHIPS):
        width = int(draw.textlength(label, font=chip_font)) + 44 * s
        if x + width > TEXT_RIGHT * s:
            x, y = TEXT_LEFT * s, y + 50 * s
        draw.rounded_rectangle((x, y, x + width, y + 38 * s), radius=10 * s, fill=look.chip)
        dot = VIOLET if index % 2 == 0 else CORAL
        draw.ellipse((x + 14 * s, y + 15 * s, x + 22 * s, y + 23 * s), fill=dot)
        draw.text((x + 30 * s, y + 7 * s), label, font=chip_font, fill=look.ink)
        x += width + 12 * s


def screenshot(look: Look) -> Image.Image:
    """The screenshot at banner size, with rounded corners."""
    s = SCALE
    with Image.open(SCREENSHOTS / look.screenshot) as image:
        shot = image.convert("RGBA")
    width = SHOT_WIDTH * s
    shot = shot.resize((width, round(shot.height * width / shot.width)), Image.LANCZOS)
    corners = Image.new("L", shot.size, 0)
    ImageDraw.Draw(corners).rounded_rectangle((0, 0, *shot.size), radius=SHOT_RADIUS * s, fill=255)
    shot.putalpha(corners)
    return shot


def fade_mask(size: tuple[int, int]) -> Image.Image:
    """Opaque down to where the screenshot starts fading, then clear at the bottom edge."""
    s = SCALE
    mask = Image.new("L", size, 255)
    draw = ImageDraw.Draw(mask)
    top = size[1] - SHOT_FADE * s
    for row in range(top, size[1]):
        draw.line([(0, row), (size[0], row)], fill=round(255 * (size[1] - row) / (SHOT_FADE * s)))
    return mask


def place_screenshot(canvas: Image.Image, look: Look) -> None:
    """The screenshot with its shadow, fading into the page at the bottom
    edge. Only this layer fades, so the words beside it stay sharp."""
    s = SCALE
    shot = screenshot(look)
    x, y = SHOT_LEFT * s, SHOT_TOP * s
    layer = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    ImageDraw.Draw(layer).rounded_rectangle(
        (x + 10 * s, y + 24 * s, x + shot.width, y + shot.height + 24 * s),
        radius=SHOT_RADIUS * s,
        fill=(0, 0, 0, 110),
    )
    layer = layer.filter(ImageFilter.GaussianBlur(26 * s))
    layer.alpha_composite(shot, (x, y))
    faded = ImageChops.multiply(layer.getchannel("A"), fade_mask(canvas.size))
    layer.putalpha(faded)
    canvas.alpha_composite(layer)


def banner(look: Look) -> Image.Image:
    canvas = Image.new("RGBA", (WIDTH * SCALE, HEIGHT * SCALE), look.background)
    draw_glow(canvas, look)
    draw_title(canvas, look)
    draw_chips(canvas, look)
    place_screenshot(canvas, look)
    return canvas.resize((WIDTH, HEIGHT), Image.LANCZOS).convert("RGB")


def main() -> None:
    for look in LOOKS:
        path = ROOT / "docs" / f"banner-{look.name}.png"
        banner(look).save(path, optimize=True)
        print(path)


if __name__ == "__main__":
    main()
