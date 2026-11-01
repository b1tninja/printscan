"""Tone ramps page: density curves, where each toner saturates, cross-talk, and uniformity bars.

Usage: python measure_ramps.py <scan.bmp>
"""
import sys

import numpy as np

from diaglib import Scan

X = 150 - 75  # left edge of the ramps and bars (content px)
NAMES = ["CYAN", "MAGENTA", "YELLOW", "GREY", "BLUE C+M", "GREEN C+Y", "RED M+Y"]
CH = {"CYAN": [0], "MAGENTA": [1], "YELLOW": [2], "GREY": [0, 1, 2], "BLUE C+M": [0, 1], "GREEN C+Y": [0, 2], "RED M+Y": [1, 2]}
BARS = [("CYAN 100", [0]), ("CYAN 50", [0]), ("MAGENTA 100", [1]), ("MAGENTA 50", [1]), ("YELLOW 100", [2]), ("YELLOW 50", [2]), ("GREY 50", [0, 1, 2])]

scan = Scan(sys.argv[1])
print(scan.describe())

print("\nRamp density (mean scanned ink in the toners' channels, 0 = paper, 255 = black), every 10 %:")
curves = {}
for r, name in enumerate(NAMES):
    y = 110 + r * 190 + 22
    vals = np.array([255 - scan.mean_rgb(X + t * 100 + 20, y + 20, 60, 80) for t in range(21)])
    curves[name] = vals
    c = vals[:, CH[name]].mean(axis=1)
    top = c.max()
    sat = next(t for t in range(21) if c[t] >= top - 0.03 * (top - c[0]))
    print(f"  {name:10} " + " ".join(f"{v:4.0f}" for v in c[::2]) + f"   97 % of max at {sat * 5} %, last 25 %: {c[20] - c[15]:+.1f}")

print("\nSingle-toner ramps at 100 %, ink in all three channels (a toner's own channel should dominate):")
for name in ("CYAN", "MAGENTA", "YELLOW"):
    print(f"  {name:8} R/G/B ink = {curves[name][20].round(0).astype(int).tolist()}")

print("\nUniformity bars: ink along the bar in 12 sections, left to right; range, and streaks (sd of column means about a 30 px running mean)")
for k, (name, chans) in enumerate(BARS):
    y = 1470 + k * 160 + 22 + 20
    xs = np.arange(X + 10, X + 2090, 0.5)
    cols = []
    for ch in chans:
        acc = np.zeros_like(xs)
        for dy in np.linspace(0, 70, 15):
            acc += scan.sample(xs, np.full_like(xs, y + dy), ch)
        cols.append(255 - acc / 15)
    col = np.mean(cols, axis=0)
    means = [b.mean() for b in np.array_split(col, 12)]
    k_ = 61
    smooth = np.convolve(col, np.ones(k_) / k_, mode="same")
    streak = np.std((col - smooth)[k_:-k_])
    print(f"  {name:11} " + " ".join(f"{m:4.0f}" for m in means) + f"   range {max(means) - min(means):4.1f}  streak sd {streak:4.1f}")
