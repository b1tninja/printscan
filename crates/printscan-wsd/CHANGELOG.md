# Changelog

All notable changes to `printscan-wsd` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- A job with `JobOptions::application_managed_colour` gets a warning: WS-Print has no setting for
  it, so the printer may colour-manage the document.

## [0.1.0]

The first release.

Every public struct and enum is `#[non_exhaustive]`. Structs are built with `new`, `Default` and
the `with_*` setters, or by assigning fields to a value from one of those; a `match` on an enum
needs a `_` arm.

The XML parser is not part of the API: a service-specific reader (`SoapClient::call`,
`call_with_parts`, `envelope::read_reply`, `eventing::subscribe_with`, `Notification::with_body`)
gets an `xml::Element`.
