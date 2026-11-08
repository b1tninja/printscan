# Changelog

All notable changes to `printscan-cups` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Experimental, not yet published (`publish = false`).

The first version: `CupsBackend`, a `printscan::print` backend named `cups` for the queues of a CUPS
scheduler, reached over IPP (by default at `ipp://127.0.0.1:631/`). It lists the queues
(`CUPS-Get-Printers`), reads the default queue (`CUPS-Get-Default`), reads a queue's capabilities
(`Get-Printer-Attributes`), prints with consent, and follows, lists, finds and cancels jobs, through
`printscan-ipp`'s client and backend.

Every public struct and enum is `#[non_exhaustive]`. Structs are built with `new` and the `with_*`
setters, or by assigning fields to a value from one of those; a `match` on an enum needs a `_` arm.
