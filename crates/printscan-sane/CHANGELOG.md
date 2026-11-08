# Changelog

All notable changes to `printscan-sane` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

The first version, experimental and not yet published (`publish = false`; release-plz leaves it
out). A client of the SANE network protocol and a `printscan::scan` backend on it: init, get
devices, open, option descriptors, control option, get parameters, start, the data connection,
cancel, close and exit; the standard options (source, mode, resolution, scan area, brightness,
contrast, threshold) mapped to the scan settings and capabilities; frames as PNG pages; statuses as
scan errors with their native code; a request for credentials reported as
`AUTHORIZATION_REQUIRED`.

Every public struct and enum is `#[non_exhaustive]`. Structs are built with `new`, `Default` and
the `with_*` setters; a `match` on an enum needs a `_` arm.
