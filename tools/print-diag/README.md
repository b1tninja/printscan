# Print diagnostics

Pages that put a printer to the test, and the measurements that read them back from a scan.

## Print

```bash
# all four pages as one job (a multi-page TIFF; one frame per page)
cargo run -p printscan-windows --example colour_diag -- print "<printer>" driver "LABEL" --suite
# one page: (none) colour, --regmap, --ramps, --geometry
cargo run -p printscan-windows --example colour_diag -- print "<printer>" driver "LABEL" --ramps
# preview without printing
cargo run -p printscan-windows --example colour_diag -- render suite.tif "LABEL" --suite
```

`driver` and `none` set `psk:PageColorManagement` when the driver lists it; `default` sends the
driver's default ticket.

| Page | What it tests |
|---|---|
| Colour | 30 patches (primaries, greens, skin, darks, pastels), grey / near-white / near-black ramps, chroma detail (equal-luma stripes, checkers and text at 1 to 8 px: chroma subsampling) |
| Registration map | 14 x 6 cells of single-toner C, M, Y (and K) blocks and C/M/Y bars over the whole sheet: each toner's offset, and how it changes down and across the page |
| Tone ramps | C, M, Y, grey and the C+M, C+Y, M+Y overprints from 0 to 100 % in 5 % steps; full-width bars at 100 and 50 %: saturation, weak toner, ink limits, uneven density across the page, streaks |
| Geometry | Lines 1 to 8 mm from each paper edge (the margins), mm and inch rulers, a 100 mm square with circle and diagonals, a 4 in square: scaling, aspect, printable area |

## Scan and measure

```powershell
powershell -ExecutionPolicy Bypass -File scan_flatbed.ps1 -Out page.bmp -Device "*MFC-9330CDW*"
```

```bash
python measure_colour.py page.bmp     # colour page
python measure_regmap.py page.bmp     # registration map
python measure_ramps.py page.bmp      # tone ramps
python measure_geometry.py page.bmp   # geometry
```

Scan at 600 dpi, colour, uncompressed (the script asks WIA for BMP). Every page has black locator
squares; `diaglib.Scan` fits the page-to-scan mapping through them and prints the residuals (about
1 scan px is a good fit). Needs `numpy` and `Pillow`.

## Reading the results

- Scanning the same sheet twice should agree within about 0.2 dE2000: more than that is the
  scanner, not the printer.
- A toner offset that is the same in every column of a row but changes down the page is the
  paper feed or a drum; `measure_regmap.py` fits a sine to it. A period of about 94 mm is one turn
  of a 30 mm drum.
- A bar whose density changes left to right is uneven toner along the cartridge (often a nearly
  empty cartridge); vertical streaks point at the drum or the developer.
