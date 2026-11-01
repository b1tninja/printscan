//! Printing and scanning for Rust applications.
//!
//! - [`job`]: the job and device vocabulary shared by print and scan: states, ids, retry advice.
//! - [`scan`] (feature `scan`, on by default): the scanner contract, the backend registry and `ScanJob`.
//! - [`print`](mod@print) (feature `print`, on by default): the print backend registry, tracked jobs, conversion and job watching.
//! - [`devices`] (feature `devices`): the scan job journal for large piles and manual duplex.
//!
//! Network and platform backends live in their own crates, such as `printscan-wsd` and `printscan-ipp`.

pub mod job;

#[cfg(feature = "scan")]
pub mod scan;

#[cfg(feature = "print")]
pub mod print;

#[cfg(feature = "devices")]
pub mod devices;
