# printscan-windows

Windows printing and scanning for [printscan](https://github.com/b1tninja/printscan). Requires
Rust 1.95.

* **`spooler`** (feature `print`, default): the Windows print spooler as a `printscan::print`
  backend, on [`winprint`](https://crates.io/crates/winprint). A PDF job is drawn by the in-box
  `Windows.Data.Pdf` into the XPS print API at the resolution of the job's PrintTicket, in this
  process: no other program is started. Pages of either orientation print as one job on one
  paper, a page the other way round turned onto it; with shrink-to-fit, oversized pages shrink
  onto the printable area. Copies, collation, two-sided, colour, paper, orientation,
  resolution, rendering intent and application-managed colour become the PrintTicket, which the
  driver validates: a setting the job asks for that the driver does not keep refuses the job
  before anything is sent, and copies the driver cannot make are drawn into the job. `spooler::open_preferences` opens the driver's own Printing
  Preferences window.
* **`wia`** (feature `scan`, default): WIA 2.0 scanners as a `printscan::scan` backend, registered
  as `wia` with device ids `wia:<WIA id>`. Listing opens no device; reading details opens it;
  only `acquire` feeds paper.

```rust,ignore
printscan::print::register(Box::new(printscan_windows::spooler::WindowsSpooler::default()))?;
printscan_windows::wia::register();
```

An application that manages its own colour can print RGB already converted to the printer's
profile through the spooler: with `JobOptions::application_managed_colour` the ticket asks the
driver not to colour-manage (`PageColorManagement` = `None`, `PageDestinationColorProfile` =
`Application`) where it offers that, and a driver that would convert the colours anyway refuses the
job. The rendering intent becomes `PageICMRenderingIntent`: [docs/colour.md](docs/colour.md).

## `unsafe`

WIA is COM, and COM calls cannot be made without `unsafe`. The crate denies `unsafe` and allows it
only in two modules, `wia::ffi` (small wrappers around single COM calls and `PROPVARIANT` reads)
and `wia::sinks` (the COM callback objects), every block with a `// SAFETY:` comment:
[docs/wia-unsafe-audit.md](docs/wia-unsafe-audit.md). The spooler path is safe code over winprint.

## Other platforms

The crate builds everywhere; off Windows only the WIA rule tables (HRESULT meanings, status bits,
sheet-length rules) are compiled, so an app can depend on it unconditionally.

## Examples

```text
cargo run -p printscan-windows --example wia_scan -- list               # opens no device
cargo run -p printscan-windows --example wia_scan -- details <id>       # reads properties
cargo run -p printscan-windows --example capabilities -- "Printer name" # reads the driver, prints nothing
cargo run -p printscan-windows --example colour_diag -- render <out.bmp>
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

The spooler backend uses [winprint](https://github.com/ArcticLampyrid/winprint.rs) (BSD-3-Clause)
as a dependency.
