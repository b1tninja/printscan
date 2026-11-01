"""Registration map: per cell, each toner block's offset from where it should be, and its ink.

Usage: python measure_regmap.py <scan.bmp>

Each block edge is found in the scan channel its toner absorbs (cyan: red, magenta: green,
yellow: blue) within +-10 px of where it should be (blocks are 15 px apart), at the level half-way
between the paper next to the block and the block's own centre, so neighbouring blocks and a weak
toner do not confuse it. Offsets between toners in one cell need no mapping; the sine fit of the
cyan-minus-yellow vertical offset down the page finds a periodic error (a drum turn is ~94 mm).
"""
import sys

import numpy as np

from diaglib import Scan

X0, Y0, PX, PY, BLK, STEP = 150, 110, 340, 200, 60, 75  # colour_diag regmap_page grid
ROWS, COLS = 14, 6
TONERS = [("C", 0), ("M", 1), ("Y", 2)]
SPAN = 10

scan = Scan(sys.argv[1])
print(scan.describe())


def edge(fixed, centre, outside, axis, ch, half_width=8):
    t = np.linspace(outside, centre, 241)
    acc = np.zeros_like(t)
    for w in np.linspace(-half_width, half_width, 9):
        acc += scan.sample(t, np.full_like(t, fixed + w), ch) if axis == "x" else scan.sample(np.full_like(t, fixed + w), t, ch)
    p = acc / 9
    paper, ink = p[:12].mean(), p[-12:].mean()
    if paper - ink < 20:
        return None
    mid = (paper + ink) / 2
    idx = np.nonzero(p < mid)[0]
    if len(idx) == 0 or idx[0] == 0:
        return None
    i = idx[0]
    f = (p[i - 1] - mid) / (p[i - 1] - p[i])
    return t[i - 1] + f * (t[i] - t[i - 1])


res = {}
for row in range(ROWS):
    for col in range(COLS):
        cx0, cy0 = X0 + col * PX, Y0 + row * PY
        for k, (name, ch) in enumerate(TONERS):
            bx, by = cx0 + k * STEP, cy0
            mx, my = bx + BLK / 2, by + BLK / 2
            top = edge(mx, by + SPAN, by - SPAN, "y", ch)
            bot = edge(mx, by + BLK - SPAN, by + BLK + SPAN, "y", ch)
            left = edge(my, bx + SPAN, bx - SPAN, "x", ch)
            right = edge(my, bx + BLK - SPAN, bx + BLK + SPAN, "x", ch)
            ys, xs = np.mgrid[by + 15:by + BLK - 15:3, bx + 15:bx + BLK - 15:3]
            r = {"ink": 255 - scan.sample(xs, ys, ch).mean()}
            if top is not None and bot is not None:
                r["dy"], r["h"] = (top - by + bot - (by + BLK)) / 2, bot - top
            if left is not None and right is not None:
                r["dx"] = (left - bx + right - (bx + BLK)) / 2
            res[(row, col, name)] = r

get = lambda row, col, t, key: res.get((row, col, t), {}).get(key)


def grid(title, value, fmt="{:+6.2f}"):
    print(f"\n{title}")
    print("         " + "".join(f"   col{c}" for c in range(COLS)))
    for row in range(ROWS):
        cells = []
        for col in range(COLS):
            v = value(row, col)
            cells.append("      -" if v is None else " " + fmt.format(v))
        print(f"  row {row:2d}" + "".join(cells))


def rel(a, b, key):
    return lambda r, c: None if get(r, c, a, key) is None or get(r, c, b, key) is None else get(r, c, a, key) - get(r, c, b, key)


grid("C minus Y, vertical (page px, + = cyan lower; 1 px = 0.085 mm)", rel("C", "Y", "dy"))
grid("M minus Y, vertical (page px)", rel("M", "Y", "dy"))
grid("C minus Y, horizontal (page px, + = cyan further right)", rel("C", "Y", "dx"))
grid("M minus Y, horizontal (page px)", rel("M", "Y", "dx"))
for t, _ in TONERS:
    grid(f"{t}: ink (255 - its scan channel; higher = more toner)", lambda r, c, t=t: get(r, c, t, "ink"), "{:6.0f}")

# A periodic error down the page: fit a sine to the per-row mean of C - Y (and M - Y).
for a in ("C", "M"):
    ys, vs = [], []
    for row in range(ROWS):
        v = [rel(a, "Y", "dy")(row, c) for c in range(COLS)]
        v = [x for x in v if x is not None]
        if v:
            ys.append(Y0 + row * PY + BLK / 2)
            vs.append(np.mean(v))
    y, v = np.array(ys), np.array(vs)
    best = None
    for period in np.arange(400, 2401, 5):
        X = np.column_stack([np.sin(2 * np.pi * y / period), np.cos(2 * np.pi * y / period), np.ones_like(y)])
        c, *_ = np.linalg.lstsq(X, v, rcond=None)
        r = float(((X @ c - v) ** 2).sum())
        if best is None or r < best[0]:
            best = (r, period, c)
    r, period, c = best
    amp = np.hypot(c[0], c[1])
    print(f"\n{a} - Y vertical down the page: mean {c[2]:+.2f} px, best sine period {period * 25.4 / 300:.1f} mm, "
          f"amplitude {amp:.2f} px ({amp * 25.4 / 300:.2f} mm), rms residual {np.sqrt(r / len(v)):.2f} px")
print(f"\nblocks with all four edges: {sum(1 for v in res.values() if 'dy' in v and 'dx' in v)} of {len(res)}")
