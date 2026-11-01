"""Colour page: average scanned colour of each patch against what was sent (Lab, CIEDE2000).

Usage: python measure_colour.py <scan.bmp>

The scanner is not calibrated, so absolute numbers mix printer and scanner; the greys and the
pale patches show how far the scanner itself is off, and the ranking shows which colours move.
"""
import sys

from diaglib import CORNERS, Scan, de2000, to_lab

PATCHES = [
    ([255, 0, 0], "R"), ([0, 255, 0], "G"), ([0, 0, 255], "B"), ([0, 255, 255], "C"), ([255, 0, 255], "M"), ([255, 255, 0], "Y"),
    ([0, 128, 0], "G128"), ([40, 180, 60], "G180"), ([128, 255, 128], "LTG"), ([0, 100, 60], "DKTEAL"), ([154, 205, 50], "YLWGRN"), ([0, 200, 140], "SEAGRN"),
    ([224, 172, 140], "SKIN1"), ([198, 134, 66], "SKIN2"), ([141, 85, 36], "SKIN3"), ([255, 219, 172], "SKIN4"), ([135, 206, 235], "SKY"), ([255, 165, 0], "ORANGE"),
    ([75, 0, 130], "INDIGO"), ([128, 0, 32], "BURGUN"), ([20, 30, 90], "NAVY"), ([255, 105, 180], "PINK"), ([250, 240, 230], "LINEN"), ([240, 248, 255], "ALICE"),
    ([128, 128, 128], "GRY128"), ([64, 64, 64], "GRY64"), ([192, 192, 192], "GRY192"), ([16, 16, 16], "GRY16"), ([128, 128, 0], "OLIVE"), ([0, 128, 128], "TEAL"),
]
PW, PH, X0, Y0 = 330, 170, 90, 140  # patch size and grid of colour_diag's colour page

scan = Scan(sys.argv[1], locators=CORNERS)
print(scan.describe())
rows = []
for i, (rgb, name) in enumerate(PATCHES):
    col, row = i % 6, i // 6
    x, y = X0 + col * (PW + 25), Y0 + row * (PH + 45)
    got = scan.mean_rgb(x + 0.2 * PW, y + 0.2 * PH, 0.6 * PW, 0.6 * PH)
    ls, lg = to_lab(rgb), to_lab(got)
    rows.append((de2000(ls, lg), name, rgb, got, ls, lg))
print(f"{'patch':7} {'sent RGB':>15} {'scanned RGB':>15}   {'sent Lab':>20} {'scanned Lab':>20}  dE2000")
for de, name, rgb, got, ls, lg in sorted(rows, reverse=True):
    print(f"{name:7} {str(rgb):>15} {str(got.round().astype(int).tolist()):>15}   {str(ls.round(1).tolist()):>20} {str(lg.round(1).tolist()):>20}  {de:5.1f}")
