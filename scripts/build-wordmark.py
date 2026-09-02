# Build Codewhale wordmarks from Space Mono Bold outlines.
# Run: python3 scripts/build-wordmark.py <path-to-SpaceMono-Bold.ttf>
import re
import shutil
import sys
from pathlib import Path

from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

TEXT = "codewhale"
PAD = 4.0
TARGET_HEIGHT = 265.0

def number(value):
    value = round(value, 3)
    if value == 0:
        return "0"
    return f"{value:.3f}".rstrip("0").rstrip(".")

def rounded_path(path):
    pattern = r"[-+]?(?:\d*\.\d+|\d+)(?:[eE][-+]?\d+)?"
    return re.sub(pattern, lambda match: number(float(match.group())), path)

def main(font_path):
    font = TTFont(font_path)
    glyph_set = font.getGlyphSet()
    cmap = font.getBestCmap()
    hmtx = font["hmtx"]
    min_x = min_y = float("inf")
    max_x = max_y = float("-inf")
    x = 0
    for character in TEXT:
        glyph_name = cmap[ord(character)]
        pen = BoundsPen(glyph_set)
        glyph_set[glyph_name].draw(pen)
        left, bottom, right, top = pen.bounds
        min_x = min(min_x, x + left)
        max_x = max(max_x, x + right)
        min_y = min(min_y, bottom)
        max_y = max(max_y, top)
        x += hmtx[glyph_name][0]
    scale = (TARGET_HEIGHT - 2 * PAD) / (max_y - min_y)
    width = (max_x - min_x) * scale + 2 * PAD
    height = (max_y - min_y) * scale + 2 * PAD
    x_offset = PAD - min_x * scale
    baseline = PAD + max_y * scale
    paths = []
    x = 0
    for character in TEXT:
        glyph_name = cmap[ord(character)]
        pen = SVGPathPen(glyph_set)
        transformed = TransformPen(pen, (scale, 0, 0, -scale, x_offset + x * scale, baseline))
        glyph_set[glyph_name].draw(transformed)
        paths.append(pen.getCommands())
        x += hmtx[glyph_name][0]
    path = rounded_path(" ".join(paths))
    width_text, height_text = number(width), number(height)
    gradient = (f'<defs><linearGradient id="wordmark-gradient" x1="0" y1="0" '
                f'x2="{width_text}" y2="0" gradientUnits="userSpaceOnUse">'
                '<stop offset="0%" stop-color="#1535B2"/>'
                '<stop offset="100%" stop-color="#6AA6DC"/></linearGradient></defs>')
    svg_start = f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width_text} {height_text}">'
    gradient_svg = f'{svg_start}{gradient}<path fill="url(#wordmark-gradient)" fill-rule="evenodd" d="{path}"/></svg>'
    inverted_svg = f'{svg_start}<path fill="#ffffff" fill-rule="evenodd" d="{path}"/></svg>'
    root = Path(__file__).resolve().parent.parent
    (root / "brand/wordmark.svg").write_text(gradient_svg + "\n")
    (root / "brand/wordmark-inverted.svg").write_text(inverted_svg + "\n")
    shutil.copyfile(root / "brand/wordmark.svg", root / "web/public/brand/wordmark.svg")
    shutil.copyfile(root / "brand/wordmark-inverted.svg", root / "web/public/brand/wordmark-inverted.svg")
    print(f"viewBox: 0 0 {width_text} {height_text}")

if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: python3 scripts/build-wordmark.py <path-to-SpaceMono-Bold.ttf>")
    main(sys.argv[1])
