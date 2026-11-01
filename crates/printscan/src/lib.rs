//! Printing and scanning for Rust applications.
//!
//! - [`job`]: the job and device vocabulary shared by print and scan: states, ids, retry advice.
//! - [`scan`] (feature `scan`, on by default): the scanner contract, the backend registry and `ScanJob`.
//! - [`devices`] (feature `devices`): the scan job journal for large piles and manual duplex.
//!
//! Network and platform backends live in their own crates, such as `printscan-wsd`.

pub mod job;

#[cfg(feature = "scan")]
pub mod scan;

#[cfg(feature = "devices")]
pub mod devices;
