# printscan

Printing and scanning for Rust applications: one job and device vocabulary for both, a scanner contract with a
backend registry, and the network and platform backends behind it.

| Crate | What it holds |
|---|---|
| [`printscan`](crates/printscan) | `printscan::job`: job and device vocabulary shared by print and scan (states, ids, retry advice). `printscan::scan` (feature `scan`, default): the scanner contract, backend registry, `ScanJob` and page handling. `printscan::devices` (feature `devices`): the scan job journal for large piles and manual duplex. |
| [`printscan-wsd`](crates/printscan-wsd) | Web Services for Devices: bounded XML, SOAP 1.2 and WS-Addressing, MTOM, faults, a local-network-only URL policy, WS-Transfer, WS-Eventing and WS-Discovery messages, and a blocking HTTP transport. |

Planned: the print contract (`printscan::print`), WS-Print and WS-Scan backends, an IPP backend on the
[`ipp`](https://crates.io/crates/ipp) crate, CUPS, and a Windows crate on
[`winprint`](https://github.com/ArcticLampyrid/winprint.rs) with WIA scanning.

The test devices are separate projects, so a parser bug cannot pass on both sides:
[fakescan](https://github.com/b1tninja/fakescan) (WSD and WIA scanner) and
[fakeprint](https://github.com/b1tninja/fakeprint) (WSD printer).

## History

The crates started as `craft-job`, `craft-scan`, `craft-devices` and `craft-wsd`, written for the
[Craft apps](https://github.com/storytold/craft-libs) and proposed there; their history is kept here.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
