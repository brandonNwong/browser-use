#!/usr/bin/env python3
"""
Render the Browser Use brand icon (matching macos/Resources/AppIcon.svg) into a
multi-resolution PNG-encoded Windows .ico at windows/BrowserUse/Resources/AppIcon.ico.

Pure Python (struct/zlib/math) — zero external dependencies required.
"""

from __future__ import annotations

import math
import struct
import zlib
from pathlib import Path

SIZES = [16, 24, 32, 48, 64, 128, 256]

# SVG viewBox is 1024x1024.
# Background linear gradient from (0,0) to (1024,1024)
BG_STOPS = [
    (0.00, (0x22, 0x23, 0x2A)),
    (0.55, (0x17, 0x18, 0x1D)),
    (1.00, (0x0F, 0x10, 0x14)),
]
# White-silver arc linear gradient from (186,186) to (838,838)
ARC_STOPS = [
    (0.00, (0xFF, 0xFF, 0xFF)),
    (1.00, (0xD6, 0xD8, 0xE0)),
]
# Orange circle linear gradient from (404,404) to (620,620)
ORANGE_STOPS = [
    (0.00, (0xFF, 0x8A, 0x3D)),
    (1.00, (0xFF, 0x52, 0x00)),
]

# Outer arc: M 826.89 427.62 A 326 326 0 1 1 698.99 244.96 (stroke-width=56, round cap)
# Inner arc: M 305.29 567.39 A 214 214 0 1 1 389.25 687.30 (stroke-width=56, round cap)
# Center circle: cx=512, cy=512, r=108
CX = 512.0
CY = 512.0
OUTER_R = 326.0
INNER_R = 214.0
STROKE_HALF = 28.0
CENTER_R = 108.0

OUTER_START = math.atan2(427.62 - CY, 826.89 - CX)
OUTER_END = math.atan2(244.96 - CY, 698.99 - CX)
OUTER_CAP_1 = (826.89, 427.62)
OUTER_CAP_2 = (698.99, 244.96)

INNER_START = math.atan2(567.39 - CY, 305.29 - CX)
INNER_END = math.atan2(687.30 - CY, 389.25 - CX)
INNER_CAP_1 = (305.29, 567.39)
INNER_CAP_2 = (389.25, 687.30)


def lerp_stops(stops: list[tuple[float, tuple[int, int, int]]], t: float) -> tuple[float, float, float]:
    t = max(0.0, min(1.0, t))
    for i in range(len(stops) - 1):
        t0, c0 = stops[i]
        t1, c1 = stops[i + 1]
        if t <= t1 or i == len(stops) - 2:
            f = 0.0 if t1 == t0 else (t - t0) / (t1 - t0)
            return (
                c0[0] + (c1[0] - c0[0]) * f,
                c0[1] + (c1[1] - c0[1]) * f,
                c0[2] + (c1[2] - c0[2]) * f,
            )
    c = stops[-1][1]
    return float(c[0]), float(c[1]), float(c[2])


def linear_t(x: float, y: float, x0: float, y0: float, x1: float, y1: float) -> float:
    dx = x1 - x0
    dy = y1 - y0
    denom = dx * dx + dy * dy
    if denom == 0.0:
        return 0.0
    return ((x - x0) * dx + (y - y0) * dy) / denom


def angle_in_sweep(theta: float, start: float, end: float) -> bool:
    """True when theta lies on the positive-angle (clockwise in SVG screen coords) arc from start to end."""
    two_pi = 2.0 * math.pi
    d_theta = (theta - start) % two_pi
    d_end = (end - start) % two_pi
    return d_theta <= d_end


def dist_to_arc(x: float, y: float, radius: float, start: float, end: float, cap1: tuple[float, float], cap2: tuple[float, float]) -> float:
    dx = x - CX
    dy = y - CY
    r = math.hypot(dx, dy)
    theta = math.atan2(dy, dx)
    if angle_in_sweep(theta, start, end):
        return abs(r - radius)
    d1 = math.hypot(x - cap1[0], y - cap1[1])
    d2 = math.hypot(x - cap2[0], y - cap2[1])
    return min(d1, d2)


def rounded_rect_sdf(x: float, y: float, pad: float, corner_r: float) -> float:
    """Signed distance to centered rounded square inside [pad, 1024-pad]. Negative is inside."""
    half = 512.0 - pad - corner_r
    qx = abs(x - 512.0) - half
    qy = abs(y - 512.0) - half
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    inside = min(max(qx, qy), 0.0)
    return outside + inside - corner_r


def sample_pixel(x: float, y: float, px_size: float) -> tuple[float, float, float, float]:
    # Rounded square mask (Windows desktop/taskbar icons look best with subtle rounded corners).
    sdf_bg = rounded_rect_sdf(x, y, pad=40.0, corner_r=180.0)
    bg_alpha = max(0.0, min(1.0, 0.5 - sdf_bg / px_size))
    if bg_alpha <= 0.0:
        return (0.0, 0.0, 0.0, 0.0)

    r, g, b = lerp_stops(BG_STOPS, linear_t(x, y, 0.0, 0.0, 1024.0, 1024.0))

    # Arcs
    d_outer = dist_to_arc(x, y, OUTER_R, OUTER_START, OUTER_END, OUTER_CAP_1, OUTER_CAP_2) - STROKE_HALF
    d_inner = dist_to_arc(x, y, INNER_R, INNER_START, INNER_END, INNER_CAP_1, INNER_CAP_2) - STROKE_HALF
    d_arc = min(d_outer, d_inner)
    arc_alpha = max(0.0, min(1.0, 0.5 - d_arc / px_size))
    if arc_alpha > 0.0:
        ar, ag, ab = lerp_stops(ARC_STOPS, linear_t(x, y, 186.0, 186.0, 838.0, 838.0))
        inv = 1.0 - arc_alpha
        r = r * inv + ar * arc_alpha
        g = g * inv + ag * arc_alpha
        b = b * inv + ab * arc_alpha

    # Center orange circle
    d_center = math.hypot(x - CX, y - CY) - CENTER_R
    center_alpha = max(0.0, min(1.0, 0.5 - d_center / px_size))
    if center_alpha > 0.0:
        cr, cg, cb = lerp_stops(ORANGE_STOPS, linear_t(x, y, 404.0, 404.0, 620.0, 620.0))
        inv = 1.0 - center_alpha
        r = r * inv + cr * center_alpha
        g = g * inv + cg * center_alpha
        b = b * inv + cb * center_alpha

    return (r, g, b, bg_alpha)


def render_png(size: int) -> bytes:
    scale = 1024.0 / size
    sub = 3 if size <= 64 else 2
    inv_sub = 1.0 / sub
    inv_samples = 1.0 / (sub * sub)

    raw_rows = bytearray()
    for py in range(size):
        raw_rows.append(0)  # PNG filter type None
        for px in range(size):
            ar = ag = ab = aa = 0.0
            for sy in range(sub):
                y = (py + (sy + 0.5) * inv_sub) * scale
                for sx in range(sub):
                    x = (px + (sx + 0.5) * inv_sub) * scale
                    sr, sg, sb, sa = sample_pixel(x, y, scale)
                    ar += sr * sa
                    ag += sg * sa
                    ab += sb * sa
                    aa += sa
            aa *= inv_samples
            if aa > 1e-6:
                rr = int(max(0.0, min(255.0, (ar * inv_samples) / aa + 0.5)))
                gg = int(max(0.0, min(255.0, (ag * inv_samples) / aa + 0.5)))
                bb = int(max(0.0, min(255.0, (ab * inv_samples) / aa + 0.5)))
                ia = int(max(0.0, min(255.0, aa * 255.0 + 0.5)))
            else:
                rr = gg = bb = ia = 0
            raw_rows.extend((rr, gg, bb, ia))

    def chunk(tag: bytes, data: bytes) -> bytes:
        crc = zlib.crc32(tag + data) & 0xFFFFFFFF
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", crc)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    idat = zlib.compress(bytes(raw_rows), level=9)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


def build_ico(pngs: list[tuple[int, bytes]]) -> bytes:
    header = struct.pack("<HHH", 0, 1, len(pngs))
    offset = 6 + 16 * len(pngs)
    entries = bytearray()
    payload = bytearray()
    for size, png in pngs:
        dim = 0 if size >= 256 else size
        entries.extend(struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), offset))
        payload.extend(png)
        offset += len(png)
    return header + bytes(entries) + bytes(payload)


def main() -> None:
    repo_root = Path(__file__).resolve().parents[2]
    out_path = repo_root / "windows" / "BrowserUse" / "Resources" / "AppIcon.ico"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    pngs = [(size, render_png(size)) for size in SIZES]
    out_path.write_bytes(build_ico(pngs))
    print(f"Generated {out_path} ({out_path.stat().st_size} bytes, sizes={SIZES})")


if __name__ == "__main__":
    main()
