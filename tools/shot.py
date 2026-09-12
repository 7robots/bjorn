#!/usr/bin/env python3
"""Turn a cell dump from `cargo run --example shot` into a PNG.

    cargo run --example shot -- --theme red-graphite --out /tmp/a.json
    uv run --with pillow python tools/shot.py /tmp/a.json /tmp/a.png

One cell is one CELL_W x CELL_H box. The font is whatever Pillow finds; the
point is the colours and the layout, not typographic fidelity.
"""

from __future__ import annotations

import json
import sys

from PIL import Image, ImageDraw, ImageFont

CELL_W, CELL_H = 9, 19
FONTS = [
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/SFNSMono.ttf",
    "/Library/Fonts/DejaVuSansMono.ttf",
]


def font(size: int) -> ImageFont.FreeTypeFont:
    for path in FONTS:
        try:
            return ImageFont.truetype(path, size)
        except OSError:
            continue
    return ImageFont.load_default()


def rgb(value: str, fallback: str) -> str:
    return value if value.startswith("#") else fallback


def main() -> None:
    doc = json.load(open(sys.argv[1]))
    out = sys.argv[2] if len(sys.argv) > 2 else sys.argv[1].replace(".json", ".png")
    w, h = doc["width"], doc["height"]
    img = Image.new("RGB", (w * CELL_W, h * CELL_H), "#000000")
    draw = ImageDraw.Draw(img)
    regular, bold = font(15), font(15)
    try:
        bold = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", 15, index=1)
    except OSError:
        pass

    for cell in doc["cells"]:
        x, y = cell["x"] * CELL_W, cell["y"] * CELL_H
        bg = rgb(cell["bg"], "#000000")
        fg = rgb(cell["fg"], "#e0e0e0")
        mods = cell["mods"]
        if "REVERSED" in mods:
            fg, bg = bg, fg
        draw.rectangle([x, y, x + CELL_W, y + CELL_H], fill=bg)
        glyph = cell["s"]
        if glyph.strip():
            draw.text(
                (x, y + 2),
                glyph,
                font=bold if "BOLD" in mods else regular,
                fill=fg,
            )
        if "UNDERLINED" in mods:
            draw.line([x, y + CELL_H - 2, x + CELL_W, y + CELL_H - 2], fill=fg)

    img.save(out)
    print(out)


if __name__ == "__main__":
    main()
