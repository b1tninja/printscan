# Changelog

All notable changes to `printscan` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `print`: `JobOptions::application_managed_colour` (and `with_application_managed_colour`): the
  document's colours are already in the printer's colour space and are not to be colour-managed
  again, with `JobHint::ApplicationManagedColour` for a backend that can tell the printer. Both are
  additions to `#[non_exhaustive]` types; the default is `false`, as before.

## [0.1.0]

The first release.

Every public struct and enum is `#[non_exhaustive]`. Structs are built with `new`, `Default` and
the `with_*` setters, or by assigning fields to a value from one of those; a `match` on an enum
needs a `_` arm.
