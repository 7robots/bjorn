#!/usr/bin/env python3
"""Generate Bjorn palettes from Bear's own theme files.

    python3 tools/bear_theme.py > src/ui/palettes.rs
    python3 tools/bear_theme.py --manifest > config/themes/bear-themes.sha256

Bear.app ships one JSON file per theme in its BearCore framework. A file may
name a `meta.base theme` it overrides, and any value may be a `$section.key`
reference into the merged result. This script merges the chain, resolves the
references and maps Bear's keys onto the fields of `ui::theme::Theme`, filling
the few things Bear has no word for (a focus tint, the toast colours) by
blending or from the palette the theme is named after.

The mapping is fixed here rather than tuned per theme, and every theme file
Bear ships is converted, so a Bear update that adds one needs only a rerun.
`red-graphite` and its dark twin stay hand-tuned in theme.rs; they were
sampled from the app before this existed. Bear's own Red Graphite is still
generated, as `BEAR_RED_GRAPHITE`, for the themes that name it as their base.

`--manifest` prints each theme file's SHA-256 instead: Bear's files are not in
the repo, and the manifest is how the parity test knows a Bear.app it finds
has the files `palettes.rs` was generated from.
"""

from __future__ import annotations

import colorsys
import hashlib
import json
import sys
import unicodedata
from pathlib import Path

BEAR = Path(
    "/Applications/Bear.app/Contents/Frameworks/BearCore.framework/Versions/A/Resources"
)

# The three toast colours (success, warning, error) are the green, yellow and
# red of the palette each theme is named after; Bear's files carry none. A
# theme that is Bear's own, or whose palette has no such triad, is left out
# here and gets them derived from its highlighter colours instead.
SEMANTICS = {
    "Atom": ("#98C379", "#E5C07B", "#E06C75"),
    "Ayu": ("#86B300", "#F2AE49", "#F51818"),
    "Ayu Mirage": ("#BAE67E", "#FFD580", "#FF3333"),
    "Catppuccin Latte": ("#40A02B", "#DF8E1D", "#D20F39"),
    "Catppuccin Macchiato": ("#A6DA95", "#EED49F", "#ED8796"),
    "Cobalt": ("#3AD900", "#FFC600", "#FF628C"),
    "Dracula": ("#50FA7B", "#F1FA8C", "#FF5555"),
    "Everforest Dark": ("#A7C080", "#DBBC7F", "#E67E80"),
    "Everforest Light": ("#8DA101", "#DFA000", "#F85552"),
    "Gruvbox": ("#79740E", "#B57614", "#9D0006"),
    "Nord": ("#A3BE8C", "#EBCB8B", "#BF616A"),
    "Rosé Pine": ("#31748F", "#F6C177", "#EB6F92"),
    "Rosé Pine Dawn": ("#286983", "#EA9D34", "#B4637A"),
    "Shibuya Jazz": ("#4EA9A9", "#E38E13", "#D84848"),
    "Shibuya Lo-fi": ("#2FA288", "#FBA80B", "#D84848"),
    "Solarized Dark": ("#859900", "#B58900", "#DC322F"),
    "Solarized Light": ("#859900", "#B58900", "#DC322F"),
    "Tokyo Night": ("#9ECE6A", "#E0AF68", "#F7768E"),
    "Tokyo Night Light": ("#587539", "#8F5E15", "#F52A65"),
}

# Hand-tuned in theme.rs before this script existed, so not offered from
# here. Still generated, as `BEAR_RED_GRAPHITE`, because seven of Bear's
# themes name it as their `base theme`: a copy of one of those in the user's
# themes directory is laid over Bear's own colors, not over the hand-tuned
# ones (`bear_theme::built_in_document`).
SKIP = {"Red Graphite"}



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


def contrast(a: str, b: str) -> float:
    x, y = luminance(a) + 0.05, luminance(b) + 0.05
    return x / y if x > y else y / x


def on(bg: str, candidates: list[str]) -> str:
    """Text over `bg`: the theme's own colour that reads best, when one reads
    at 3:1; plain white or near-black otherwise (a mid-orange accent on a
    light theme has neither its page nor its text colour legible over it)."""
    best = max(candidates, key=lambda c: contrast(c, bg))
    if contrast(best, bg) >= 3.0:
        return best
    return max(["#FFFFFF", "#111111"], key=lambda c: contrast(c, bg))


def legible(fg: str, bg: str, toward: str, ratio: float = 3.0) -> str:
    """`fg` pushed toward `toward` until it reads at `ratio` on `bg`."""
    t = 0.0
    while contrast(hexstr(blend(fg, toward, t)), bg) < ratio and t < 1.0:
        t += 0.05
    return hexstr(blend(fg, toward, t))


def hexstr(c: tuple[int, int, int]) -> str:
    return "#%02X%02X%02X" % c


def recolor(hex_: str, dark: bool) -> str:
    """A highlighter background's hue as a foreground: same hue, full enough
    saturation, and a lightness that sits on the page."""
    r, g, b = (v / 255 for v in parse(hex_))
    h, l, s = colorsys.rgb_to_hls(r, g, b)
    l = 0.68 if dark else 0.36
    s = max(s, 0.45)
    return hexstr(tuple(round(v * 255) for v in colorsys.hls_to_rgb(h, l, s)))


def derived_semantics(d: dict, dark: bool) -> tuple[str, str, str]:
    return tuple(
        recolor(get(d, f"editor.highlighter.{c}.background color"), dark)
        for c in ("green", "yellow", "red")
    )


# Letters NFKD does not decompose, so the ASCII fold below would drop them
# (`Bjørn` would be `bjrn`). `LETTERS` in src/ui/bear_theme.rs is the same list.
LETTERS = str.maketrans({"ß": "ss", "æ": "ae", "œ": "oe", "ø": "o", "ð": "d", "đ": "d",
                         "ħ": "h", "ı": "i", "ł": "l", "ŧ": "t", "þ": "th"})


def slug(bear_name: str) -> str:
    folded = bear_name.lower().translate(LETTERS)
    ascii_ = unicodedata.normalize("NFKD", folded).encode("ascii", "ignore").decode()
    return "-".join("".join(c if c.isalnum() else " " for c in ascii_).lower().split())


def theme(bear_name: str, prefix: str = "") -> str:
    name = slug(bear_name)
    d = load(bear_name)
    g = lambda p: get(d, p)  # noqa: E731
    bg, text, muted = g("base.background color"), g("base.text color"), g("base.text secondary color")
    bg2, accent = g("base.background secondary color"), g("base.accent color")
    sb_bg, sb_text = g("sidebar.background color"), g("sidebar.text color")
    dark = luminance(bg) < 0.5
    semantics = SEMANTICS.get(bear_name) or derived_semantics(d, dark)
    over_accent = on(accent, [bg, text, sb_bg])
    blur_bg, sb_blur_bg = g("notes.selection background color"), g("sidebar.background secondary color")
    semantics = tuple(legible(c, bg, text) for c in semantics)
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
        "header_focus_fg": parse(over_accent),
        "cursor_bg": parse(accent),
        "cursor_fg": parse(over_accent),
        "cursor_blur_bg": parse(blur_bg),
        "cursor_blur_fg": parse(on(blur_bg, [text, g("editor.headers.text color")])),
        "sidebar_cursor_blur_bg": parse(sb_blur_bg),
        "sidebar_cursor_blur_fg": parse(on(sb_blur_bg, [g("sidebar.text secondary color"), sb_text, text])),
        "footer_bg": parse(bg2),
        "footer_fg": parse(muted),
        "footer_key": parse(legible(accent, bg2, text)),
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
    const = prefix + name.upper().replace("-", "_")
    lines = [f"/// Bear's {bear_name}.", f"pub const {const}: Theme = Theme {{", f'    name: "{name}",', f"    dark: {str(dark).lower()},"]
    lines += [f"    {k}: rgb({fmt(v)})," for k, v in fields.items()]
    lines.append("};")
    return "\n".join(lines)


def main() -> None:
    out = [
        "//! Palettes generated from Bear's own theme files by `tools/bear_theme.py`.",
        "//! Do not edit by hand: change the script and rerun",
        "//!",
        "//! ```text",
        "//! python3 tools/bear_theme.py > src/ui/palettes.rs",
        "//! ```",
        "",
        # rustfmt's order, so a regenerated file needs no `cargo fmt` after it.
        "use super::theme::{Theme, rgb};",
        "",
    ]
    names = sorted(p.stem for p in BEAR.glob("*.theme") if p.stem not in SKIP)
    out += [theme(n) + "\n" for n in names]
    out += [theme(n, prefix="BEAR_") + "\n" for n in sorted(SKIP)]
    sys.stdout.write("\n".join(out))


def manifest() -> None:
    """Name and SHA-256 of each theme file, in `shasum -a 256` format: what
    `config/themes/bear-themes.sha256` records of the files `palettes.rs` was
    generated from, without their contents."""
    for path in sorted(BEAR.glob("*.theme")):
        name = unicodedata.normalize("NFC", path.name)
        sys.stdout.write(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {name}\n")


if __name__ == "__main__":
    manifest() if sys.argv[1:] == ["--manifest"] else main()
