"""Generate the animated hero graphic for osc-bridge.

A designed (not terminal-recorded) animation: clients on the left feed one
osc-bridge core, which fans out to hardware synths (MIDI/SysEx) and music
software (OSC). Packets flow along the wires; the loop is seamless.

Output is an **APNG** (`.png` extension, written by Pillow with
`save_all=True`). GitHub renders APNG inline and auto-plays it — no video
overlay, no play button. Same format the RTFM project landed on.

Run from the repo root:
    python3 docs/demo/make_demo.py
Output:
    docs/demo/osc-bridge-demo.png
"""
from __future__ import annotations
import math
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

# ── Canvas ──────────────────────────────────────────────────────────────
W, H = 960, 470

# ── Palette (Tokyo Night-ish, matches the Pages site theme family) ──────
BG       = (24, 25, 38)
PANEL    = (32, 34, 52)
PANEL_HI = (40, 43, 64)
FG       = (192, 202, 245)
DIM      = (110, 118, 160)
WIRE     = (58, 62, 92)
CORE     = (122, 162, 247)   # the bridge — blue
MCP      = (187, 154, 247)   # MCP / Claude — purple
MIDI     = (255, 158, 100)   # hardware side — amber
OSC      = (158, 206, 106)   # software side — green
WHITE    = (235, 240, 255)

# ── Fonts ───────────────────────────────────────────────────────────────
def font(size, bold=False):
    base = "/usr/share/fonts/truetype/dejavu/"
    name = "DejaVuSans-Bold.ttf" if bold else "DejaVuSans.ttf"
    p = Path(base + name)
    return ImageFont.truetype(str(p), size) if p.exists() else ImageFont.load_default()

F_TITLE = font(30, bold=True)
F_SUB   = font(14)
F_BOX   = font(15, bold=True)
F_SMALL = font(11)
F_CORE  = font(20, bold=True)
F_FOOT  = font(13, bold=True)

# ── Animation ───────────────────────────────────────────────────────────
FPS = 14
N_FRAMES = 42                       # 3.0 s, seamless loop
DOTS_PER_WIRE = 3

# ── Geometry ────────────────────────────────────────────────────────────
# Left: 3 source boxes. Center: the bridge core. Right: 2 target boxes.
BOX_W_L, BOX_H_L = 210, 74
BOX_W_R, BOX_H_R = 232, 92
CORE_W, CORE_H   = 196, 150

LX = 36
CX = (W - CORE_W) // 2
RX = W - BOX_W_R - 36
CORE_Y = (H - CORE_H) // 2 + 6

# (label, detail, accent, y-center) — detail strings kept short enough to
# clear the box padding at F_SMALL (≈30 chars left, ≈33 right).
SOURCES = [
    ("OSC clients", "SuperCollider · Max · Python …", FG,  150),
    ("CLI",         "run · inspect · orchestrate",    FG,  240),
    ("LLM via MCP", "Claude · Cursor · MCP stdio",    MCP, 330),
]
TARGETS = [
    ("Hardware synths",    "MIDI / SysEx · Moog · Prophet …", MIDI, 188),
    ("DAWs & live-coding", "OSC · Ableton · Bitwig · Reaper", OSC,  312),
]

def src_box(i):
    _, _, _, cy = SOURCES[i]
    return (LX, cy - BOX_H_L // 2, LX + BOX_W_L, cy + BOX_H_L // 2)

def tgt_box(i):
    _, _, _, cy = TARGETS[i]
    return (RX, cy - BOX_H_R // 2, RX + BOX_W_R, cy + BOX_H_R // 2)

CORE_BOX = (CX, CORE_Y, CX + CORE_W, CORE_Y + CORE_H)

# ── Drawing helpers ─────────────────────────────────────────────────────
def lerp(a, b, t):
    return a + (b - a) * t

def rounded(d, box, fill, outline=None, width=1, radius=12):
    d.rounded_rectangle(box, radius=radius, fill=fill, outline=outline, width=width)

def text_c(d, cx, cy, s, fnt, fill):
    bb = d.textbbox((0, 0), s, font=fnt)
    d.text((cx - (bb[2] - bb[0]) / 2, cy - (bb[3] - bb[1]) / 2), s, font=fnt, fill=fill)

def wire(d, p0, p1, color=WIRE, width=2):
    d.line([p0, p1], fill=color, width=width)

def dot(d, p0, p1, t, color):
    """A travelling packet: bright core + faint halo, at parametric t on p0->p1."""
    x = lerp(p0[0], p1[0], t)
    y = lerp(p0[1], p1[1], t)
    # halo
    d.ellipse([x - 7, y - 7, x + 7, y + 7], fill=(*color, 60) if False else _mix(color, BG, 0.78))
    # core
    d.ellipse([x - 3.2, y - 3.2, x + 3.2, y + 3.2], fill=color)

def _mix(c1, c2, t):
    return tuple(int(lerp(c1[i], c2[i], t)) for i in range(3))

# ── Frame ───────────────────────────────────────────────────────────────
def make_frame(frame):
    phase = frame / N_FRAMES                       # 0..1, wraps seamlessly
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)

    # Title + subtitle
    text_c(d, W / 2, 34, "osc-bridge", F_TITLE, WHITE)
    text_c(d, W / 2, 62, "one named OSC surface for 849 synths and DAWs — and an MCP server",
           F_SUB, DIM)

    core_l = (CORE_BOX[0], CORE_Y + CORE_H / 2)
    core_r = (CORE_BOX[2], CORE_Y + CORE_H / 2)

    # Wires (under everything)
    for i in range(len(SOURCES)):
        b = src_box(i)
        wire(d, (b[2], (b[1] + b[3]) / 2), core_l)
    for i in range(len(TARGETS)):
        b = tgt_box(i)
        wire(d, core_r, (b[0], (b[1] + b[3]) / 2))

    # Travelling packets — inputs (source -> core)
    for i, (_, _, accent, _) in enumerate(SOURCES):
        b = src_box(i)
        p0 = (b[2], (b[1] + b[3]) / 2)
        col = MCP if accent is MCP else CORE
        for k in range(DOTS_PER_WIRE):
            t = (phase + k / DOTS_PER_WIRE) % 1.0
            dot(d, p0, core_l, t, col)
    # Travelling packets — outputs (core -> target)
    for i, (_, _, accent, _) in enumerate(TARGETS):
        b = tgt_box(i)
        p1 = (b[0], (b[1] + b[3]) / 2)
        for k in range(DOTS_PER_WIRE):
            t = (phase + k / DOTS_PER_WIRE) % 1.0
            dot(d, core_r, p1, t, accent)

    # Source boxes
    for i, (label, detail, accent, cy) in enumerate(SOURCES):
        b = src_box(i)
        rounded(d, b, PANEL, outline=PANEL_HI, width=1)
        d.text((b[0] + 16, b[1] + 13), label, font=F_BOX,
               fill=(MCP if accent is MCP else FG))
        d.text((b[0] + 16, b[1] + 39), detail, font=F_SMALL, fill=DIM)

    # Core — subtle pulse on the outline, seamless (period = N_FRAMES)
    pulse = 0.5 + 0.5 * math.sin(2 * math.pi * phase)
    core_outline = _mix(WIRE, CORE, 0.35 + 0.55 * pulse)
    rounded(d, CORE_BOX, PANEL_HI, outline=core_outline, width=3, radius=16)
    cx = CX + CORE_W / 2
    text_c(d, cx, CORE_Y + 40, "osc-bridge", F_CORE, CORE)
    text_c(d, cx, CORE_Y + 72, "849 device drivers", F_SMALL, FG)
    text_c(d, cx, CORE_Y + 90, "one JSON = one driver", F_SMALL, DIM)
    text_c(d, cx, CORE_Y + 112, "rate-limited · bidirectional", F_SMALL, DIM)

    # Target boxes
    for i, (label, detail, accent, cy) in enumerate(TARGETS):
        b = tgt_box(i)
        rounded(d, b, PANEL, outline=_mix(PANEL_HI, accent, 0.45), width=2)
        d.text((b[0] + 16, b[1] + 16), label, font=F_BOX, fill=accent)
        # detail can be long — wrap to two lines on the bullet
        d.text((b[0] + 16, b[1] + 44), detail, font=F_SMALL, fill=DIM)
        d.text((b[0] + 16, b[1] + 62),
               "tested live" if accent is OSC else "verified on hardware",
               font=F_SMALL, fill=_mix(accent, BG, 0.35))

    # Footer
    text_c(d, W / 2, H - 26,
           "hardware + software   ·   MCP   ·   GPL-3   ·   npx -y @roomi-fields/osc-bridge",
           F_FOOT, _mix(FG, MCP, 0.4))

    return img

# ── Main ────────────────────────────────────────────────────────────────
def main():
    out = Path(__file__).parent / "osc-bridge-demo.png"
    frames = [make_frame(i) for i in range(N_FRAMES)]
    durations = [int(1000 / FPS)] * N_FRAMES
    frames[0].save(
        out, save_all=True, append_images=frames[1:],
        duration=durations, loop=0, optimize=True,
    )
    kb = out.stat().st_size / 1024
    print(f"Generated: {out}  ({N_FRAMES} frames @ {FPS}fps, {N_FRAMES / FPS:.1f}s, {kb:.0f} KB)")

if __name__ == "__main__":
    main()
