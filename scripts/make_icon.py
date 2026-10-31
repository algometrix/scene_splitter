"""Draws the Cutaway icon: a video frame on a slate tile, cut in two
by a slanted gap. The left piece is white with a play mark, the right piece
violet, and a coral spark sits in the cut.

Writes assets/icon.png (256 px, the window icon), assets/icon.ico (16 to
256 px, the program file on Windows) and assets/mark.png (512 px, for the
README). Requires Pillow.
Usage: python scripts/make_icon.py
"""
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets"

SLATE = (0x3A, 0x3E, 0x46, 255)
SLATE_EDGE = (0x4A, 0x4E, 0x57, 255)
WHITE = (0xFF, 0xFF, 0xFF, 255)
VIOLET = (0x7C, 0x6D, 0xF2, 255)
CORAL = (0xFF, 0x7A, 0x50, 255)

CANVAS = 1024  # drawn this large, then scaled down for smooth edges
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]


def frame_piece(size: int, polygon: list[tuple[float, float]], color) -> Image.Image:
    """One piece of the cut frame: the rounded frame, kept only inside `polygon`."""
    frame = Image.new("L", (size, size), 0)
    left, top, right, bottom = 0.13 * size, 0.27 * size, 0.85 * size, 0.75 * size
    ImageDraw.Draw(frame).rounded_rectangle(
        (left, top, right, bottom), radius=0.085 * size, fill=255
    )
    keep = Image.new("L", (size, size), 0)
    ImageDraw.Draw(keep).polygon(polygon, fill=255)
    mask = Image.composite(frame, Image.new("L", (size, size), 0), keep)
    piece = Image.new("RGBA", (size, size), color)
    piece.putalpha(mask)
    return piece


def icon(size: int = CANVAS) -> Image.Image:
    s = size
    canvas = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    draw = ImageDraw.Draw(canvas)
    draw.rounded_rectangle((0, 0, s - 1, s - 1), radius=0.225 * s, fill=SLATE_EDGE)
    inset = 0.012 * s
    draw.rounded_rectangle(
        (inset, inset, s - 1 - inset, s - 1 - inset), radius=0.215 * s, fill=SLATE
    )

    # The cut runs from upper right to lower left, a little past the middle,
    # and the right piece sits slightly lower, as if it were pulled away.
    gap = 0.045 * s
    cut_top, cut_bottom = 0.575 * s, 0.455 * s
    left_side = [(0, 0), (cut_top - gap / 2, 0), (cut_bottom - gap / 2, s), (0, s)]
    right_side = [(cut_top + gap / 2, 0), (s, 0), (s, s), (cut_bottom + gap / 2, s)]
    canvas.alpha_composite(frame_piece(s, left_side, WHITE))
    right_piece = frame_piece(s, right_side, VIOLET)
    canvas.alpha_composite(right_piece, (int(0.025 * s), int(0.03 * s)))

    # The play mark, cut out of the white piece in slate.
    cx, cy, r = 0.31 * s, 0.51 * s, 0.095 * s
    draw.polygon(
        [(cx - 0.55 * r, cy - r), (cx + 0.95 * r, cy), (cx - 0.55 * r, cy + r)], fill=SLATE
    )

    # The spark where the frame was cut.
    sx, sy, sr = 0.585 * s, 0.215 * s, 0.05 * s
    draw.polygon(
        [
            (sx, sy - sr * 1.6),
            (sx + sr * 0.45, sy - sr * 0.45),
            (sx + sr * 1.6, sy),
            (sx + sr * 0.45, sy + sr * 0.45),
            (sx, sy + sr * 1.6),
            (sx - sr * 0.45, sy + sr * 0.45),
            (sx - sr * 1.6, sy),
            (sx - sr * 0.45, sy - sr * 0.45),
        ],
        fill=CORAL,
    )
    return canvas


def main() -> None:
    ASSETS.mkdir(exist_ok=True)
    large = icon()
    large.resize((256, 256), Image.LANCZOS).save(ASSETS / "icon.png")
    large.resize((512, 512), Image.LANCZOS).save(ASSETS / "mark.png")
    large.resize((256, 256), Image.LANCZOS).save(
        ASSETS / "icon.ico", sizes=[(n, n) for n in ICO_SIZES]
    )


if __name__ == "__main__":
    main()
