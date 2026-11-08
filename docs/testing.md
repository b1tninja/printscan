# Testing

`cargo test --workspace --all-features` runs everything that needs no device: unit tests, scripted
in-memory devices for each protocol, and loopback HTTP servers. The live tests in this document run
the backends against fake devices that are separate programs, so a parser bug cannot pass on both
sides; without their variable set they skip themselves.

No test talks to anything but loopback. Real devices are for people, by hand (see the examples).

## Which fake covers what

| Backend | Fake | Live tests | Variable |
|---|---|---|---|
| WS-Scan (`printscan-wsd`, feature `scan`) | [fakescan](https://github.com/b1tninja/fakescan) | `crates/printscan-wsd/tests/live_fakescan.rs` | `PRINTSCAN_FAKESCAN_URL` |
| WS-Print (`printscan-wsd`, feature `print`) | [fakeprint](https://github.com/b1tninja/fakeprint) | `crates/printscan-wsd/tests/wsd_live.rs` | `PRINTSCAN_FAKEPRINT_URL` |
| WSD metadata and WS-Eventing (`printscan-wsd`) | fakeprint | `crates/printscan-wsd/tests/live_loopback.rs` | `PRINTSCAN_FAKEPRINT_URL` |
| IPP (`printscan-ipp`) | fakeprint | `crates/printscan-ipp/tests/live_fakeprint.rs` | `PRINTSCAN_FAKEPRINT_URL` |
| SANE (`printscan-sane`) | a fake `saned` inside the test (in `cargo test`, every platform) | `crates/printscan-sane/tests/sane_backend.rs` | |
| WIA (`printscan-windows`) | none automated | | |
| Windows spooler (`printscan-windows`) | a temporary print-to-file printer (in `cargo test`, Windows only) | `crates/printscan-windows/tests/spooler.rs` | |

Each variable is the fake's base URL, `http://127.0.0.1:PORT`; the tests derive the endpoints
(`/ScannerService`, `/WebServices/PrinterService`, `ipp://…/ipp/print`) and the control plane
(`/v1/…`) from it.

WIA has no automated fake: fakescan can be installed as a WSD scanner that Windows then exposes
through WIA, but that takes a person at Windows' Add device dialog
([fakescan docs/windows.md](https://github.com/b1tninja/fakescan/blob/main/docs/windows.md)).

## Running the live tests

With both fakes checked out beside this repository and built:

```sh
cargo build --release -p fakescan --manifest-path ../fakescan/Cargo.toml
cargo build --release -p fakeprint --manifest-path ../fakeprint/Cargo.toml
python tools/with-fakes.py --fakescan ../fakescan --fakeprint ../fakeprint -- cargo test --workspace --all-features
```

`tools/with-fakes.py` starts fakescan `--headless` (loopback, an ephemeral port, no WS-Discovery,
and a `FAKESCAN_READY` line with its URLs) and fakeprint on `127.0.0.1:3921` with discovery and
JetDirect off, runs the command with both variables set, and stops both when it ends. CI's `live`
job does the same with both fakes at pinned commits (`FAKESCAN_REF`, `FAKEPRINT_REF` in
`.github/workflows/ci.yml`).

By hand, one fake at a time:

```sh
fakescan --headless --exit-on-stdin-close --profiles profiles --profile wia-lossless
# FAKESCAN_READY {"base_url":"http://127.0.0.1:57262", ...}
PRINTSCAN_FAKESCAN_URL=http://127.0.0.1:57262 cargo test -p printscan-wsd --test live_fakescan

fakeprint --listen 127.0.0.1:3921 --no-discovery --no-jetdirect --profile generic-wsd --inbox /tmp/inbox
PRINTSCAN_FAKEPRINT_URL=http://127.0.0.1:3921 cargo test -p printscan-wsd --test wsd_live --test live_loopback
PRINTSCAN_FAKEPRINT_URL=http://127.0.0.1:3921 cargo test -p printscan-ipp --test live_fakeprint
```

## What the live tests check

**WS-Scan against fakescan** (profile `wia-lossless`). Each test resets fakescan
(`POST /v1/reset`) and sets the scene through its control plane:

* a glass scan is one page, in the lossless format, with the density the file carries;
* an open-ended feeder scan takes every sheet (`POST /v1/pages {"count":3}`);
* an empty feeder is named before a job is made;
* a jam after two sheets keeps them and names the jam (fault plan `jam` after 2 pages);
* a reply cut off mid-page is a failure, not a cancel (fault plan `abort` at half of page 2).

**WS-Print and IPP against fakeprint** (profile `generic-wsd`): the printer describes itself, and a
PDF prints to `Completed` and reaches the printer (fakeprint's `captured_jobs` goes up by one).
WSD metadata and the WS-Eventing life cycle (subscribe, renew, status, unsubscribe) run against the
same fakeprint.

## Not covered by a fake yet

* **The Cancel key on a scanner's panel.** A real scanner cut the image reply off and dropped the
  job; fakescan's `abort` cuts the reply but leaves the job open. The scripted scanner in
  `tests/wsd_scan.rs` covers it (the job status says `JobCanceledAtDevice`, or the scanner answers
  `ClientErrorJobCancelled`).
* **A scanner that delivers exactly the area asked for.** fakescan delivers the same page size
  whatever the ticket asks, like the scanner its Brother-shaped profiles model (at 300 dpi
  2480 x 3440 pixels), which a real scanner also did. The backend reports the size the image has.
* **WIA and printing to a real queue** (above).
