#!/usr/bin/env python3
"""Render brand/mark.svg as braille cells (2x4 dots per cell) for the TUI.

The launch mark in `crates/tui/src/tui/mark.rs` is generated here, never
hand-drawn (shell design §2.0 item 4: "the mark is the real logo, in dots").

Pipeline: substitute `currentColor`, rasterise with ImageMagick at a high
density, crop to the glyph's bounding box, box-filter the alpha coverage down
to a (cols*2) x (rows*4) dot grid that preserves the glyph's aspect ratio
(centred inside the box), threshold, and pack each 2x4 block into one braille
codepoint (U+2800 + dot bits). Blank cells are emitted as a space so the
renderer can leave the field behind them untouched, and all-blank edge
columns are trimmed so the emitted footprint is the ink's, not the box's.

    scripts/brand/braille-mark.py                   # print + Rust consts
    scripts/brand/braille-mark.py --png out.png --px 96 --color 5B9BFF

The `--png` form writes the same glyph as a coloured PNG on a transparent
ground for the kitty-graphics tier (`crates/tui/assets/mark-*.png`).

Requires `magick` (ImageMagick 7). No other dependencies.
"""

from __future__ import annotations

import argparse
import pathlib
import re
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
SVG = ROOT / "brand" / "mark.svg"

# Braille dot bit for (dot_row, dot_col) inside one cell — U+2800 layout:
# dots 1,2,3 are column 0 rows 0..2 (bits 0..2), dots 4,5,6 column 1 rows
# 0..2 (bits 3..5), dots 7,8 are row 3 (bits 6,7).
DOT_BITS = {
    (0, 0): 0x01,
    (1, 0): 0x02,
    (2, 0): 0x04,
    (0, 1): 0x08,
    (1, 1): 0x10,
    (2, 1): 0x20,
    (3, 0): 0x40,
    (3, 1): 0x80,
}


def svg_with_fill(color: str) -> bytes:
    text = SVG.read_text(encoding="utf-8")
    return text.replace("currentColor", color).encode("utf-8")


def rasterise_alpha(density: int) -> tuple[int, int, list[list[float]]]:
    """Return (width, height, coverage[y][x] in 0..1) of the trimmed glyph."""
    with tempfile.TemporaryDirectory() as tmp:
        svg = pathlib.Path(tmp) / "mark.svg"
        svg.write_bytes(svg_with_fill("#000000"))
        pgm = subprocess.run(
            [
                "magick",
                "-background",
                "none",
                "-density",
                str(density),
                str(svg),
                "-trim",
                "+repage",
                "-alpha",
                "extract",
                "-compress",
                "none",
                "pgm:-",
            ],
            check=True,
            capture_output=True,
        ).stdout
    tokens = re.split(rb"\s+", pgm.strip())
    if tokens[0] != b"P2":
        raise SystemExit("magick did not emit an ASCII PGM")
    width, height, maxval = int(tokens[1]), int(tokens[2]), int(tokens[3])
    values = [int(v) / maxval for v in tokens[4 : 4 + width * height]]
    rows = [values[y * width : (y + 1) * width] for y in range(height)]
    return width, height, rows


def downsample(
    coverage: list[list[float]], width: int, height: int, dots_w: int, dots_h: int
) -> list[list[float]]:
    """Box-filter coverage into a dots_w x dots_h grid, aspect preserved and
    centred. Cells outside the glyph read 0."""
    scale = min(dots_w / width, dots_h / height)
    glyph_w = max(1, round(width * scale))
    glyph_h = max(1, round(height * scale))
    off_x = (dots_w - glyph_w) // 2
    off_y = (dots_h - glyph_h) // 2
    grid = [[0.0] * dots_w for _ in range(dots_h)]
    for gy in range(glyph_h):
        y0 = int(gy * height / glyph_h)
        y1 = max(y0 + 1, int((gy + 1) * height / glyph_h))
        for gx in range(glyph_w):
            x0 = int(gx * width / glyph_w)
            x1 = max(x0 + 1, int((gx + 1) * width / glyph_w))
            total = 0.0
            for y in range(y0, min(y1, height)):
                row = coverage[y]
                for x in range(x0, min(x1, width)):
                    total += row[x]
            grid[off_y + gy][off_x + gx] = total / ((y1 - y0) * (x1 - x0))
    return grid


def eye_hole(coverage: list[list[float]], width: int, height: int) -> tuple[float, float] | None:
    """Locate the eye: the smallest enclosed hole in the glyph (the belly
    lines are the other holes, but they are long). Returns its centroid as a
    fraction of the glyph's width and height, or None when nothing is
    enclosed. Works on a coarse copy so the flood fill stays cheap."""
    scale = max(1, width // 220)
    cw, ch = width // scale, height // scale
    solid = [
        [coverage[y * scale][x * scale] >= 0.5 for x in range(cw)] for y in range(ch)
    ]
    seen = [[False] * cw for _ in range(ch)]
    holes = []
    for sy in range(ch):
        for sx in range(cw):
            if solid[sy][sx] or seen[sy][sx]:
                continue
            stack, cells, touches_edge = [(sx, sy)], [], False
            seen[sy][sx] = True
            while stack:
                x, y = stack.pop()
                cells.append((x, y))
                if x in (0, cw - 1) or y in (0, ch - 1):
                    touches_edge = True
                for nx, ny in ((x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)):
                    if 0 <= nx < cw and 0 <= ny < ch and not solid[ny][nx] and not seen[ny][nx]:
                        seen[ny][nx] = True
                        stack.append((nx, ny))
            if not touches_edge:
                holes.append(cells)
    if not holes:
        return None
    eye = min(holes, key=len)
    cx = sum(x for x, _ in eye) / len(eye) + 0.5
    cy = sum(y for _, y in eye) / len(eye) + 0.5
    return cx / cw, cy / ch


def carve_eye(
    grid: list[list[float]], width: int, height: int, dots_w: int, dots_h: int, eye: tuple[float, float]
) -> None:
    """Clear the one dot under the eye's centroid so the eye survives rungs
    where it is smaller than a dot. Same geometry as `downsample`."""
    scale = min(dots_w / width, dots_h / height)
    glyph_w = max(1, round(width * scale))
    glyph_h = max(1, round(height * scale))
    off_x = (dots_w - glyph_w) // 2
    off_y = (dots_h - glyph_h) // 2
    x = min(dots_w - 1, off_x + int(eye[0] * glyph_w))
    y = min(dots_h - 1, off_y + int(eye[1] * glyph_h))
    grid[y][x] = 0.0


def to_braille(grid: list[list[float]], cols: int, rows: int, threshold: float) -> list[str]:
    lines = []
    for cy in range(rows):
        line = []
        for cx in range(cols):
            bits = 0
            for (dy, dx), bit in DOT_BITS.items():
                if grid[cy * 4 + dy][cx * 2 + dx] >= threshold:
                    bits |= bit
            line.append(chr(0x2800 + bits) if bits else " ")
        lines.append("".join(line))
    return lines


def trim_columns(lines: list[str]) -> list[str]:
    width = max(len(line) for line in lines)
    padded = [line.ljust(width) for line in lines]
    blank = [all(line[x] == " " for line in padded) for x in range(width)]
    first = next((x for x in range(width) if not blank[x]), 0)
    last = next((x for x in range(width - 1, -1, -1) if not blank[x]), width - 1)
    return [line[first : last + 1] for line in padded]


def rust_const(name: str, lines: list[str]) -> str:
    body = "\n".join(f'    "{line}",' for line in lines)
    return f"const {name}: [&str; {len(lines)}] = [\n{body}\n];"


def write_png(out: pathlib.Path, px: int, color: str) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        svg = pathlib.Path(tmp) / "mark.svg"
        svg.write_bytes(svg_with_fill(f"#{color}"))
        subprocess.run(
            [
                "magick",
                "-background",
                "none",
                "-density",
                "600",
                str(svg),
                "-trim",
                "+repage",
                "-resize",
                f"{px}x{px}",
                "-gravity",
                "center",
                "-extent",
                f"{px}x{px}",
                "-depth",
                "8",
                "-define",
                "png:color-type=6",
                "-strip",
                str(out),
            ],
            check=True,
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument(
        "--rung",
        action="append",
        default=None,
        metavar="NAME:COLSxROWS",
        help="cell box to render, e.g. SMALL:11x3 (default: MEDIUM:22x5 SMALL:11x3 TINY:8x2)",
    )
    parser.add_argument("--threshold", type=float, default=0.3, help="dot coverage threshold")
    parser.add_argument("--density", type=int, default=400, help="rasterisation DPI")
    parser.add_argument("--no-eye", action="store_true", help="do not carve the eye dot")
    parser.add_argument("--png", type=pathlib.Path, help="write a coloured PNG instead")
    parser.add_argument("--px", type=int, default=96, help="PNG edge in pixels")
    parser.add_argument("--color", default="5B9BFF", help="PNG fill colour (hex, no #)")
    args = parser.parse_args()

    if args.png:
        write_png(args.png, args.px, args.color)
        print(f"wrote {args.png} ({args.px}x{args.px}, #{args.color})")
        return 0

    rungs = args.rung or ["MEDIUM:22x5", "SMALL:11x3", "TINY:8x2"]
    width, height, coverage = rasterise_alpha(args.density)
    eye = None if args.no_eye else eye_hole(coverage, width, height)
    print(f"// generated by scripts/brand/braille-mark.py from brand/mark.svg")
    print(
        f"// (density {args.density}, glyph bbox {width}x{height}px, "
        f"threshold {args.threshold}, aspect preserved, edge columns trimmed, "
        f"eye {'carved' if eye else 'not found'})"
    )
    for spec in rungs:
        name, box = spec.split(":")
        cols, rows = (int(v) for v in box.lower().split("x"))
        grid = downsample(coverage, width, height, cols * 2, rows * 4)
        if eye is not None:
            carve_eye(grid, width, height, cols * 2, rows * 4, eye)
        lines = trim_columns(to_braille(grid, cols, rows, args.threshold))
        print(f"\n// {name}: box {cols}x{rows} -> ink {len(lines[0])}x{len(lines)}")
        print(rust_const(f"{name}_ROWS", lines))
        print("//", file=sys.stderr)
        for line in lines:
            print(f"//  |{line}|", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
