//! Windows printing for [printscan](https://github.com/b1tninja/printscan).
//!
//! - [`spooler`] (feature `print`, on by default): the Windows print spooler as a
//!   `printscan::print` backend, on [`winprint`](https://crates.io/crates/winprint).
//! - [`gdi`] (feature `gdi`, on by default): driver settings, printable geometry and an RGB/GDI
//!   transport with the colour management under the caller's control.
//!
//! On other platforms the crate is empty, except `gdi`, whose calls report `Unsupported`.

#[cfg(all(windows, feature = "print"))]
pub mod spooler;

#[cfg(feature = "gdi")]
pub mod gdi;
