# Colour on the Windows print path

What reaches the driver, and how an application that manages its own colour prints.

## The spooler path keeps RGB values

`WindowsSpooler` prints through winprint, which draws with Direct2D into the XPS print API. Direct2D's
print control is RGB only: its colour space is sRGB, scRGB or a custom profile
([`D2D1_PRINT_CONTROL_PROPERTIES`](https://learn.microsoft.com/en-us/windows/win32/api/d2d1_1/ns-d2d1_1-d2d1_print_control_properties)).
CMYK cannot be sent this way.

`tests/colour_paths.rs` prints ten colour patches through winprint's `ImagePrinter` to a temporary printer
on the in-box Microsoft PWG Raster Class Driver (a lossless raster on a file port) and compares the decoded
patches with the input. Measured 2026-11-01 on Windows 11:

| Path | Largest channel error (0 to 255) |
|---|---|
| `ImagePrinter` (XPS/Direct2D), default ticket | 0 |
| `ImagePrinter`, ticket with `PageColorManagement` = `None` | 0 (the driver drops the option) |
| GDI, `StretchDIBits` with ICM off (`ICM_DONE_OUTSIDEDC`) | 2 (HALFTONE resampling) |

That driver does no colour management of its own. A vendor driver may; this measurement does not cover one.

An earlier colour shift, seen through the Microsoft IPP Class Driver, came from that driver re-encoding every
image as JPEG quality 90 with 4:2:0 chroma. GDI jobs were re-encoded the same way.

## Application-managed colour

Most photo and office inkjet drivers take RGB and convert to ink themselves. An application that soft-proofs
and converts to the printer's profile sends RGB already in that profile, and tells the driver not to convert
again with the Print Schema keywords `PageColorManagement` = `None` and `PageDestinationColorProfile` =
`Application` ([Color management and the Print Schema](https://learn.microsoft.com/en-us/windows/win32/printdocs/color-management-and-the-print-schema)).
Whether a driver honours them is up to the driver. Sending untagged pixels without them lets a driver that
manages colour treat them as sRGB and convert them a second time.

GDI with ICM off was the older way to get the same effect. It is not offered here: the XPS path is at least
as exact on the driver measured, and GDI is the legacy pipeline.

## Device colour (CMYK)

Device colour reaches a printer only by going around the Windows driver: an IPP printer that takes
`application/pdf` (a PDF keeps DeviceCMYK, ICC-based colour and output intents) or PWG raster in `cmyk_8`
when it lists that type (`printscan-ipp`). IPP printers that publish `printer-icc-profiles` let a client fetch
their profiles to soft-proof, and take `print-rendering-intent` per job (PWG 5100.13).
