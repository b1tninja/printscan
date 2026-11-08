//! SANE scanners as a [`printscan::scan::Backend`]. Cargo feature `scan` (default).
//!
//! [`SaneBackend`] scans through `saned`, over this crate's [`crate::client::Client`]. It registers
//! as `sane` and names its devices `<address>:<port>/<device name>`.
//!
//! # Nothing happens until the app asks
//!
//! * The backend **lists only what the app added**: daemons ([`SaneBackend::add_host`]), whose
//!   devices it lists by asking them, and single devices ([`SaneBackend::add_device`]), listed
//!   without asking. It discovers nothing.
//! * Building a backend and adding to it open no connection. Listing makes one connection per
//!   added daemon; details, a dump and a scan open the device.
//! * Addresses are numeric loopback, link-local or private ones ([`crate::policy`]).
//!
//! # A scan
//!
//! Open the device and read its option descriptors; set `source`, `mode`, the resolution, the
//! levels and the scan area from the request ([`options`](self#options)); then start, read the
//! frame's parameters and data, and start again for every further page until the feeder answers
//! `SANE_STATUS_NO_DOCS` or every page asked for arrived. The device is reset with a cancel at the
//! end, and closed.
//!
//! * **Pages are PNG.** SANE sends raw samples; each image becomes a PNG (1, 8 or 16 bits, grey or
//!   colour, three-pass colour interleaved) carrying the resolution the device took. A
//!   black-and-white request on a device without a 1-bit mode scans grey and is thresholded.
//! * **Source.** SANE cannot see paper before a scan, so `Source::Auto` is the feeder when there is
//!   one, and an empty feeder (no page at all) is retried once on the glass.
//! * **Cancel.** The request's cancel token stops the scan between images, and during an image by
//!   shutting the data connection down; the device is then cancelled.
//! * **Statuses** become [`printscan::scan::ScanError::Native`] with the status as the native code
//!   ([`DOMAIN`]) and the cause it names ([`interrupt_kind`]): an empty feeder, a jam, an open
//!   cover, a busy or warming device. A daemon that asks for credentials is
//!   [`AUTHORIZATION_REQUIRED`].
//!
//! # Options
//!
//! SANE options are one set per device, whatever the source, so the glass and the feeder report
//! the same choices, read for the source the device is set to. The well-known options are read by
//! their standard names (`source`, `mode`, `resolution`, `x-resolution`, `y-resolution`, `tl-x`,
//! `tl-y`, `br-x`, `br-y`, `brightness`, `contrast`, `threshold`) and source and mode entries by
//! their usual spellings (`Flatbed`, `ADF`, `ADF Duplex`; `Lineart`, `Gray`, `Color`).

mod backend;
mod frame;
mod options;
mod status;

pub use backend::{NAME, SaneBackend};
pub use status::{AUTHORIZATION_REQUIRED, DOMAIN, NET_DOMAIN, interrupt_kind, native, scan_error};
