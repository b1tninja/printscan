//! Printing and scanning for Rust applications.
//!
//! - [`job`]: the job and device vocabulary shared by print and scan: states, ids, retry advice.
//! - [`scan`] (feature `scan`, on by default): the scanner contract, the backend registry and `ScanJob`.
//! - [`print`](mod@print) (feature `print`, on by default): the print backend registry, tracked jobs, conversion and job watching.
//! - [`devices`] (feature `devices`): the scan job journal for large piles and manual duplex.
//!
//! Network and platform backends live in their own crates, such as `printscan-wsd` and `printscan-ipp`.

/// `with_*` setters for a `non_exhaustive` struct: each sets one field and returns the value, so a
/// caller outside the crate can build one from its `Default`. A `some` setter takes the bare value
/// of an `Option` field.
// Unused when the features that need it are off.
#[allow(unused_macros)]
macro_rules! setters {
    ($ty:ty { $($kind:ident $method:ident => $field:ident: $t:ty),* $(,)? }) => {
        impl $ty {
            $(setters!(@one $kind $method $field $t);)*
        }
    };
    (@one set $method:ident $field:ident $t:ty) => {
        #[doc = concat!("Sets [`Self::", stringify!($field), "`].")]
        #[must_use]
        pub fn $method(mut self, value: $t) -> Self {
            self.$field = value;
            self
        }
    };
    (@one some $method:ident $field:ident $t:ty) => {
        #[doc = concat!("Sets [`Self::", stringify!($field), "`] to `Some(value)`.")]
        #[must_use]
        pub fn $method(mut self, value: $t) -> Self {
            self.$field = Some(value);
            self
        }
    };
    (@one into $method:ident $field:ident $t:ty) => {
        #[doc = concat!("Sets [`Self::", stringify!($field), "`].")]
        #[must_use]
        pub fn $method(mut self, value: impl Into<$t>) -> Self {
            self.$field = value.into();
            self
        }
    };
    (@one some_into $method:ident $field:ident $t:ty) => {
        #[doc = concat!("Sets [`Self::", stringify!($field), "`] to `Some(value)`.")]
        #[must_use]
        pub fn $method(mut self, value: impl Into<$t>) -> Self {
            self.$field = Some(value.into());
            self
        }
    };
}

pub mod job;

#[cfg(feature = "scan")]
pub mod scan;

#[cfg(feature = "print")]
pub mod print;

#[cfg(feature = "devices")]
pub mod devices;
