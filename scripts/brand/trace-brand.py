#!/usr/bin/env python3
"""Re-derive every brand surface from the founder's artwork in brand/.

    python3 scripts/brand/trace-brand.py            # rewrite every output
    python3 scripts/brand/trace-brand.py --check    # exit 1 if any output drifted

Founder sources (committed, never regenerated):

    brand/codewhalemarkfinal.png   1254x1254 sheet; hero mark top centre
    brand/wordmark0901.png         "codewhale" wordmark, navy on white, 2172x724
    brand/wordmarkinverted.png     the same wordmark, white on navy, 2508x627

`brand/mark.svg` is the kept trace of the sheet's hero mark (viewBox 0 0 512
512, one evenodd path, fill currentColor). It is the source of truth for the
mark: this script does not re-trace it, it derives every other mark surface
from it. The wordmark is traced from `wordmark0901.png` every run:

    magick brand/wordmark0901.png -colorspace Gray -threshold 60% -trim +repage wm.pbm
    potrace wm.pbm -s --flat -t 20 -O 0.4 -a 1.2 -o wm-raw.svg

potrace's `translate(0,H) scale(0.1,-0.1)` group is folded into the
coordinates so the result is one compact path in a tight pixel viewBox
(1874x264 for the 0901 render; the wordmark is ~7.1:1).

`wordmarkinverted.png` traces just as cleanly (negate first, threshold 40%),
but its render is set slightly tighter (2172x310 after trim, ~7.0:1), so the
0901 render is the one path used for both colourways: geometry that differs
between the light and dark wordmark is a second wordmark.

Outputs:

    brand/wordmark.svg               #142352 on transparent
    brand/wordmark-inverted.svg      #FFFFFF on transparent, same path
    brand/mark-navy.svg              mark.svg path, #142352
    brand/mark-gradient.svg          mark.svg path, cobalt->action gradient
    web/public/brand/{mark,mark-gradient,wordmark,wordmark-inverted}.svg
    web/app/icon.svg                 white mark on the #142352 rounded tile
    web/app/favicon.ico              16/32/48 of the tile
    web/app/apple-icon.png           180, opaque tile
    web/public/icon-192.png, icon-512.png

`web/components/whale.tsx` inlines the mark path; the script checks it still
matches `brand/mark.svg` rather than rewriting a React component.

Needs ImageMagick 7 (`magick`) and potrace on PATH. No network.
"""

from __future__ import annotations

import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BRAND = ROOT / "brand"
WEB = ROOT / "web"

NAVY = "#142352"
WHITE = "#FFFFFF"
# Gradient stops are the TUI's WHALE_COBALT -> WHALE_ACTION.
GRADIENT = ("#1535B2", "#6AA6DC")

# The mark in the 512 tile: the sheet's app icon sets the whale at ~2/3 of
# the tile width, so scale the 512 mark by 0.72 and centre its content box
# (x 16.9..494.3, y 46.7..471.1) on 256,256.
TILE_SCALE = 0.72
TILE_TRANSLATE = (72, 70)
TILE_RADIUS = 102.4  # 20% of 512, the sheet's corner

THRESHOLD = "60%"
POTRACE = ["potrace", "-s", "--flat", "-t", "20", "-O", "0.4", "-a", "1.2"]


def run(*args: str) -> None:
    subprocess.run(args, check=True)


def number(value: float) -> str:
    text = f"{value:.1f}"
    return text[:-2] if text.endswith(".0") else text


def fold_potrace(svg: str) -> tuple[str, float, float]:
    """Return (path d, width, height) with potrace's group transform applied."""
    width = float(re.search(r'width="([\d.]+)pt"', svg).group(1))
    height = float(re.search(r'height="([\d.]+)pt"', svg).group(1))
    tx, ty, sx, sy = (
        float(v)
        for v in re.search(
            r"translate\(([-\d.]+),([-\d.]+)\) scale\(([-\d.]+),([-\d.]+)\)", svg
        ).groups()
    )
    d = " ".join(re.findall(r'<path d="([^"]+)"', svg, re.S))
    tokens = re.findall(r"[MmLlCcZz]|-?\d+(?:\.\d+)?", d)
    arity = {"M": 2, "L": 2, "C": 6}
    out: list[str] = []
    emitted = ""
    cmd = ""
    i = 0
    while i < len(tokens):
        token = tokens[i]
        if token.isalpha():
            cmd = token
            i += 1
            if cmd in "Zz":
                out.append("Z")
                emitted = "Z"
            continue
        n = arity[cmd.upper()]
        nums = [float(v) for v in tokens[i : i + n]]
        i += n
        if cmd.isupper():
            pts = [(tx + sx * nums[k], ty + sy * nums[k + 1]) for k in range(0, n, 2)]
        else:
            pts = [(sx * nums[k], sy * nums[k + 1]) for k in range(0, n, 2)]
        if cmd != emitted:
            out.append(cmd)
            emitted = cmd
        out.append(" ".join(f"{number(x)} {number(y)}" for x, y in pts))
        # An implicit repeat after moveto is a lineto (SVG path grammar).
        if cmd in "Mm":
            cmd = "L" if cmd == "M" else "l"
    return " ".join(out), width, height


def trace_wordmark(png: Path, tmp: Path) -> tuple[str, float, float]:
    pbm = tmp / "wm.pbm"
    raw = tmp / "wm-raw.svg"
    run(
        "magick", str(png), "-colorspace", "Gray", "-threshold", THRESHOLD,
        "-trim", "+repage", str(pbm),
    )
    run(*POTRACE, str(pbm), "-o", str(raw))
    return fold_potrace(raw.read_text())


def wordmark_svg(d: str, width: float, height: float, fill: str) -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {number(width)} {number(height)}">'
        f'<path fill="{fill}" fill-rule="evenodd" d="{d}"/></svg>\n'
    )


def mark_path() -> str:
    return re.search(r' d="([^"]+)"', (BRAND / "mark.svg").read_text()).group(1)


def mark_svg(fill: str, defs: str = "") -> str:
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512">'
        f'{defs}<path fill="{fill}" fill-rule="evenodd" d="{mark_path()}"/></svg>\n'
    )


def icon_svg() -> str:
    tx, ty = TILE_TRANSLATE
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512">'
        f'<rect width="512" height="512" rx="{TILE_RADIUS}" fill="{NAVY}"/>'
        f'<path transform="translate({tx} {ty}) scale({TILE_SCALE})" fill="{WHITE}" '
        f'fill-rule="evenodd" d="{mark_path()}"/></svg>\n'
    )


def rasters(icon: Path) -> None:
    run(
        "magick", "-background", "none", str(icon),
        "-define", "icon:auto-resize=48,32,16", str(WEB / "app/favicon.ico"),
    )
    run(
        "magick", "-background", NAVY, str(icon), "-resize", "180x180",
        "-alpha", "off", "-depth", "8", str(WEB / "app/apple-icon.png"),
    )
    for size in (192, 512):
        run(
            "magick", "-background", "none", str(icon), "-resize", f"{size}x{size}",
            "-depth", "8", str(WEB / f"public/icon-{size}.png"),
        )


def main() -> int:
    check = "--check" in sys.argv[1:]
    gradient = (
        '<defs><linearGradient id="mark-gradient" x1="0" y1="0" x2="512" y2="512" '
        'gradientUnits="userSpaceOnUse">'
        f'<stop offset="0%" stop-color="{GRADIENT[0]}"/>'
        f'<stop offset="100%" stop-color="{GRADIENT[1]}"/></linearGradient></defs>'
    )
    with tempfile.TemporaryDirectory() as tmpdir:
        tmp = Path(tmpdir)
        d, width, height = trace_wordmark(BRAND / "wordmark0901.png", tmp)
        texts = {
            BRAND / "wordmark.svg": wordmark_svg(d, width, height, NAVY),
            BRAND / "wordmark-inverted.svg": wordmark_svg(d, width, height, WHITE),
            BRAND / "mark-navy.svg": mark_svg(NAVY),
            BRAND / "mark-gradient.svg": mark_svg("url(#mark-gradient)", gradient),
            WEB / "app/icon.svg": icon_svg(),
        }
        texts[WEB / "public/brand/mark.svg"] = (BRAND / "mark.svg").read_text()
        for name in ("mark-gradient", "wordmark", "wordmark-inverted"):
            texts[WEB / "public/brand" / f"{name}.svg"] = texts[BRAND / f"{name}.svg"]

        whale = (WEB / "components/whale.tsx").read_text()
        inline = re.search(r'WHALE_MARK =\s*"([^"]+)"', whale).group(1)
        drift = [] if inline == mark_path() else ["web/components/whale.tsx"]

        if check:
            drift += [
                str(path.relative_to(ROOT))
                for path, text in texts.items()
                if not path.exists() or path.read_text() != text
            ]
            for name in drift:
                print(f"drift: {name}", file=sys.stderr)
            return 1 if drift else 0

        for path, text in texts.items():
            path.write_text(text)
            print(f"wrote {path.relative_to(ROOT)}")
        rasters(WEB / "app/icon.svg")
        print("wrote web/app/favicon.ico apple-icon.png public/icon-192.png icon-512.png")
        for name in drift:
            print(f"drift: {name} no longer matches brand/mark.svg", file=sys.stderr)
        return 1 if drift else 0


if __name__ == "__main__":
    if not shutil.which("magick") or not shutil.which("potrace"):
        sys.exit("needs `magick` (ImageMagick 7) and `potrace` on PATH")
    sys.exit(main())
