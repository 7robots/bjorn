#!/usr/bin/env python3
"""Generate Bjorn palettes from Bear's own theme files.

    python3 tools/bear_theme.py > src/ui/palettes.rs

Bear.app ships one JSON file per theme in its BearCore framework. A file may
name a `meta.base theme` it overrides, and any value may be a `$section.key`
reference into the merged result. This script merges the chain, resolves the
references and maps Bear's keys onto the fields of `ui::theme::Theme`, filling
the few things Bear has no word for (a focus tint, the toast colours) by
blending or from the palette the theme is named after.

The mapping is fixed here rather than tuned per theme, so a new Bear theme is
one line in THEMES. `red-graphite` and its dark twin stay hand-tuned in
theme.rs; they were sampled from the app before this existed.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

BEAR = Path(
    "/Applications/Bear.app/Contents/Frameworks/BearCore.framework/Versions/A/Resources"
)

# (Bear theme file, config name, (success, warning, error)). The three toast
# colours are the green, yellow and red of the palette each theme is named
# after; Bear's files carry none. The Shibuya pair are Bear originals, so
# theirs come from their own syntax-highlight tables.
THEMES = [
    ("Nord", "nord", ("#A3BE8C", "#EBCB8B", "#BF616A")),
    ("Dracula", "dracula", ("#50FA7B", "#F1FA8C", "#FF5555")),
    ("Tokyo Night", "tokyo-night", ("#9ECE6A", "#E0AF68", "#F7768E")),
    ("Tokyo Night Light", "tokyo-night-light", ("#587539", "#8F5E15", "#F52A65")),
    ("Catppuccin Latte", "catppuccin-latte", ("#40A02B", "#DF8E1D", "#D20F39")),
    ("Catppuccin Macchiato", "catppuccin-macchiato", ("#A6DA95", "#EED49F", "#ED8796")),
    ("Shibuya Jazz", "shibuya-jazz", ("#4EA9A9", "#E38E13", "#D84848")),
    ("Shibuya Lo-fi", "shibuya-lo-fi", ("#2FA288", "#FBA80B", "#D84848")),
]


def deep_merge(base: dict, over: dict) -> dict:
    out = dict(base)
    for k, v in over.items():
        out[k] = deep_merge(out[k], v) if isinstance(v, dict) and isinstance(out.get(k), dict) else v
    return out


def load(name: str) -> dict:
    doc = json.loads((BEAR / f"{name}.theme").read_text())
    base = doc.get("meta", {}).get("base theme")
    return deep_merge(load(base), doc) if base else doc


def get(doc: dict, path: str) -> str:
    node = doc
    for part in path.split("."):
        node = node[part]
    return get(doc, node[1:]) if isinstance(node, str) and node.startswith("$") else node


def parse(hex_: str) -> tuple[int, int, int]:
    h = hex_.lstrip("#")
    if len(h) == 3:
        h = "".join(c * 2 for c in h)
    return int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16)


def fmt(c: tuple[int, int, int]) -> str:
    return "0x%02X%02X%02X" % c


def blend(a: str, b: str, t: float) -> tuple[int, int, int]:
    pa, pb = parse(a), parse(b)
    return tuple(round(x + (y - x) * t) for x, y in zip(pa, pb))


def luminance(hex_: str) -> float:
    def lin(v: int) -> float:
        v /= 255
        return v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4

    r, g, b = parse(hex_)
    return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)


def on(bg: str, dark_text: str) -> str:
    """Text over `bg`: the theme's darkest surface when the fill is light, white otherwise."""
    return dark_text if luminance(bg) > 0.3 else "#FFFFFF"


def theme(bear_name: str, name: str, semantics: tuple[str, str, str]) -> str:
    d = load(bear_name)
    g = lambda p: get(d, p)  # noqa: E731
    bg, text, muted = g("base.background color"), g("base.text color"), g("base.text secondary color")
    bg2, accent = g("base.background secondary color"), g("base.accent color")
    sb_bg, sb_text = g("sidebar.background color"), g("sidebar.text color")
    dark = luminance(bg) < 0.5
    darkest = bg if dark else text
    fields = {
        "background": parse(bg),
        "surface": parse(bg),
        "surface_focus": blend(bg, text, 0.04),
        "sidebar_bg": parse(sb_bg),
        "sidebar_focus": blend(sb_bg, sb_text, 0.04),
        "foreground": parse(text),
        "muted": parse(muted),
        "sidebar_fg": parse(sb_text),
        "sidebar_muted": blend(sb_text, sb_bg, 0.35),
        "border": parse(g("base.stroke color")),
        "sidebar_border": parse(g("sidebar.stroke color")),
        "header_bg": parse(bg2),
        "header_fg": parse(g("editor.headers.text color")),
        "sidebar_header_bg": blend(sb_bg, sb_text, 0.06),
        "sidebar_header_fg": parse(sb_text),
        "header_focus_bg": parse(accent),
        "header_focus_fg": parse(on(accent, darkest)),
        "cursor_bg": parse(accent),
        "cursor_fg": parse(on(accent, darkest)),
        "cursor_blur_bg": parse(g("notes.selection background color")),
        "cursor_blur_fg": parse(text),
        "sidebar_cursor_blur_bg": parse(g("sidebar.background secondary color")),
        "sidebar_cursor_blur_fg": parse(g("sidebar.text secondary color")),
        "footer_bg": parse(bg2),
        "footer_fg": parse(muted),
        "footer_key": parse(accent),
        "accent": parse(accent),
        "primary": parse(accent),
        "success": parse(semantics[0]),
        "warning": parse(semantics[1]),
        "error": parse(semantics[2]),
        "heading": parse(g("editor.headers.text color")),
        "heading_alt": parse(text),
        "link": parse(g("editor.link color")),
        "bullet": parse(g("editor.list marker color")),
        "code_fg": parse(g("editor.code.text color")),
        "code_bg": parse(g("editor.code.background color")),
        "tag_fg": parse(g("editor.tag.text color")),
        "tag_bg": parse(g("editor.tag.background color")),
    }
    const = name.upper().replace("-", "_")
    lines = [f"/// Bear's {bear_name}.", f"pub const {const}: Theme = Theme {{", f'    name: "{name}",', f"    dark: {str(dark).lower()},"]
    lines += [f"    {k}: rgb({fmt(v)})," for k, v in fields.items()]
    lines.append("};")
    return "\n".join(lines)


def main() -> None:
    out = [
        "//! Palettes generated from Bear's own theme files by `tools/bear_theme.py`.",
        "//! Do not edit by hand: change the script or the list in it and rerun",
        "//!",
        "//!     python3 tools/bear_theme.py > src/ui/palettes.rs",
        "",
        "use super::theme::{rgb, Theme};",
        "",
    ]
    out += [theme(*t) + "\n" for t in THEMES]
    sys.stdout.write("\n".join(out))


if __name__ == "__main__":
    main()
