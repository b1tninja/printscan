# printscan-macos

macOS scanning for [printscan](https://github.com/b1tninja/printscan). Requires Rust 1.95.

> **Experimental, not yet published.** The crate has `publish = false` and is left out of
> release-plz. Its Objective-C layer compiles for macOS but has not been run against a scanner;
> the rules underneath it (capabilities, settings, the session state machines) are tested on every
> platform. No other printscan crate depends on it.

* **`ica`** (feature `scan`, default): scanners reached through Apple's ImageCaptureCore (the
  flatbed and the document feeder) as a `printscan::scan` backend, registered as `ica` with device
  ids `ica:<persistent id>`. Listing browses and opens no device; reading details opens a session
  and reads every functional unit; only `acquire` feeds paper. Pages are the files ImageCaptureCore
  writes (TIFF by default; PNG, JPEG or BMP on request) into a directory that is removed after the
  scan.

```rust,ignore
printscan_macos::ica::register(); // local scanners only
// or, with network (Bonjour, shared) scanners browsed too:
use printscan_macos::ica::{Ica, IcaOptions};
printscan::scan::register(Box::new(Ica::with_options(IcaOptions::default().with_network(true))))?;
```

Printing on macOS is a separate CUPS backend; this crate scans only.

## Threads and the run loop

ImageCaptureCore calls its delegates on the main thread. The backend makes every call on the main
queue and waits for the answers: called from the main thread (a command-line tool), it runs the
main run loop while it waits; called from a worker thread, it needs the main thread to be running
its run loop, as an AppKit app's is. Every step has a deadline, so a blocked main thread is an
error, not a hang. [docs/run-loop.md](docs/run-loop.md) has the design.

An app that uses the scanner may need the sandbox entitlements for USB or network devices; macOS
refusing access is reported as an error that says so.

## `unsafe`

The objc2 bindings mark ImageCaptureCore's methods `unsafe fn`. The crate denies `unsafe` and
allows it only in two modules, `ica::ffi` (small wrappers around the calls) and `ica::delegate`
(the Objective-C class ImageCaptureCore calls back into), every block with a `// SAFETY:`
comment: [docs/ica-unsafe-audit.md](docs/ica-unsafe-audit.md).

## Other platforms

The crate builds everywhere; off macOS only the rule tables, the capability mapping and the
session state machines are compiled, and `ica::register` returns `false`.

## Examples

```text
cargo run -p printscan-macos --example ica_scan -- list               # opens no device
cargo run -p printscan-macos --example ica_scan -- details <id>       # opens a session, reads the units
cargo run -p printscan-macos --example ica_scan -- scan <id> out.tiff # one page from the glass
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

The backend uses the [objc2](https://github.com/madsmtm/objc2) crates (MIT, or Zlib OR
Apache-2.0 OR MIT) as dependencies.
