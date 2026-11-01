"""Geometry page: printed size and aspect of the 100 mm square and 4 in square, and the margins.

Usage: python measure_geometry.py <scan.bmp> [scan_dpi]

Sizes are measured in scan pixels and converted with the scanner's resolution (600 dpi by
default), so they do not depend on the page-to-scan mapping. The margins: which of the lines
1 to 8 mm from each paper edge printed (the first one that did is the printer's margin there).
"""
import sys

import numpy as np

from diaglib import MARGIN, Scan

scan = Scan(sys.argv[1])
dpi = float(sys.argv[2]) if len(sys.argv) > 2 else 600.0
print(scan.describe())
MM = 300 / 25.4  # page px per mm


def dark_profile(p0, p1, n=4001, half=6):
    """Mean of the three channels along p0 -> p1 (sheet px), averaged over +-half px across."""
    p0, p1 = np.asarray(p0, float), np.asarray(p1, float)
    d = (p1 - p0) / np.linalg.norm(p1 - p0)
    nrm = np.array([-d[1], d[0]])
    t = np.linspace(0, 1, n)
    acc = np.zeros(n)
    for w in np.linspace(-half, half, 7):
        pts = p0[None, :] + (p1 - p0)[None, :] * t[:, None] + nrm * w
        px, py = pts[:, 0] - MARGIN, pts[:, 1] - MARGIN  # sheet -> content coordinates
        acc += sum(scan.sample(px, py, ch) for ch in range(3)) / 3
    return t, acc / 7


def line_centres(p0, p1, expect, window=15):
    """Sub-pixel centres (in sheet px along p0 -> p1) of the dark lines nearest the `expect`ed offsets."""
    p0, p1 = np.asarray(p0, float), np.asarray(p1, float)
    length = np.linalg.norm(p1 - p0)
    t, prof = dark_profile(p0, p1)
    s = t * length
    ink = np.clip(np.percentile(prof, 90) - prof, 0, None)
    out = []
    for e in expect:
        m = (s > e - window) & (s < e + window)
        w = ink[m]
        if w.max() < 40:
            out.append(None)
            continue
        w = np.clip(w - 0.5 * w.max(), 0, None)
        out.append(float((s[m] * w).sum() / w.sum()))
    return out


def scan_distance(a, b):
    """Distance in mm between two sheet points, measured in the scan."""
    (xa, ya), (xb, yb) = (scan.to_scan(a[0] - MARGIN, a[1] - MARGIN), scan.to_scan(b[0] - MARGIN, b[1] - MARGIN))
    return np.hypot(xb - xa, yb - ya) / dpi * 25.4


# 100 mm square: outline 3 px at (sx, sy) side 1181 px (sheet px)
sx, sy, side = MARGIN + 450, MARGIN + 450, round(100 * MM)
mid = sy + side / 2
l, r = line_centres((sx - 40, mid + 150), (sx + side + 40, mid + 150), [40 + 1.5, 40 + side - 1.5])
t, b = line_centres((sx + side / 2 + 150, sy - 40), (sx + side / 2 + 150, sy + side + 40), [40 + 1.5, 40 + side - 1.5])
if None not in (l, r, t, b):
    width = scan_distance((sx - 40 + l, mid), (sx - 40 + r, mid))
    height = scan_distance((sx, sy - 40 + t), (sx, sy - 40 + b))
    # the outline centres are 3 px inside the drawn square's outer edges: add the 3 px back
    pad = 3 / MM
    print(f"\n100 mm square: {width + pad:.2f} x {height + pad:.2f} mm (aspect {width / height:.4f})")

# 4 inch square
qx, qy, q = MARGIN + 450, sy + side + 120, 1200
mid = qy + q / 2
l, r = line_centres((qx - 40, mid), (qx + q + 40, mid), [40 + 1.5, 40 + q - 1.5])
t, b = line_centres((qx + q / 2, qy - 40), (qx + q / 2, qy + q + 40), [40 + 1.5, 40 + q - 1.5])
if None not in (l, r, t, b):
    w = scan_distance((qx - 40 + l, mid), (qx - 40 + r, mid)) + 3 / MM
    h = scan_distance((qx, qy - 40 + t), (qx, qy - 40 + b)) + 3 / MM
    print(f"4 in square: {w / 25.4:.4f} x {h / 25.4:.4f} in ({w:.2f} x {h:.2f} mm)")

# Margins: lines d mm from each paper edge (sheet 2550 x 3300)
W, H = 2550, 3300
print("\nMargin lines that printed (mm from the paper edge):")
combs = {
    "top": ((W / 2, 1), (W / 2, 120), [round(d * MM) + 1 - 1 for d in range(1, 9)]),
    "bottom": ((W / 2, H - 2), (W / 2, H - 121), [round(d * MM) + 1 for d in range(1, 9)]),
    "left": ((1, H / 2), (120, H / 2), [round(d * MM) for d in range(1, 9)]),
    "right": ((W - 2, H / 2), (W - 121, H / 2), [round(d * MM) + 1 for d in range(1, 9)]),
}
for side_name, (p0, p1, expect) in combs.items():
    try:
        found = line_centres(p0, p1, expect, window=5)
    except (IndexError, ValueError):
        print(f"  {side_name:6} outside the scan")
        continue
    present = [d for d, f in zip(range(1, 9), found) if f is not None]
    print(f"  {side_name:6} {present}  -> margin about {present[0] if present else '>8'} mm")
