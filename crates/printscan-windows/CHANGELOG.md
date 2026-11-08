# Changelog

All notable changes to `printscan-windows` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `spooler`: application-managed colour. A job with `JobOptions::application_managed_colour` sets
  `PageColorManagement` = `None` and `PageDestinationColorProfile` = `Application` where the driver
  lists them. A driver that lists colour management without `None`, or does not keep it in the
  validated ticket, refuses the job before anything is sent; one with no colour-management setting
  prints it with a warning.
- `spooler`: `JobOptions::rendering_intent` becomes `PageICMRenderingIntent` where the driver lists
  the intent (relative with black point compensation as relative colorimetric, with a warning); an
  intent it lacks or replaces is a warning. It was a warning for every intent.
- `spooler`: `printer_details` lists `JobHint::RenderingIntent` and
  `JobHint::ApplicationManagedColour` for a driver that offers them.

## [0.1.0]

The first release.

Every public struct and enum is `#[non_exhaustive]`. Structs are built with `new`, `Default` and
the `with_*` setters, or by assigning fields to a value from one of those; a `match` on an enum
needs a `_` arm.
