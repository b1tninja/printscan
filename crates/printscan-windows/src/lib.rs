//! Windows printing and scanning for [printscan](https://github.com/b1tninja/printscan):
//!
//! - `spooler` (feature `print`, on by default): the print spooler as a `printscan::print` backend,
//!   on [`winprint`](https://crates.io/crates/winprint).
//! - [`wia`] (feature `scan`, on by default): WIA 2.0 scanners as a `printscan::scan` backend. Its
//!   COM calls are the crate's only `unsafe` code, in two modules (docs/wia-unsafe-audit.md).
//!
//! An application that manages its own colour prints RGB already converted to the printer's
//! profile through the spooler; see `docs/colour.md`. On other platforms only the WIA rule tables
//! build.

#[cfg(all(windows, feature = "print"))]
pub mod spooler;

#[cfg(feature = "scan")]
pub mod wia;
