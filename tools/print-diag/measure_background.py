"""Background toner on blank paper: how dark it is down the page and whether it repeats.

Usage: python measure_background.py <scan.bmp> [x0 x1 y0 y1]

Reads a blank strip of the page (content px; by default the empty column right of the geometry
page's squares), averages each row across the strip, and reports the darkness profile and its
strongest periods. A period near 94 mm is one turn of a 30 mm drum.
"""
import sys

import numpy as np

from diaglib import Scan

scan = Scan(sys.argv[1])
print(scan.describe())
x0, x1, y0, y1 = (float(v) for v in sys.argv[2:6]) if len(sys.argv) >= 6 else (1720.0, 2120.0, 380.0, 2900.0)
MM = 300 / 25.4

ys = np.arange(y0, y1, 0.5)
xs = np.linspace(x0, x1, 80)
grey = np.zeros_like(ys)
for x in xs:
    grey += sum(scan.sample(np.full_like(ys, x), ys, ch) for ch in range(3)) / 3
grey = 255 - grey / len(xs)  # darkness: 0 = scanner white

print(f"\nblank strip x {x0:.0f}-{x1:.0f}, y {y0:.0f}-{y1:.0f} (content px): darkness mean {grey.mean():.2f}, "
      f"min {grey.min():.2f}, max {grey.max():.2f}, sd {grey.std():.2f}")
# Profile in 5 mm steps.
step = int(round(5 * MM / 0.5))
means = [grey[i:i + step].mean() for i in range(0, len(grey) - step + 1, step)]
print("darkness every 5 mm down the strip: " + " ".join(f"{m:4.1f}" for m in means))

# Strongest periods.
g = grey - grey.mean()
spec = np.abs(np.fft.rfft(g * np.hanning(len(g))))
freqs = np.fft.rfftfreq(len(g), d=0.5 / MM)  # cycles per mm
order = np.argsort(spec[1:])[::-1] + 1
print("strongest periods:")
shown = []
for k in order:
    period = 1 / freqs[k]
    if period > 300 or period < 3 or any(abs(period - p) / p < 0.08 for p in shown):
        continue
    shown.append(period)
    print(f"  {period:6.1f} mm   (relative strength {spec[k] / spec[order[0]]:.2f})")
    if len(shown) == 5:
        break
