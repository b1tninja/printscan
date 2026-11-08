# printscan

The core of [printscan](https://github.com/b1tninja/printscan): one job and device vocabulary for
printing and scanning, and the contracts and registries the backends plug into. No `unsafe`, no
platform code, no network code; the backends live in their own crates. Requires Rust 1.95.

```text
job      states, ids, phases, retry advice and neutral errors shared by print and scan
scan     the scanner contract (Backend), the backend registry, ScanJob, settings, pages and image
         handling (BMP/PNG/TIFF, black-and-white, sheet length)                    feature `scan`
print    the print backend registry, consent (UserConfirmed), tracked jobs, document conversion,
         PWG raster and job watching                                               feature `print`
devices  the scan job journal for large piles and manual duplex                    feature `devices`
```

## Backends

| Crate | Backends |
|---|---|
| [`printscan-wsd`](https://crates.io/crates/printscan-wsd) | WS-Print and WS-Scan (Web Services for Devices) |
| [`printscan-ipp`](https://crates.io/crates/printscan-ipp) | IPP printers |
| [`printscan-windows`](https://crates.io/crates/printscan-windows) | the Windows print spooler, WIA 2.0 scanners |
| [`printscan-sane`](https://github.com/b1tninja/printscan/tree/main/crates/printscan-sane) | SANE scanners through `saned` (experimental, not yet published) |

An app registers the backends it wants at start-up; ids are namespaced by backend (`wsd:…`,
`ipp:…`, `wia:…`, `sane:…`), listing aggregates every backend and reports one that fails without hiding the
others, and every call routes by prefix. A network backend lists only devices the app added and
needs a `UserConfirmed` for every print job.

## Features

| Feature | Default | Adds |
|---|---|---|
| `scan` | yes | `printscan::scan` (`thiserror`, `image`, `png`, `tiff`) |
| `print` | yes | `printscan::print` (no extra dependencies) |
| `serde` | | `Serialize`/`Deserialize` on the plain data types ([docs/job.md](docs/job.md)) |
| `devices` | | `printscan::devices` (with `scan` and `serde`; `serde_json`, `tempfile`) |
| `testing` | | `printscan::print::testing`: fake backends for an app's tests. Never in a shipped binary |

With no features the crate is `printscan::job` only, and builds for `wasm32`.

## Documentation

[docs/scan.md](docs/scan.md), [docs/scan-job.md](docs/scan-job.md), [docs/job.md](docs/job.md),
[docs/devices.md](docs/devices.md), and for printing [docs/print/](docs/print/job-lifecycle.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
