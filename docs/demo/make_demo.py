"""Generate the animated hero graphic for osc-bridge.

A designed (not terminal-recorded) animation: clients on the left and one
osc-bridge core that fans out to hardware synths (MIDI/SysEx) and music
software (OSC). Every main wire is **bidirectional** — commands flow out,
decoded events / replies flow back. A dedicated arc shows the orchestrator's
`[[routes]]`: a hardware knob can drive a DAW directly.

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
WIRE     = (54, 58, 86)
CORE     = (122, 162, 247)   # the bridge — blue
MCP      = (187, 154, 247)   # MCP / Claude — purple
MIDI     = (255, 158, 100)   # hardware side — amber
OSC      = (158, 206, 106)   # software side — green
ROUTE    = (224, 175, 104)   # [[routes]] cross-link — gold
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
F_TINY  = font(10, bold=True)
F_CORE  = font(20, bold=True)
F_FOOT  = font(13, bold=True)

# ── Animation ───────────────────────────────────────────────────────────
FPS = 14
N_FRAMES = 42                       # 3.0 s, seamless loop
FWD_DOTS = 3                        # commands: outward
RET_DOTS = 2                        # replies / decoded events: inward
LANE = 4.5                          # perpendicular offset between the two lanes

# ── Geometry ────────────────────────────────────────────────────────────
BOX_W_L, BOX_H_L = 210, 74
BOX_W_R, BOX_H_R = 232, 92
CORE_W, CORE_H   = 196, 150

LX = 36
CX = (W - CORE_W) // 2
RX = W - BOX_W_R - 64               # leave a channel on the far right for the arc
CORE_Y = (H - CORE_H) // 2 + 6

# (label, detail, accent, y-center)
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

# ── Helpers ─────────────────────────────────────────────────────────────
def lerp(a, b, t):
    return a + (b - a) * t

def _mix(c1, c2, t):
    return tuple(int(lerp(c1[i], c2[i], t)) for i in range(3))

def rounded(d, box, fill, outline=None, width=1, radius=12):
    d.rounded_rectangle(box, radius=radius, fill=fill, outline=outline, width=width)

def text_c(d, cx, cy, s, fnt, fill):
    bb = d.textbbox((0, 0), s, font=fnt)
    d.text((cx - (bb[2] - bb[0]) / 2, cy - (bb[3] - bb[1]) / 2), s, font=fnt, fill=fill)

def perp_offset(p0, p1, d):
    """Shift the segment p0->p1 by `d` along its perpendicular."""
    dx, dy = p1[0] - p0[0], p1[1] - p0[1]
    L = math.hypot(dx, dy) or 1.0
    px, py = -dy / L, dx / L
    return (p0[0] + px * d, p0[1] + py * d), (p1[0] + px * d, p1[1] + py * d)

def dot(d, p0, p1, t, color, r=3.2):
    x = lerp(p0[0], p1[0], t)
    y = lerp(p0[1], p1[1], t)
    d.ellipse([x - r - 3, y - r - 3, x + r + 3, y + r + 3], fill=_mix(color, BG, 0.80))
    d.ellipse([x - r, y - r, x + r, y + r], fill=color)

def poly_point(pts, t):
    """Point at parametric t in [0,1] along a polyline (list of points)."""
    segs = [(pts[i], pts[i + 1]) for i in range(len(pts) - 1)]
    lens = [math.hypot(b[0] - a[0], b[1] - a[1]) for a, b in segs]
    total = sum(lens) or 1.0
    target = t * total
    acc = 0.0
    for (a, b), L in zip(segs, lens):
        if acc + L >= target:
            u = (target - acc) / (L or 1.0)
            return (lerp(a[0], b[0], u), lerp(a[1], b[1], u))
        acc += L
    return pts[-1]

def dot_at(d, p, color, r=3.0):
    x, y = p
    d.ellipse([x - r - 3, y - r - 3, x + r + 3, y + r + 3], fill=_mix(color, BG, 0.80))
    d.ellipse([x - r, y - r, x + r, y + r], fill=color)

def rotated_label(img, s, fnt, fill, cx, cy):
    """Draw `s` rotated 90° (reads bottom-to-top), centred on (cx, cy)."""
    bb = ImageDraw.Draw(img).textbbox((0, 0), s, font=fnt)
    tw, th = bb[2] - bb[0], bb[3] - bb[1]
    tile = Image.new("RGBA", (tw + 4, th + 4), (0, 0, 0, 0))
    ImageDraw.Draw(tile).text((2, 2), s, font=fnt, fill=fill)
    tile = tile.rotate(90, expand=True)
    img.paste(tile, (int(cx - tile.width / 2), int(cy - tile.height / 2)), tile)

# ── Frame ───────────────────────────────────────────────────────────────
def make_frame(frame):
    phase = frame / N_FRAMES                       # 0..1, wraps seamlessly
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)

    # Title + subtitle
    text_c(d, W / 2, 34, "osc-bridge", F_TITLE, WHITE)
    text_c(d, W / 2, 62,
           "one bidirectional OSC surface for 849 synths and DAWs — and an MCP server",
           F_SUB, DIM)

    core_l = (CORE_BOX[0], CORE_Y + CORE_H / 2)
    core_r = (CORE_BOX[2], CORE_Y + CORE_H / 2)

    # ── Main wires (drawn once, down the middle) ────────────────────────
    legs = []  # (p_near_box, p_near_core, accent, is_input)
    for i, (_, _, accent, _) in enumerate(SOURCES):
        b = src_box(i)
        legs.append(((b[2], (b[1] + b[3]) / 2), core_l, accent, True))
    for i, (_, _, accent, _) in enumerate(TARGETS):
        b = tgt_box(i)
        legs.append(((b[0], (b[1] + b[3]) / 2), core_r, accent, False))
    for p_box, p_core, _, _ in legs:
        d.line([p_box, p_core], fill=WIRE, width=2)

    # ── Hardware → DAW cross-link (the orchestrator's [[routes]]) ────────
    hb, db = tgt_box(0), tgt_box(1)
    hw_r  = (hb[2], (hb[1] + hb[3]) / 2)
    daw_r = (db[2], (db[1] + db[3]) / 2)
    ch_x = RX + BOX_W_R + 30                       # arc channel on the far right
    arc = [hw_r, (ch_x, hw_r[1]), (ch_x, daw_r[1]), daw_r]
    for a, b in zip(arc, arc[1:]):
        d.line([a, b], fill=_mix(WIRE, ROUTE, 0.35), width=2)
    rotated_label(img, "[[routes]]", F_TINY, _mix(ROUTE, BG, 0.15),
                  ch_x + 13, (hw_r[1] + daw_r[1]) / 2)

    # ── Travelling packets ──────────────────────────────────────────────
    # Forward lane: commands flowing out (client→core, core→target).
    # Return lane:  decoded events / replies flowing back (the other way).
    for p_box, p_core, accent, is_input in legs:
        if is_input:
            fwd_a, fwd_b = p_box, p_core            # source → core
            fwd_col = MCP if accent is MCP else CORE
        else:
            fwd_a, fwd_b = p_core, p_box            # core → target
            fwd_col = accent
        f0, f1 = perp_offset(fwd_a, fwd_b, LANE)
        for k in range(FWD_DOTS):
            t = (phase + k / FWD_DOTS) % 1.0
            dot(d, f0, f1, t, fwd_col, r=3.2)
        # return lane — opposite direction, dimmer + smaller
        r0, r1 = perp_offset(fwd_b, fwd_a, LANE)
        ret_col = _mix(accent if not is_input else FG, BG, 0.30)
        for k in range(RET_DOTS):
            t = (phase + k / RET_DOTS) % 1.0
            dot(d, r0, r1, t, ret_col, r=2.3)

    # cross-link packets: hardware → DAW
    for k in range(2):
        t = (phase + k / 2) % 1.0
        dot_at(d, poly_point(arc, t), ROUTE, r=3.0)

    # ── Source boxes ────────────────────────────────────────────────────
    for i, (label, detail, accent, cy) in enumerate(SOURCES):
        b = src_box(i)
        rounded(d, b, PANEL, outline=PANEL_HI, width=1)
        d.text((b[0] + 16, b[1] + 13), label, font=F_BOX,
               fill=(MCP if accent is MCP else FG))
        d.text((b[0] + 16, b[1] + 39), detail, font=F_SMALL, fill=DIM)

    # ── Core — subtle pulse on the outline, seamless (period = N_FRAMES) ─
    pulse = 0.5 + 0.5 * math.sin(2 * math.pi * phase)
    rounded(d, CORE_BOX, PANEL_HI, outline=_mix(WIRE, CORE, 0.35 + 0.55 * pulse),
            width=3, radius=16)
    cx = CX + CORE_W / 2
    text_c(d, cx, CORE_Y + 38, "osc-bridge", F_CORE, CORE)
    text_c(d, cx, CORE_Y + 70, "849 device drivers", F_SMALL, FG)
    text_c(d, cx, CORE_Y + 90, "decode in · command out", F_SMALL, DIM)
    text_c(d, cx, CORE_Y + 110, "rate-limited · orchestrated", F_SMALL, DIM)

    # ── Target boxes ────────────────────────────────────────────────────
    for i, (label, detail, accent, cy) in enumerate(TARGETS):
        b = tgt_box(i)
        rounded(d, b, PANEL, outline=_mix(PANEL_HI, accent, 0.45), width=2)
        d.text((b[0] + 16, b[1] + 16), label, font=F_BOX, fill=accent)
        d.text((b[0] + 16, b[1] + 44), detail, font=F_SMALL, fill=DIM)
        d.text((b[0] + 16, b[1] + 62),
               "tested live" if accent is OSC else "verified on hardware",
               font=F_SMALL, fill=_mix(accent, BG, 0.35))

    # Footer
    text_c(d, W / 2, H - 26,
           "bidirectional   ·   hardware ↔ software   ·   MCP   ·   npx -y @roomi-fields/osc-bridge",
           F_FOOT, _mix(FG, MCP, 0.4))

    return img

# ── Main ────────────────────────────────────────────────────────────────
def main():
    out = Path(__file__).parent / "osc-bridge-demo.png"
    rgb = [make_frame(i) for i in range(N_FRAMES)]

    # Every frame is written in full (no APNG frame-diffing) — diffing was
    # accumulating rounding error and popping at the loop wrap. To keep the
    # file small without diffing, the frames are quantized to ONE shared
    # 128-colour palette (the graphic is flat colour + AA text, so 128 is
    # plenty) with dithering off — flat regions stay byte-identical frame to
    # frame, which both compresses well and keeps the loop perfectly clean.
    pal = rgb[N_FRAMES // 4].quantize(colors=128, method=Image.Quantize.MEDIANCUT)
    frames = [f.quantize(palette=pal, dither=Image.Dither.NONE) for f in rgb]
    durations = [int(1000 / FPS)] * N_FRAMES
    frames[0].save(
        out, save_all=True, append_images=frames[1:],
        duration=durations, loop=0, optimize=False,
    )
    kb = out.stat().st_size / 1024
    print(f"Generated: {out}  ({N_FRAMES} frames @ {FPS}fps, {N_FRAMES / FPS:.1f}s, {kb:.0f} KB)")

if __name__ == "__main__":
    main()
