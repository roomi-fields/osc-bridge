"""Generate the osc-bridge logo — a 400x400 PNG mark.

A bridge: two side nodes linked through a central core, on the same dark
palette as the README demo. Pure mark, no text — reads at any size, used
by the Cline Marketplace listing and the Smithery icon.

Run from the repo root:
    python3 assets/make_logo.py
Output:
    assets/osc-bridge-logo-400.png
"""
from __future__ import annotations
import math
from pathlib import Path
from PIL import Image, ImageDraw

S = 400  # square

# Palette — matches docs/demo/make_demo.py
BG    = (24, 25, 38)
TILE  = (32, 34, 52)
WIRE  = (70, 75, 110)
CORE  = (122, 162, 247)   # blue — the bridge
MIDI  = (255, 158, 100)   # amber — hardware side
OSC   = (158, 206, 106)   # green — software side
GLOW  = (40, 43, 64)


def mix(c1, c2, t):
    return tuple(int(c1[i] + (c2[i] - c1[i]) * t) for i in range(3))


def main():
    # Render at 4x then downsample — clean anti-aliasing on the curves.
    scale = 4
    s = S * scale
    img = Image.new("RGB", (s, s), BG)
    d = ImageDraw.Draw(img)

    def px(v):
        return int(v * scale)

    # Rounded tile background
    m = px(18)
    d.rounded_rectangle([m, m, s - m, s - m], radius=px(72), fill=TILE)

    cy = s / 2
    cx = s / 2
    side_dx = px(116)        # horizontal distance of the side nodes from centre
    core_r = px(46)          # half-size of the central rounded square
    node_r = px(26)          # radius of the side nodes

    left = (cx - side_dx, cy)
    right = (cx + side_dx, cy)

    # Wires (under the nodes)
    d.line([left, (cx, cy)], fill=WIRE, width=px(9))
    d.line([(cx, cy), right], fill=WIRE, width=px(9))

    # Travelling packet dots on the wires — a couple per side
    for (a, b), col in [((left, (cx, cy)), MIDI), (((cx, cy), right), OSC)]:
        for t in (0.40, 0.78):
            x = a[0] + (b[0] - a[0]) * t
            y = a[1] + (b[1] - a[1]) * t
            d.ellipse([x - px(11), y - px(11), x + px(11), y + px(11)],
                      fill=mix(col, BG, 0.78))
            d.ellipse([x - px(6), y - px(6), x + px(6), y + px(6)], fill=col)

    # Side nodes — amber (hardware) left, green (software) right
    for (x, y), col in [(left, MIDI), (right, OSC)]:
        d.ellipse([x - node_r - px(7), y - node_r - px(7),
                   x + node_r + px(7), y + node_r + px(7)], fill=mix(col, BG, 0.80))
        d.ellipse([x - node_r, y - node_r, x + node_r, y + node_r], fill=col)

    # Central core — a rounded square, the bridge itself
    glow = px(14)
    d.rounded_rectangle([cx - core_r - glow, cy - core_r - glow,
                         cx + core_r + glow, cy + core_r + glow],
                        radius=px(26), fill=mix(CORE, TILE, 0.78))
    d.rounded_rectangle([cx - core_r, cy - core_r, cx + core_r, cy + core_r],
                        radius=px(18), fill=CORE)
    # inner notch — a small dark rounded square, gives the core a "hub" read
    inr = px(17)
    d.rounded_rectangle([cx - inr, cy - inr, cx + inr, cy + inr],
                        radius=px(7), fill=mix(CORE, BG, 0.62))

    img = img.resize((S, S), Image.LANCZOS)
    out = Path(__file__).parent / "osc-bridge-logo-400.png"
    img.save(out, optimize=True)
    print(f"Generated: {out}  ({S}x{S}, {out.stat().st_size / 1024:.0f} KB)")


if __name__ == "__main__":
    main()
