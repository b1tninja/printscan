# Contributing

## Before a pull request

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
```

CI runs the same on Linux, Windows and macOS, and also checks the minimum Rust version (1.95), the
`wasm32` builds, the published packages (`cargo package --workspace`) and every dependency's
licence and advisories (`cargo deny check`, configured in `deny.toml`). Its `live` job runs every test
against fakescan and fakeprint built from their repositories; [docs/testing.md](docs/testing.md) says
how to do the same locally.

## Rules the code keeps

* **No `unsafe`** outside `printscan-windows`' `wia::ffi` and `wia::sinks`, each block with a
  `// SAFETY:` comment (crates/printscan-windows/docs/wia-unsafe-audit.md), and the experimental
  `printscan-macos`' `ica::ffi` and `ica::delegate`, whose sign-off is pending
  (crates/printscan-macos/docs/ica-unsafe-audit.md).
* **Nothing on the network until the app asks.** Backends list only the devices the app added,
  discover nothing on their own, and connect only to numeric local-network addresses. A print job
  on a network backend needs a `UserConfirmed`.
* **Hostile replies never panic.** Parsers are bounded (size, depth, count) and return errors.
* **Tests use fake devices.** No test contacts anything but loopback. The fakes that run as
  separate programs are [fakescan](https://github.com/b1tninja/fakescan) and
  [fakeprint](https://github.com/b1tninja/fakeprint).
* **The specification is the reference, not one device.** A device's quirk is handled as an
  opt-in mode named for what it does, not for the vendor or model that needs it.
* **No specification files in the repository.** Schemas and WSDLs whose licences do not allow
  redistribution stay on the developer's machine.
* **Every public struct and enum is `#[non_exhaustive]`**, so a field or variant can be added in a
  minor release. A struct comes with a way to build it outside the crate: `new` with what it cannot
  do without, `Default` when every field has a sensible default, and `with_*` setters (the
  crate-private `setters!` macro) on the types callers configure. A `match` on another crate's enum
  says what an unknown variant means (a warning, `None`, the device's default), never `unreachable!`.

## Commits and changelogs

Commit messages say what changed and why, in the form `area: what changed`. release-plz builds
each crate's `CHANGELOG.md` entry from them, and the release pull request is the place to edit it.

## License

Contributions are licensed as the project is: MIT OR Apache-2.0, without any additional terms.
