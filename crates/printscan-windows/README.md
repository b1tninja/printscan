# printscan-windows

Windows printing and scanning for [printscan](https://github.com/b1tninja/printscan). Requires
Rust 1.95.

* **`spooler`** (feature `print`, default): the Windows print spooler as a `printscan::print`
  backend, on [`winprint`](https://crates.io/crates/winprint). A PDF job is drawn by the in-box
  `Windows.Data.Pdf` into the XPS print API at the resolution of the job's PrintTicket, in this
  process: no other program is started. Copies, collation, two-sided, colour, paper, orientation
  and resolution become the PrintTicket, each from the options the driver offers; a setting the
  driver does not offer is named in the job's warnings.
* **`wia`** (feature `scan`, default): WIA 2.0 scanners as a `printscan::scan` backend, registered
  as `wia` with device ids `wia:<WIA id>`. Listing opens no device; reading details opens it;
  only `acquire` feeds paper.

```rust,ignore
printscan::print::register(Box::new(printscan_windows::spooler::WindowsSpooler))?;
printscan_windows::wia::register();
```

An application that manages its own colour can print RGB already converted to the printer's
profile through the spooler: [docs/colour.md](docs/colour.md).

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
