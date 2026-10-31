"""Turns a picture saved by the app's screenshot hook (raw RGBA after a
width and height header) into a PNG, optionally cut to a box.
Requires Pillow.
Usage: python scripts/frame_to_png.py <frame.rgba> <out.png> [left top right bottom]
"""
import struct
import sys

from PIL import Image


def main() -> None:
    source, target, *box = sys.argv[1:]
    data = open(source, "rb").read()
    width, height = struct.unpack("<II", data[:8])
    image = Image.frombytes("RGBA", (width, height), data[8:]).convert("RGB")
    if box:
        image = image.crop(tuple(int(side) for side in box))
    image.save(target, optimize=True)


if __name__ == "__main__":
    main()
