#!/usr/bin/env python3
"""Preview archived Codewhale empty-state portrait art: before vs after.

This script is historical comparison evidence only. It is not a preview of the
current Codewhale mark: the canonical mark is a raster asset with no approved
ASCII or block-glyph substitute. The historical rows are held below as a fixed record because the live
`underwater.rs` constants were intentionally deleted with the old artwork.

    python3 scripts/preview-whale-art.py            # ANSI true colour
    python3 scripts/preview-whale-art.py --plain    # no escape codes
    python3 scripts/preview-whale-art.py --png out.png   # if Pillow exists

Colours are the real palette tokens: Signal Gold #F6C453 body, Signal Current
#48D7FF spout and belly cut, ivory eye, on the whale theme's #03070D surface.
"""

from __future__ import annotations

import argparse

GOLD = (246, 196, 83)  # WHALE_HUMAN_RGB  — Signal Gold
CYAN = (72, 215, 255)  # WHALE_CYAN_RGB   — Signal Current
IVORY = (233, 238, 245)  # text_body
SAKURA = (247, 168, 200)  # uwu accent_primary
SEAFOAM = (79, 209, 197)  # whale theme accent_secondary (the old spout)
SURFACE = (3, 7, 13)  # whale theme surface_bg

# The art this branch replaced, kept verbatim so the PR is reviewable.
BEFORE = {
    "classic": ("   ˚", [
        "  ▗▄▄▄▄▄▄▄▄▄▄▄▖      ▚△▞",
        " ▐██·███████████▙━━━━▞",
        "  ▝▀▀▀▀▀▀▀▀▀▀▀▀▘",
    ]),
    "uwu": ("   ˚✦", [
        " ▗▄▄▄▄▄▄▄▄▄▄▄▖    ▚△▞",
        "▐█░·░█████████▙▄▄▞",
        " ▝▀▀▀▀▀▀▀▀▀▀▀▘",
    ]),
}

# The former Signal Cut revision, kept as a static record. It must not be
# inferred from live source: the runtime portrait constants were deliberately
# removed when the product mark changed.
AFTER = {
    "classic": ("    ˚", [
        "  ▗▄▄▟▄▄▄▄▄▖  ▚△▞",
        " ▐█·████████▙▄▄▞",
        "  ▝▀▀▀▀▀▀▀▀▘",
    ]),
    "uwu": ("    ˚✦", [
        "  ▗▄▄▟▄▄▄▄▖  ▚△▞",
        " ▐█░·░█████▙▄▄▞",
        "  ▝▀▀▀▀▀▀▀▘",
    ]),
}

# Same table the TUI narrows through (`glyphs::ascii_fallback`).
ASCII_FALLBACK = {
    "━": "-", "▐": "|", "█": "#", "▄": "#", "▟": "#",
    "▙": "#", "▀": "#", "▗": ".", "▖": ".", "▝": ".",
    "▘": ".", "▚": "\\", "▞": "/", "░": ":", "△": "^",
    "·": ".", "˚": "o", "✦": "*",
}

CURRENT_ROW = 2  # IDLE_WHALE_CURRENT_ROW


def ink(variant: str, row: int, ch: str, old: bool = False) -> tuple[int, int, int]:
    """The colour the TUI resolves for one glyph, without the caustic sweep."""
    if ch in "·░✦△":
        return SAKURA if variant == "uwu" else IVORY
    if old:
        # main: spout took the theme's secondary accent, the belly was body gold.
        return SEAFOAM if row < 0 else GOLD
    if row < 0:
        return CYAN  # spout row
    if row == CURRENT_ROW:
        return CYAN  # belly cut
    return GOLD


def paint(variant: str, spout: str, body: list[str], colour: bool, old: bool = False) -> list[str]:
    out = []
    for row, text in enumerate([spout] + body, start=-1):
        if not colour:
            out.append(text)
            continue
        line = f"\x1b[48;2;{SURFACE[0]};{SURFACE[1]};{SURFACE[2]}m"
        for ch in text:
            r, g, b = ink(variant, row, ch, old)
            line += f"\x1b[38;2;{r};{g};{b}m{ch}"
        out.append(line + "\x1b[0m")
    return out


def ascii_rows(spout: str, body: list[str]) -> list[str]:
    return ["".join(ASCII_FALLBACK.get(ch, ch) for ch in row) for row in [spout] + body]


def block(title: str, rows: list[str]) -> None:
    print(f"  {title}")
    for row in rows:
        print(f"    {row}")
    print()


# Block-element geometry as (x0, y0, x1, y1) fractions of one terminal cell.
# Terminals scale these to fill the cell exactly; font rasterisers do not, so
# the PNG draws them itself and keeps the font only for the small marks.
CELL_SHAPES: dict[str, tuple[tuple[float, float, float, float], ...]] = {
    "█": ((0, 0, 1, 1),),
    "▀": ((0, 0, 1, 0.5),),
    "▄": ((0, 0.5, 1, 1),),
    "▌": ((0, 0, 0.5, 1),),
    "▐": ((0.5, 0, 1, 1),),
    "▘": ((0, 0, 0.5, 0.5),),
    "▝": ((0.5, 0, 1, 0.5),),
    "▖": ((0, 0.5, 0.5, 1),),
    "▗": ((0.5, 0.5, 1, 1),),
    "▙": ((0, 0, 0.5, 1), (0.5, 0.5, 1, 1)),
    "▟": ((0.5, 0, 1, 0.5), (0, 0.5, 1, 1)),
    "▚": ((0, 0, 0.5, 0.5), (0.5, 0.5, 1, 1)),
    "▞": ((0.5, 0, 1, 0.5), (0, 0.5, 0.5, 1)),
    "━": ((0, 0.42, 1, 0.6),),
    "·": ((0.34, 0.42, 0.66, 0.6),),
    "˚": ((0.3, 0.16, 0.7, 0.42),),
    "░": ((0, 0, 1, 1),),
}
# Marks the cell grammar draws as outlines or points rather than fills.
RING = {"˚": (0.3, 0.16, 0.7, 0.44)}
TRIANGLE = {"△": (0.22, 0.2, 0.78, 0.76)}
STAR = {"✦": (0.5, 0.35, 0.26)}
SHADED = {"░"}


def render_png(after: dict[str, tuple[str, list[str]]], path: str) -> None:
    """A terminal-cell mock of BEFORE vs AFTER, drawn as geometry."""
    try:
        from PIL import Image, ImageDraw  # noqa: PLC0415
    except ImportError:
        print(f"! Pillow is not installed; skipping {path}", file=sys.stderr)
        return

    cell_w, cell_h, pad = 18, 34, 30
    panels = [
        ("BEFORE  ·  main", BEFORE["classic"], True),
        ("ARCHIVED AFTER  ·  Signal Cut", after["classic"], False),
    ]
    cols = max(len(row) for _, (spout, body), _ in panels for row in [spout] + body)
    width = pad * 2 + cols * cell_w
    height = pad + len(panels) * (cell_h * 6 + pad)
    img = Image.new("RGB", (width, height), SURFACE)
    draw = ImageDraw.Draw(img, "RGBA")

    y = pad
    for title, (spout, body), old in panels:
        draw.text((pad, y + cell_h // 3), title, fill=(120, 138, 158))
        y += cell_h * 2
        for row, text in enumerate([spout] + body, start=-1):
            top = y + (row + 1) * cell_h
            for col, ch in enumerate(text):
                if ch == " ":
                    continue
                left = pad + col * cell_w
                colour = ink("classic", row, ch, old)
                cell = lambda x0, y0, x1, y1: (  # noqa: E731
                    left + x0 * cell_w,
                    top + y0 * cell_h,
                    left + x1 * cell_w - 1,
                    top + y1 * cell_h - 1,
                )
                if ch in RING:
                    draw.ellipse(cell(*RING[ch]), outline=colour, width=2)
                elif ch in TRIANGLE:
                    x0, y0, x1, y1 = cell(*TRIANGLE[ch])
                    draw.polygon([((x0 + x1) / 2, y0), (x1, y1), (x0, y1)], outline=colour, width=2)
                elif ch in STAR:
                    cx, cy, r = STAR[ch]
                    x, y0 = left + cx * cell_w, top + cy * cell_h
                    draw.polygon(
                        [(x, y0 - r * cell_h), (x + r * cell_w, y0),
                         (x, y0 + r * cell_h), (x - r * cell_w, y0)],
                        fill=colour,
                    )
                else:
                    alpha = 90 if ch in SHADED else 255
                    for shape in CELL_SHAPES.get(ch, ()):
                        draw.rectangle(cell(*shape), fill=(*colour, alpha))
        y += cell_h * 4 + pad
    img.save(path)
    print(f"wrote {path}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plain", action="store_true", help="no ANSI colour")
    parser.add_argument("--png", metavar="PATH", help="also write a PNG (needs Pillow)")
    args = parser.parse_args()
    colour = not args.plain and sys.stdout.isatty()

    after = AFTER
    print("ARCHIVED: former Codewhale empty-state portrait — main vs Signal Cut\n")
    for variant in ("classic", "uwu"):
        label = "classic" if variant == "classic" else "uwu"
        print(f"== {label} ==\n")
        before_spout, before_body = BEFORE[variant]
        after_spout, after_body = after[variant]
        block("BEFORE", paint(variant, before_spout, before_body, colour, old=True))
        block("AFTER", paint(variant, after_spout, after_body, colour))
        block("AFTER, CODEWHALE_ASCII_SAFE=1", ascii_rows(after_spout, after_body))
        widths = (
            max(len(r) for r in [before_spout] + before_body),
            max(len(r) for r in [after_spout] + after_body),
        )
        print(f"  block width: {widths[0]} -> {widths[1]} columns\n")

    if args.png:
        render_png(after, args.png)


if __name__ == "__main__":
    main()
