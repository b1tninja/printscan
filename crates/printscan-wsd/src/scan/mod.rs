//! WS-Scan (Web Services for Devices) as a [`printscan::scan::Backend`]. Cargo feature `scan`; not
//! compiled for `wasm32`.
//!
//! [`WsdScanBackend`] scans from network scanners that host the WS-Scan `ScannerService`, over this
//! crate's SOAP 1.2 / WS-Addressing, MTOM and URL policy. It registers as `wsd` and names its
//! scanners by their service URL.
//!
//! # Nothing happens until the app asks
//!
//! * The backend **lists only the scanners the app added** ([`WsdScanBackend::add`]). It does no
//!   discovery and sends no multicast; finding scanners is the app's separate decision
//!   ([`crate::discovery`] builds the messages).
//! * Building a backend, adding a scanner and listing scanners open no connection. Details and
//!   status are one `GetScannerElements` each; a scan is the job below.
//! * URLs must be `http://` with a numeric loopback, link-local or private address.
//!
//! # A scan
//!
//! `GetScannerElements` (configuration and status, for the plan and a preflight), `CreateScanJob`
//! with a [`ticket::Ticket`] planned from the request, then `RetrieveImage` until the scanner
//! answers `ClientErrorNoImagesAvailable` or every image asked for arrived. A job stopped early
//! (cancelled, failed, or an open-ended feeder job that reached [`printscan::scan::MAX_PAGES`]) is
//! ended with `CancelJob`.
//!
//! * **Formats.** A request that names a format gets that format or an error. One that leaves it
//!   open gets the first lossless format the scanner lists (PNG, uncompressed TIFF, DIB, then EXIF
//!   and JFIF), and when the scanner refuses it with `ClientErrorFormatNotSupported` the job is
//!   retried in the order Microsoft's WSD scan driver uses (PNG, EXIF for colour or grey, G4 for
//!   black and white, uncompressed TIFF).
//! * **Source.** WS-Scan never says that paper is in the feeder, so `Source::Auto` is the glass
//!   when the scanner has one. A status that names `InputTrayEmpty` makes the feeder known empty.
//! * **Cancel.** The request's cancel token stops the job between images; an image already on
//!   the wire is read to its timeout first.
//! * **Faults** become [`printscan::scan::ScanError::Native`] with the sub-code as the native code
//!   ([`fault::DOMAIN`]) and the cause it names; a fault that names none is restated from a fresh
//!   status read when the status names a jam, an open cover, a double feed or a lamp fault.

pub mod client;
pub mod fault;
pub mod model;
pub mod ns;
pub mod ticket;

mod backend;

pub use backend::WsdScanBackend;
pub use client::{Client, Section};
