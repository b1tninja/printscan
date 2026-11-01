# printscan

Printing and scanning for Rust applications: one job and device vocabulary for both, a scanner contract with a
backend registry, and the network and platform backends behind it.

| Crate | What it holds |
|---|---|
| [`printscan`](crates/printscan) | `printscan::job`: job and device vocabulary shared by print and scan (states, ids, retry advice). `printscan::scan` (feature `scan`, default): the scanner contract, backend registry, `ScanJob` and page handling. `printscan::print` (feature `print`, default): the print backend registry, tracked jobs, document conversion and job watching. `printscan::devices` (feature `devices`): the scan job journal for large piles and manual duplex. |
| [`printscan-ipp`](crates/printscan-ipp) | An IPP (RFC 8010/8011) print backend: plain HTTP to numeric local addresses, no discovery, consent for every job. |
| [`printscan-windows`](crates/printscan-windows) | The Windows print spooler as a backend, on [`winprint`](https://github.com/ArcticLampyrid/winprint.rs). Colour on that path: [docs/colour.md](crates/printscan-windows/docs/colour.md). |
| [`printscan-wsd`](crates/printscan-wsd) | Web Services for Devices, and with feature `print` (default) a WS-Print backend for `printscan::print`: bounded XML, SOAP 1.2 and WS-Addressing, MTOM, faults, a local-network-only URL policy, WS-Transfer, WS-Eventing and WS-Discovery messages, and a blocking HTTP transport. |

[tools/print-diag](tools/print-diag) prints diagnostic pages (colour, registration, tone ramps, geometry) and measures them from a scan.

Planned: a WS-Scan backend, CUPS, and WIA scanning in `printscan-windows`.

The test devices are separate projects, so a parser bug cannot pass on both sides:
[fakescan](https://github.com/b1tninja/fakescan) (WSD and WIA scanner) and
[fakeprint](https://github.com/b1tninja/fakeprint) (WSD printer).

## History

The crates started as `craft-job`, `craft-scan`, `craft-devices`, `craft-wsd` and the backend layer of `craft-print`, written for the
[Craft apps](https://github.com/storytold/craft-libs) and proposed there; their history is kept here.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
