"""Makes docs/tour.gif for the README: screenshots one after another, with
short cross-fades.

Reads the screenshots named in SLIDES from docs/screenshots: frames of the
window, all the same size, taken with the app's screenshot hook
(SCENE_SPLITTER_SCREENSHOT) and turned into PNGs by scripts/frame_to_png.py.
Requires Pillow.
Usage: python scripts/make_tour.py
"""
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
SCREENSHOTS = ROOT / "docs" / "screenshots"
SLIDES = ["welcome.png", "editor.png", "studio.png", "soft-light.png", "settings.png", "editor-light.png"]
WIDTH = 960
HOLD_MS = 2400
FADE_STEPS = 4
FADE_MS = 70
COLORS = 192


def load_slides() -> list[Image.Image]:
    """The screenshots at the width of the tour. They must all be one size,
    or the tour would jump."""
    images = []
    for name in SLIDES:
        with Image.open(SCREENSHOTS / name) as image:
            images.append(image.convert("RGB"))
    size = images[0].size
    for name, image in zip(SLIDES, images):
        if image.size != size:
            raise ValueError(f"{name} is {image.size}, the other screenshots are {size}")
    height = round(size[1] * WIDTH / size[0])
    return [image.resize((WIDTH, height), Image.LANCZOS) for image in images]


def timeline(slides: list[Image.Image]) -> list[tuple[Image.Image, int]]:
    """Each slide held, then cross-faded into the next, with how long each
    frame shows. The last slide fades back into the first."""
    shown = []
    for current, following in zip(slides, slides[1:] + slides[:1]):
        shown.append((current, HOLD_MS))
        shown += [
            (Image.blend(current, following, step / (FADE_STEPS + 1)), FADE_MS)
            for step in range(1, FADE_STEPS + 1)
        ]
    return shown


def main() -> None:
    frames, durations = zip(*timeline(load_slides()))
    paletted = [frame.quantize(colors=COLORS, method=Image.Quantize.MEDIANCUT) for frame in frames]
    path = ROOT / "docs" / "tour.gif"
    paletted[0].save(
        path, save_all=True, append_images=paletted[1:], duration=list(durations), loop=0, optimize=True, disposal=1
    )
    print(path, path.stat().st_size // 1024, "KB")


if __name__ == "__main__":
    main()
