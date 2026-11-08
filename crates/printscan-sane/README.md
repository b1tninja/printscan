# printscan-sane

**Experimental, not yet published.** The crate is `publish = false` and release-plz leaves it out
until it has been used against real daemons; its API may change without a major version.

SANE scanners for [printscan](https://github.com/b1tninja/printscan)'s `printscan::scan` registry,
through the SANE network protocol that `saned` speaks on TCP port 6566. Pure Rust, no `unsafe`, no
`libsane` at build or run time: it builds and tests on every platform and scans from any machine
that can reach a `saned`, the local one (`127.0.0.1`) included. Requires Rust 1.95.

* **Nothing happens until the app asks.** The backend lists only the daemons and devices the app
  added; it does no discovery and sends no multicast. Building it and adding to it open no
  connection; listing asks each added daemon for its devices, and details and a scan open the
  device.
* **Local network only.** Addresses are numeric loopback, link-local or private ones; host names
  and public addresses are refused. A scan's data connection goes to the control connection's
  address, never to one the daemon names.
* **No credentials.** A daemon that asks for a user name and password gets neither: the error
  carries the native code `AUTHORIZATION_REQUIRED` and the resource it named.

```rust,ignore
let sane = printscan_sane::scan::SaneBackend::new();
sane.add_host("192.168.1.20")?;             // a saned; port 6566 unless given
printscan::scan::register(Box::new(sane))?;
// Devices are listed as `sane:<address>:<port>/<device name>`.
let listing = printscan::scan::scanner_list()?;
```

## Layers

```text
scan      SaneBackend (feature `scan`, default): the registry-facing backend; the standard options
          mapped to settings and capabilities, frames as PNG pages, statuses as scan errors
client    Client: one method per remote procedure (init, get devices, open, close, option
          descriptors, control option, get parameters, start, cancel, exit); DataStream
protocol  statuses, option descriptors, constraints, parameters, frames, devices
wire      the encoding: big-endian words, counted strings and arrays, pointers; bounded by Limits
policy    SaneHost: a numeric local-network address and port
error     Error, with is_retryable() and retry_safety()
```

## Scanning

* **Pages are PNG.** SANE sends raw samples; each image becomes a PNG (1, 8 or 16 bits, grey or
  colour, three-pass colour interleaved) carrying the resolution the device took.
* **Options.** `source` (`Flatbed`, `ADF`, `ADF Duplex`, ...), `mode` (`Lineart`, `Gray`, `Color`,
  ...), `resolution` / `x-resolution` / `y-resolution`, `tl-x` `tl-y` `br-x` `br-y` (millimetres or
  pixels), `brightness`, `contrast`, `threshold`. SANE has one option set per device, so the glass
  and the feeder report the same choices. A resolution the device does not list is refused before
  anything is scanned.
* **Source.** SANE cannot see paper before a scan: `Source::Auto` is the feeder when there is one,
  and a feeder that gives no page at all is retried once on the glass.
* **Statuses.** The native code is the status (domain `sane`, `sane 7 (SANE_STATUS_NO_DOCS)`), and
  the cause follows it: `NO_DOCS` an empty feeder, `JAMMED` a jam, `COVER_OPEN` an open cover,
  `DEVICE_BUSY` / `WARMING_UP` / `HW_LOCKED` busy (which `ScanJob` may retry by itself),
  `IO_ERROR` offline (the device is checked first). Pages that arrived before a failure are kept.
* **Cancel.** The cancel token stops the scan between images, and during one by shutting the data
  connection down; the device is then cancelled.

`cargo run -p printscan-sane --example sane_scan -- list 127.0.0.1` lists a daemon's devices;
`details` reads one device's options without scanning, and `scan` scans.

## Not yet

* Credentials (the authorize procedure, with the daemon's MD5 challenge).
* A `libsane` backend (dlopen on Unix) for drivers loaded in-process, without a daemon.
* Options beyond the standard ones (gamma tables, buttons, sensors), and preflight status.

## Tests

Unit tests beside each module (hostile lengths and counts, the encoding, the address policy, the
option mapping, frame assembly for every depth and layout), and `tests/sane_backend.rs`, which runs
the backend against a fake `saned` written in the test (its own encoder, separate from the
crate's) on loopback TCP: listing, details, glass, feeder, duplex, Auto's fallback, a jam, an empty
feeder, a busy device, an open cover, credentials, cancel mid-transfer, three-pass colour and a
hostile daemon. No test contacts anything but loopback.

```text
cargo test -p printscan-sane
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
