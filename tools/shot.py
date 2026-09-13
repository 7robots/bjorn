#!/usr/bin/env python3
"""Turn a cell dump from `cargo run --example shot` into a PNG.

    cargo run --example shot -- --theme red-graphite --out /tmp/a.json
    uv run --with pillow --with fonttools python tools/shot.py /tmp/a.json /tmp/a.png [--scale 2]

One cell is one CELL_W x CELL_H box, times --scale (2 gives a Retina-density
PNG for the README). Each glyph is drawn with the first font in the chain that
has it: a Nerd Font when installed (the sidebar icons), then Menlo (text, box
drawing, the checkboxes), then Apple Color Emoji (the pin). Without the Nerd
Font the icons come out as boxes; the colours and the layout are the point.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

from fontTools.ttLib import TTFont
from PIL import Image, ImageDraw, ImageFont

CELL_W, CELL_H = 9, 19
FONT_SIZE = 15
HOME = str(Path.home())
CHAIN = [
    (f"{HOME}/Library/Fonts/JetBrainsMonoNerdFontMono-Regular.ttf", 0),
    ("/System/Library/Fonts/Menlo.ttc", 0),
    ("/System/Library/Fonts/SFNSMono.ttf", 0),
    ("/Library/Fonts/DejaVuSansMono.ttf", 0),
]
BOLD_CHAIN = [
    (f"{HOME}/Library/Fonts/JetBrainsMonoNerdFontMono-Bold.ttf", 0),
    ("/System/Library/Fonts/Menlo.ttc", 1),
]
EMOJI = "/System/Library/Fonts/Apple Color Emoji.ttc"
EMOJI_SIZE = 96  # one of the strike sizes Apple's bitmap font ships; the tile is scaled down from it


class Fonts:
    def __init__(self, size: int) -> None:
        self.regular = self._load(CHAIN, size)
        self.bold = self._load(BOLD_CHAIN, size) or self.regular
        self.emoji = None
        try:
            self.emoji = (ImageFont.truetype(EMOJI, EMOJI_SIZE), TTFont(EMOJI, fontNumber=0).getBestCmap())
        except OSError:
            pass

    @staticmethod
    def _load(chain, size):
        out = []
        for path, index in chain:
            try:
                out.append((ImageFont.truetype(path, size, index=index), TTFont(path, fontNumber=index).getBestCmap()))
            except OSError:
                continue
        return out

    def pick(self, glyph: str, bold: bool):
        cp = ord(glyph[0])
        for font, cmap in (self.bold if bold else self.regular) + self.regular:
            if cp in cmap:
                return font
        return None

    def has_emoji(self, glyph: str) -> bool:
        return self.emoji is not None and ord(glyph[0]) in self.emoji[1]


def rgb(value: str, fallback: str) -> str:
    return value if value.startswith("#") else fallback


def main() -> None:
    args = sys.argv[1:]
    scale = 1
    if "--scale" in args:
        i = args.index("--scale")
        scale = int(args[i + 1])
        del args[i : i + 2]
    doc = json.load(open(args[0]))
    out = args[1] if len(args) > 1 else args[0].replace(".json", ".png")
    cw, ch = CELL_W * scale, CELL_H * scale
    w, h = doc["width"], doc["height"]
    img = Image.new("RGB", (w * cw, h * ch), "#000000")
    draw = ImageDraw.Draw(img)
    fonts = Fonts(FONT_SIZE * scale)

    for cell in doc["cells"]:
        x, y = cell["x"] * cw, cell["y"] * ch
        bg = rgb(cell["bg"], "#000000")
        fg = rgb(cell["fg"], "#e0e0e0")
        mods = cell["mods"]
        if "REVERSED" in mods:
            fg, bg = bg, fg
        draw.rectangle([x, y, x + cw, y + ch], fill=bg)
        glyph = cell["s"]
        if glyph.strip():
            font = fonts.pick(glyph, "BOLD" in mods)
            if font is not None:
                draw.text((x, y + 2 * scale), glyph, font=font, fill=fg)
            elif fonts.has_emoji(glyph):
                # Emoji are double-width; the next cell is the placeholder.
                tile = Image.new("RGBA", (EMOJI_SIZE + 20, EMOJI_SIZE + 20), (0, 0, 0, 0))
                ImageDraw.Draw(tile).text((0, 0), glyph, font=fonts.emoji[0], embedded_color=True)
                tile = tile.crop(tile.getbbox()).resize((2 * cw - 2 * scale, ch - 4 * scale))
                img.paste(tile, (x + scale, y + 2 * scale), tile)
            else:
                draw.text((x, y + 2 * scale), glyph, font=fonts.regular[0][0], fill=fg)
        if "UNDERLINED" in mods:
            draw.line([x, y + ch - 2 * scale, x + cw, y + ch - 2 * scale], fill=fg)

    img.save(out)
    print(out)


if __name__ == "__main__":
    main()
