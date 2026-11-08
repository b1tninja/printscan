//! macOS scanning for [printscan](https://github.com/b1tninja/printscan):
//!
//! - [`ica`] (feature `scan`, on by default): scanners reached through Apple's ImageCaptureCore
//!   (flatbed and document feeder) as a `printscan::scan` backend, on the
//!   [objc2](https://github.com/madsmtm/objc2) bindings.
//!
//! **Experimental, not yet published**: the Objective-C layer has been compiled for macOS but not
//! run against a scanner. Printing on macOS is a separate CUPS backend; this crate scans only.
//!
//! The crate's `unsafe` code is the Objective-C calls of two modules, `ica::ffi` and
//! `ica::delegate`, each block with a `// SAFETY:` comment (docs/ica-unsafe-audit.md). On other
//! platforms only the rule tables, the capability mapping and the session state machines build.

/// `with_*` setters for a `non_exhaustive` struct: each sets one field and returns the value, so a
/// caller outside the crate can build one from its `Default`. A `some` setter takes the bare value
/// of an `Option` field.
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
}

#[cfg(feature = "scan")]
pub mod ica;
