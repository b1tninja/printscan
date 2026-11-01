"""Shared pieces of the print diagnostic measurements: loading a scan and mapping page coordinates.

Page coordinates are 300 dpi pixels of the 7.5 x 10 in content area of a colour_diag page, which
sits 150 px inside a Letter sheet. Every page has black locator squares of 60 px centred at
LOCATORS (the colour page only the four corners). A scan at any resolution near 600 dpi is
mapped by an affine fit through the locators it finds.
"""
from __future__ import annotations

import numpy as np
from PIL import Image

LOCATORS = [(30, 30), (2220, 30), (30, 1500), (2220, 1500), (30, 2970), (2220, 2970)]
CORNERS = [(30, 30), (2220, 30), (30, 2970), (2220, 2970)]
MARGIN = 150  # content offset on the sheet, 300 dpi px


class Scan:
    """A scanned page and its page-to-scan mapping."""

    def __init__(self, path: str, locators=LOCATORS, scale_guess: float = 2.0, search: int = 150):
        self.rgb = np.asarray(Image.open(path).convert("RGB")).astype(np.float64)
        self.h, self.w, _ = self.rgb.shape
        dark = self.rgb.max(axis=2) < 90
        self._dark = dark
        self._ii = np.pad(dark.astype(np.int64).cumsum(0).cumsum(1), ((1, 0), (1, 0)))
        side = int(round(60 * scale_guess))
        found = [self._square(int((x + MARGIN) * scale_guess), int((y + MARGIN) * scale_guess), side, search) for x, y in locators]
        src = np.array([(x, y, 1.0) for x, y in locators])
        self.A, *_ = np.linalg.lstsq(src, np.array(found), rcond=None)
        self.residuals = np.hypot(*(src @ self.A - np.array(found)).T)

    def _box(self, x, y, w, h):
        ii = self._ii
        return ii[y + h, x + w] - ii[y, x + w] - ii[y + h, x] + ii[y, x]

    def _square(self, cx, cy, side, r, ring=24):
        """Centre of the solid dark square of `side` px near (cx, cy) whose surrounding ring is light."""
        x0, x1 = max(ring, cx - r), min(self.w - side - ring, cx + r)
        y0, y1 = max(ring, cy - r), min(self.h - side - ring, cy + r)
        ys, xs = np.mgrid[y0:y1, x0:x1]
        inner = self._box(xs, ys, side, side)
        outer = self._box(xs - ring, ys - ring, side + 2 * ring, side + 2 * ring) - inner
        k = np.argmax(inner - 4 * outer)
        y, x = ys.flat[k], xs.flat[k]
        yy, xx = np.nonzero(self._dark[y:y + side, x:x + side])
        return x + xx.mean(), y + yy.mean()

    def describe(self) -> str:
        sx, sy = np.hypot(*self.A[0]), np.hypot(*self.A[1])
        rot = np.degrees(np.arctan2(self.A[0, 1], self.A[0, 0]))
        return f"scan {self.w}x{self.h}; locator residuals {np.round(self.residuals, 2).tolist()} px; scale {sx:.4f} x {sy:.4f}; rotation {rot:.3f} deg"

    def to_scan(self, px, py):
        A = self.A
        return A[0, 0] * px + A[1, 0] * py + A[2, 0], A[0, 1] * px + A[1, 1] * py + A[2, 1]

    def sample(self, px, py, ch):
        """Bilinear sample of channel `ch` at page point(s); arrays allowed."""
        x, y = self.to_scan(np.asarray(px, float), np.asarray(py, float))
        x0, y0 = np.floor(x).astype(int), np.floor(y).astype(int)
        fx, fy = x - x0, y - y0
        c = self.rgb[:, :, ch]
        return c[y0, x0] * (1 - fx) * (1 - fy) + c[y0, x0 + 1] * fx * (1 - fy) + c[y0 + 1, x0] * (1 - fx) * fy + c[y0 + 1, x0 + 1] * fx * fy

    def mean_rgb(self, x, y, w, h, step=2):
        """Mean scanned RGB over a page rectangle."""
        ys, xs = np.mgrid[y:y + h:step, x:x + w:step]
        return np.array([self.sample(xs, ys, ch).mean() for ch in range(3)])


def to_lab(rgb):
    """sRGB (0-255) to CIE Lab (D65)."""
    c = np.asarray(rgb, dtype=np.float64) / 255.0
    c = np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
    m = np.array([[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]])
    xyz = m @ c / np.array([0.95047, 1.0, 1.08883])
    f = np.where(xyz > (6 / 29) ** 3, np.cbrt(xyz), xyz / (3 * (6 / 29) ** 2) + 4 / 29)
    return np.array([116 * f[1] - 16, 500 * (f[0] - f[1]), 200 * (f[1] - f[2])])


def de2000(l1, l2) -> float:
    """CIEDE2000 colour difference."""
    L1, a1, b1 = l1
    L2, a2, b2 = l2
    C1, C2 = np.hypot(a1, b1), np.hypot(a2, b2)
    Cb = (C1 + C2) / 2
    G = 0.5 * (1 - np.sqrt(Cb**7 / (Cb**7 + 25**7)))
    a1p, a2p = (1 + G) * a1, (1 + G) * a2
    C1p, C2p = np.hypot(a1p, b1), np.hypot(a2p, b2)
    h1p, h2p = np.degrees(np.arctan2(b1, a1p)) % 360, np.degrees(np.arctan2(b2, a2p)) % 360
    dLp, dCp = L2 - L1, C2p - C1p
    dh = h2p - h1p
    if C1p * C2p == 0:
        dh = 0
    elif dh > 180:
        dh -= 360
    elif dh < -180:
        dh += 360
    dHp = 2 * np.sqrt(C1p * C2p) * np.sin(np.radians(dh / 2))
    Lbp, Cbp = (L1 + L2) / 2, (C1p + C2p) / 2
    hsum = h1p + h2p
    if C1p * C2p == 0:
        hbp = hsum
    elif abs(h1p - h2p) > 180:
        hbp = (hsum + 360) / 2 if hsum < 360 else (hsum - 360) / 2
    else:
        hbp = hsum / 2
    T = 1 - 0.17 * np.cos(np.radians(hbp - 30)) + 0.24 * np.cos(np.radians(2 * hbp)) + 0.32 * np.cos(np.radians(3 * hbp + 6)) - 0.20 * np.cos(np.radians(4 * hbp - 63))
    dth = 30 * np.exp(-(((hbp - 275) / 25) ** 2))
    Rc = 2 * np.sqrt(Cbp**7 / (Cbp**7 + 25**7))
    Sl = 1 + 0.015 * (Lbp - 50) ** 2 / np.sqrt(20 + (Lbp - 50) ** 2)
    Sc, Sh = 1 + 0.045 * Cbp, 1 + 0.015 * Cbp * T
    Rt = -np.sin(np.radians(2 * dth)) * Rc
    return float(np.sqrt((dLp / Sl) ** 2 + (dCp / Sc) ** 2 + (dHp / Sh) ** 2 + Rt * (dCp / Sc) * (dHp / Sh)))
