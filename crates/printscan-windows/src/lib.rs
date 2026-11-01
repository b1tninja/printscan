//! Windows printing for [printscan](https://github.com/b1tninja/printscan).
//!
//! - `spooler` (feature `print`, on by default): the Windows print spooler as a
//!   `printscan::print` backend, on [`winprint`](https://crates.io/crates/winprint).
//! - `gdi` (feature `gdi`, on by default): an RGB image printed through GDI with the colour
//!   management under the caller's control.
//!
//! On other platforms the crate is empty.

#[cfg(all(windows, feature = "print"))]
pub mod spooler;

#[cfg(all(windows, feature = "gdi"))]
#[allow(unsafe_code)]
pub mod gdi;
