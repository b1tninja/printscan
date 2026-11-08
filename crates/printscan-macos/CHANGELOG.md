# Changelog

All notable changes to `printscan-macos` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Experimental and not published (`publish = false`, left out of release-plz).

### Added

- `ica`: ImageCaptureCore scanners as a `printscan::scan` backend named `ica`, on the objc2 crates:
  browsing local scanners (network ones when the app turns them on), sessions that read the
  flatbed and document feeder units into printscan's capabilities, scans with file-based
  transfer, cancellation, and ImageCaptureCore's return codes as printscan errors.
- The rule tables (`ica::caps`, `ica::plan`, `ica::errors`, `ica::model`) and the session state
  machines build and are tested on every platform.

Every public struct and enum is `#[non_exhaustive]`. Structs are built with `new`, `Default` and
the `with_*` setters, or by assigning fields to a value from one of those; a `match` on an enum
needs a `_` arm.
