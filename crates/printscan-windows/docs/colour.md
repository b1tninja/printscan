# Colour on the Windows print path

What reaches the driver, and how an application that manages its own colour prints.

## The spooler path keeps RGB values

`WindowsSpooler` draws each page with Direct2D into the XPS print API, through winprint's print context. Direct2D's
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

### Asking for it

A job asks with `JobOptions::application_managed_colour`, and names the intent it converted with in
`JobOptions::rendering_intent`. `WindowsSpooler` reads the driver's PrintCapabilities and:

| The driver lists | The ticket gets | Reported |
|---|---|---|
| `PageColorManagement` with `None` | `PageColorManagement` = `None` | nothing; the job is refused if the validated ticket does not keep it |
| `PageColorManagement` without `None` | nothing | the job is refused before anything is sent: the driver would convert the colours a second time |
| no `PageColorManagement` | nothing | a warning: the driver has no setting to tell, and whether it converts is up to it |
| `PageDestinationColorProfile` with `Application` | `PageDestinationColorProfile` = `Application` | a warning if the validated ticket does not keep it |
| `PageDestinationColorProfile` without `Application` | nothing | a warning |
| `PageICMRenderingIntent` with the intent | that intent | a warning if the validated ticket has another |
| `PageICMRenderingIntent` without it, or not at all | nothing | a warning |

Application-managed colour is checked like the job's other explicit settings (two sides, grey or colour): a
setting the driver would not keep refuses the job, because the print would differ from what was asked. A driver
with no colour-management setting at all prints with a warning instead, as grey prints on a printer with no
colour setting: the in-box class drivers list none and convert nothing (the measurement above), and refusing
there would refuse the drivers where the values arrive exactly. The destination profile and the intent are
hints, as the resolution is.

The rendering intents map to the Print Schema's (the DEVMODE's `dmICMIntent`): perceptual to `Photographs`,
saturation to `BusinessGraphics`, relative to `RelativeColorimetric`, absolute to `AbsoluteColorimetric`.
`RelativeBpc` becomes `RelativeColorimetric` with a warning: the Print Schema has no black point compensation.
`Auto` sets nothing. A driver that lists `PageColorManagement` = `None` and an intent shows
`JobHint::ApplicationManagedColour` and `JobHint::RenderingIntent` in its `PrinterDetails::honoured`.

The pages are drawn into an sRGB Direct2D print context, so the XPS content is tagged sRGB; with
`PageColorManagement` = `None` a driver is not to convert it. Send the colours as untagged DeviceRGB in the PDF:
how `Windows.Data.Pdf` renders ICC-based colour spaces was not measured. No destination profile URI is
written into the ticket.

Measured 2026-11-07 on Windows 11 (`tests/colour_paths.rs`, run by hand): through `WindowsSpooler`, a PDF of
the ten patches as DeviceRGB fills printed to the PWG raster printer with application-managed colour and the
relative intent came back exact (largest channel error 0), with the two warnings (no colour-management setting,
no rendering intent). On a temporary printer on the in-box PostScript driver `MS Publisher Color Printer`, whose
ticket provider lists `PageColorManagement` and `PageICMRenderingIntent` from the DEVMODE, the validated ticket
kept `None` and `RelativeColorimetric`. Its output was not decoded, and no driver that colour-manages by
itself (a vendor photo driver) has been tried: whether one honours `None` is up to the driver and is untested.

## Device colour (CMYK)

Device colour reaches a printer only by going around the Windows driver: an IPP printer that takes
`application/pdf` (a PDF keeps DeviceCMYK, ICC-based colour and output intents) or PWG raster in `cmyk_8`
when it lists that type (`printscan-ipp`). IPP printers that publish `printer-icc-profiles` let a client fetch
their profiles to soft-proof, and take `print-rendering-intent` per job (PWG 5100.13).
