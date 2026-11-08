//! SANE scanners through the SANE network protocol, the one `saned` speaks on TCP port 6566.
//!
//! SANE is how Linux and the BSDs drive scanners. A program can link `libsane` and load the
//! drivers in-process, or talk to `saned`, the daemon that serves the same drivers over the
//! network (and that the `net` driver uses for remote scanners). This crate is a client of the
//! second: plain Rust, no `unsafe`, no `libsane` at build or run time, so it builds and tests on
//! every platform and scans from any machine that can reach a `saned`, the local one included
//! (`127.0.0.1`).
//!
//! * [`client::Client`]: one control connection and one method per remote procedure (init, get
//!   devices, open, close, option descriptors, control option, get parameters, start, cancel,
//!   exit), and [`client::DataStream`], the image data a started frame sends on its own port.
//! * [`protocol`]: the values on the wire: [`protocol::Status`], [`protocol::OptionDescriptor`],
//!   [`protocol::Parameters`], [`protocol::Device`] and the rest.
//! * [`wire`]: the encoding (big-endian words, counted strings and arrays, pointers), strict and
//!   bounded by [`wire::Limits`].
//! * [`policy`]: [`policy::SaneHost`], a numeric local-network address; nothing else is dialled.
//! * [`error`]: [`Error`], with what a caller needs to decide whether to retry.
//! * [`scan`] (feature `scan`, default): [`scan::SaneBackend`], a [`printscan::scan::Backend`]
//!   registered as `sane`.
//!
//! # Nothing happens until the app asks
//!
//! The crate discovers nothing and sends no multicast. A backend lists only the daemons and the
//! devices the app added; building one and adding to it open no connection. Every address is a
//! numeric loopback, link-local or private address ([`policy`]), and the data connection of a scan
//! goes to the address of the control connection, never to one the daemon names.
//!
//! # Authorization
//!
//! A daemon can protect a device with a user name and password (`saned`'s users file). This crate
//! sends neither: a reply that asks for them is [`Error::AuthorizationRequired`] with the resource
//! it named, and the connection is dropped. The backend reports it with the native code
//! [`scan::AUTHORIZATION_REQUIRED`], so an app can tell the person to allow the device for
//! unauthenticated use on the daemon.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// `with_*` setters for a `non_exhaustive` struct: each sets one field and returns the value, so a
/// caller outside the crate can build one from its `Default`.
macro_rules! setters {
    ($ty:ty { $($method:ident => $field:ident: $t:ty),* $(,)? }) => {
        impl $ty {
            $(
                #[doc = concat!("Sets [`Self::", stringify!($field), "`].")]
                #[must_use]
                pub fn $method(mut self, value: $t) -> Self {
                    self.$field = value;
                    self
                }
            )*
        }
    };
}

pub mod client;
pub mod error;
pub mod policy;
pub mod protocol;
pub mod wire;

#[cfg(feature = "scan")]
pub mod scan;

pub use client::{Client, ClientOptions, ControlReply, DataStream, Handle, Started, Timeouts};
pub use error::{Error, Result, TransportKind};
pub use policy::{DEFAULT_PORT, SaneHost};
pub use protocol::{ByteOrder, Constraint, Device, Frame, OptionDescriptor, OptionTable, Parameters, Status, Unit, Value, ValueType};
pub use wire::Limits;
