//! Windows printing for [printscan](https://github.com/b1tninja/printscan): the print spooler as a
//! `printscan::print` backend (`spooler`, feature `print`, on by default), on
//! [`winprint`](https://crates.io/crates/winprint).
//!
//! An application that manages its own colour prints RGB already converted to the printer's
//! profile through the spooler; see `docs/colour.md`. On other platforms the crate is empty.

#[cfg(all(windows, feature = "print"))]
pub mod spooler;
