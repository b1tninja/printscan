# printscan-cups

**Experimental, not yet published.** This crate is beta: it is not on crates.io, release-plz leaves
it out of releases (`publish = false`), and no other printscan crate depends on it. It has been
tested against a fake scheduler only.

A CUPS print backend for [printscan](https://github.com/b1tninja/printscan)'s `printscan::print`
registry: the print queues of Linux, macOS and other CUPS systems. No `unsafe`, no libcups and no
C dependency. Requires Rust 1.95. Not compiled for `wasm32`.

* **IPP to the scheduler, not libcups.** CUPS listens on `localhost:631`; the backend speaks IPP to
  it with [`printscan-ipp`](https://crates.io/crates/printscan-ipp)'s client, so it builds and
  tests on every OS. The scheduler is `ipp://127.0.0.1:631/` unless the app names another numeric
  local address with `with_server`.
* **The scheduler's queues.** Listing is one `CUPS-Get-Printers` and one `CUPS-Get-Default`; nothing
  is discovered by this crate and no multicast is sent. Building a backend opens no connection.
* **Consent for every job.** A queue may forward to a network printer, so the backend is
  `BackendClass::Network` and the registry hands it only requests that carry a
  `printscan::print::UserConfirmed`.

```rust,ignore
let cups = printscan_cups::CupsBackend::new();
printscan::print::register(Box::new(cups))?;
// Printers are listed as `cups:<queue>`, jobs as `cups:<queue>#<job id>`.
```

## What it sends

| Call | Requests |
|---|---|
| `list_printers` | `CUPS-Get-Printers`, `CUPS-Get-Default` (to the scheduler's `/`) |
| `printer_details` | `Get-Printer-Attributes` on `/printers/<queue>` (`/classes/<queue>` for a class), `CUPS-Get-Default` |
| `submit` | `CUPS-Get-Default` when the job names no queue, then `printscan-ipp`'s submit on the queue |
| `job_status`, `cancel_job`, `list_jobs`, `find_job` | `Get-Job-Attributes`, `Cancel-Job`, `Get-Jobs` on the queue |

Queue names are checked (no white space, control characters, `/ \ ? # ' "`, `.` or `..`, at most
127 bytes) and percent-encoded in the URI. Jobs are owned by the `requesting-user-name`, a fixed
`printscan` unless `with_user_name` says otherwise.

## Not yet

* The scheduler's Unix domain socket (`/run/cups/cups.sock`), with which CUPS knows the local user
  from the socket's peer credentials. Over TCP, CUPS may ask for authentication for operations its
  policy restricts, such as cancelling another user's job (HTTP 401, reported as an error).
* TLS, authentication, PPD options and CUPS's administrative operations.
* Tests against a real scheduler: `tests/fake_cups.rs` is a fake on a loopback port.

`cargo run -p printscan-cups --example cups_queues` lists the local scheduler's queues without printing.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
