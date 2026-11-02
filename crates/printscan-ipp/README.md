# printscan-ipp

An IPP (RFC 8010/8011, PWG 5100.x) print backend for
[printscan](https://github.com/b1tninja/printscan)'s `printscan::print` registry. No `unsafe`.
Requires Rust 1.95. Not compiled for `wasm32`.

* **Nothing happens until the app asks.** The backend lists only the printers the app added; it
  does no discovery and sends no multicast. Building it, adding a printer and listing printers open
  no connection; `printer_details` (one `Get-Printer-Attributes`) and a job do.
* **Local network only.** Printer URIs are `ipp://` or `http://` with a numeric loopback,
  link-local or private address. Host names, public addresses, redirects, proxies and `ipps://`
  are refused: there is no TLS.
* **Consent for every job.** It is a network backend, so the registry hands it only requests that
  carry a `printscan::print::UserConfirmed`.

```rust,ignore
let ipp = printscan_ipp::IppBackend::with_printers(["ipp://192.168.1.20/ipp/print"])?;
printscan::print::register(Box::new(ipp))?;
// Printers are listed as `ipp:<printer uri>`, jobs as `ipp:<printer uri>#<job id>`.
```

## Layers

```text
IppBackend  the registry-facing backend: list, details, submit, status, cancel, find_job
client      one method per operation: Get-Printer-Attributes, Validate-Job, Print-Job, Create-Job,
            Send-Document, Get-Job-Attributes, Get-Jobs, Cancel-Job, over any transport
plan        the neutral job settings mapped onto job attributes against the printer's capabilities
model       printer and job attributes
codec       the RFC 8010 binary encoding, strict and bounded
error       IppError, classified: transient or not, safe to repeat or not, where it failed
```

The HTTP transport and the address policy come from
[`printscan-wsd`](https://crates.io/crates/printscan-wsd). `submit` returns as soon as the printer
accepts the job; following it is a separate, polled `Get-Job-Attributes`. After a failure whose
outcome is unknown, `find_job` looks for the job (by an idempotency tag in `job-name`) before
anything is resent. The status table is in
[printscan's docs/print/ipp.md](https://github.com/b1tninja/printscan/blob/main/crates/printscan/docs/print/ipp.md).

`cargo run -p printscan-ipp --example ipp_probe -- <printer uri>` reads a printer without printing.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
