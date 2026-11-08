# printscan-escl

**Experimental, not yet published.** The crate has `publish = false` and release-plz leaves it out
of releases; no other printscan crate depends on it. Its API may change before a first release.

An eSCL ("AirScan", the Mopria and Apple network scan protocol) scan backend for
[printscan](https://github.com/b1tninja/printscan)'s `printscan::scan` registry. Most current
network scanners and multifunction printers speak it, whatever the OS. No `unsafe`, no panics on
hostile replies. Requires Rust 1.95. Not compiled for `wasm32`.

* **Nothing happens until the app asks.** The backend lists only the scanners the app added by
  address; it does no discovery (no mDNS) and sends no multicast. Building it, adding a scanner and
  listing scanners open no connection; details, status and a scan do.
* **Local network only.** A scanner is named by its eSCL root, `http://` or `https://` with a
  numeric loopback, link-local or private address (`http://192.168.1.20/eSCL`; a URL without a path
  gets `/eSCL`). Host names, public addresses, redirects and proxies are refused. The job a scanner
  creates is always fetched from that same scanner: only the path of its `Location` is used.
* **Bounded.** XML replies are capped at 4 MiB, 40 levels and 100,000 nodes, without DTDs; a page
  at 256 MiB.

```rust,ignore
let escl = printscan_escl::EsclBackend::new();
let id = escl.add("http://192.168.1.20/eSCL")?;
printscan::scan::register(Box::new(escl))?;
// Scanners are listed as `escl:http://192.168.1.20:80/eSCL`.
```

`cargo run -p printscan-escl --example escl_scan -- details <url>` reads a scanner without scanning.

## A scan

`GET ScannerCapabilities` and `GET ScannerStatus`, a preflight (an empty feeder, a jam or an open
hatch stops the scan before a job is made), `POST ScanJobs` with the planned `ScanSettings`, then
`GET <job>/NextDocument` until the scanner answers 404 or every page asked for arrived; a job
stopped early is ended with `DELETE <job>`.

| Request | eSCL |
|---|---|
| `Source::Auto` | the feeder when the status says `ScannerAdfLoaded`, otherwise the glass |
| `duplex` | `scan:Duplex` on the feeder; `pages` counts sheets, so twice as many sides arrive |
| `dpi`, `dpi_y` | the next listed resolution up, or aligned into the published range |
| `color` | `RGB24`, `Grayscale8`, `BlackAndWhite1` (grey, thresholded in software, when that is not listed) |
| `format` | the MIME type it names (`png`, `jpeg`, `tiff`, `pdf`) or an error; none: PNG, TIFF, JPEG, then PDF |
| `region`, `page_size` | `pwg:ScanRegion` in three hundredths of an inch, checked against the source's area |
| `brightness`, `contrast`, `threshold` | aligned into the published `*Support` range; left out when none is published |
| `intent` | `Text` as `Document`, `Color` as `Photo`, `BestPreview` as `Preview`, when listed |

| What happened | `InterruptKind` | Native code |
|---|---|---|
| connection refused, unreachable, timed out | `Offline` | |
| HTTP 503, 429 or 409 | `Busy` | `http` + status |
| `NextDocument` 503 for longer than the busy wait (60 s) | `Busy` | `http` 503 |
| feeder empty (status, refused job, or no document from the feeder) | `FeederEmpty` | `escl-adf` `ScannerAdfEmpty`, or `http` 404 |
| `ScannerAdfJam`, `HatchOpen`, `MultipickDetected`, `Mispick` | `Jam`, `CoverOpen`, `MultiFeed`, `PaperProblem` | `escl-adf` + state |
| the job `Canceled` at the scanner's panel | `Cancelled` | `escl-job` `Canceled` |
| the job `Aborted` | the feeder state's kind, else `Other` | `escl-job` `Aborted` |

Pages that arrived before a failure are kept (`ScanError::Incomplete`).

## TLS

`https` needs the cargo feature `tls` (rustls with the ring provider; off by default because ring
builds C code) and a `TlsMode` (`EsclBackend::with_tls`). A scanner's certificate is one it made
itself, so there is no built-in trust store: the app passes the certificates it trusts
(`TlsMode::Trust`, `TlsMode::trust_pem`; checked the usual way, the scanner's IP address included),
or explicitly accepts any certificate (`TlsMode::AcceptAnyCertificate`: encrypted, but not proof of
which device answers). `EsclBackend::new()` refuses `https` URLs.

## Layers

```text
EsclBackend  the registry-facing backend: list, details, status, diagnostic dump, acquire
client       one method per resource: ScannerCapabilities, ScannerStatus, ScanJobs, NextDocument, DELETE
settings     the ScanSettings document planned from printscan's ScanSettings
model        ScannerCapabilities and ScannerStatus
classify     errors and scanner states as InterruptKinds, HandlingStatus and native codes
url          ScannerUrl: the address policy
transport    the Transport trait (GET, POST, DELETE); http: HttpTransport on ureq
```

The address check, the bounded XML checks, `NetworkLimits` and `Retry-After` parsing come from
[`printscan-wsd`](https://crates.io/crates/printscan-wsd); its URL type and transport are plain-`http`
POST only, so this crate has its own.

## Not yet

* Discovery (`_uscan._tcp` / `_uscans._tcp` over mDNS): the app adds scanners by address.
* A fake eSCL device in [fakescan](https://github.com/b1tninja/fakescan) for live tests; the tests
  here use a scripted loopback scanner (`tests/escl_backend.rs`).
* Certificate pinning by fingerprint (ureq does not take a custom verifier), and an https round
  trip in the tests.
* Setting profiles are merged per source, so a combination a scanner lists separately (a format
  only at some resolutions) is not kept apart.
* `ScanImageInfo`, `scan:NumberOfPages`, blank-page detection, `Grayscale16`/`RGB48`, and an
  idempotency tag through `ScanJobs` (`tag_support` is `None`).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
