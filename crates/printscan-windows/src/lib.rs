//! Windows printing and scanning for [printscan](https://github.com/b1tninja/printscan):
//!
//! - `spooler` (feature `print`, on by default): the print spooler as a `printscan::print` backend,
//!   on [`winprint`](https://crates.io/crates/winprint).
//! - [`wia`] (feature `scan`, on by default): WIA 2.0 scanners as a `printscan::scan` backend.
//!
//! The crate's `unsafe` code is the WIA backend's COM calls (two modules) and the spooler's page drawing (one
//! module), each block with a `// SAFETY:` comment (docs/wia-unsafe-audit.md).
//!
//! An application that manages its own colour prints RGB already converted to the printer's
//! profile through the spooler; see `docs/colour.md`. On other platforms only the WIA rule tables
//! build.

#[cfg(all(windows, feature = "print"))]
pub mod spooler;

#[cfg(feature = "scan")]
pub mod wia;
