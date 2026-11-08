//! ImageCaptureCore scanners as a `printscan::scan` backend. [`Ica`] implements
//! [`printscan::scan::Backend`] under the name `"ica"`, with device ids `ica:<persistent id>`:
//! `scanner_list` browses (it opens no device), `scanner_details` and `scanners` open a session
//! and read every functional unit (they feed no paper), and `acquire` scans, which does.
//!
//! # The name
//!
//! The prefix names the API, as `wia` does on Windows, not the operating system: a Mac can also
//! reach a network scanner through eSCL or WS-Scan, and a SANE backend could one day run there too,
//! so `macos:` would not say which stack a device id belongs to. `printscan::scan::Backend::name`
//! already uses `"ica"` as its example.
//!
//! # Nothing on the network until the app asks
//!
//! By default the device browser looks for **local** scanners only (USB and the like). Network
//! scanners (Bonjour, shared and remote ones) are browsed only when the app turns them on with
//! [`IcaOptions::with_network`]: the browse is the system's own discovery, and an app decides when
//! that happens.
//!
//! # Threads
//!
//! ImageCaptureCore delivers every delegate callback on the main thread, so every call this crate
//! makes into it runs on the main dispatch queue as well, and the calling thread waits for the
//! answers. Called from the main thread (a command-line tool), the wait runs the main run loop;
//! called from a worker thread, the main thread must be running its run loop (an AppKit app does).
//! Every step has a deadline, so a main thread that is blocked gives an error, not a hang.
//! `docs/run-loop.md` has the whole design.
//!
//! # What builds where
//!
//! [`model`] (plain data read from a functional unit), [`caps`] (that data as printscan's
//! capabilities), [`plan`] (a `ScanSettings` as unit settings), [`errors`] (ImageCaptureCore's
//! return codes) and [`IcaOptions`] have no Objective-C in them and build on every platform; so do
//! the session state machines, which the tests drive with scripted events. [`Ica`] and the
//! Objective-C layer (`ffi`, `delegate`, `main_thread`) exist on macOS only.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
// `Ica` exists only on macOS; elsewhere the links in the module docs have nothing to point at.
#![cfg_attr(not(target_os = "macos"), allow(rustdoc::broken_intra_doc_links))]

pub mod caps;
pub mod errors;
pub mod model;
pub mod plan;

mod devices;
mod options;
mod session;

pub use options::IcaOptions;

#[cfg(target_os = "macos")]
mod backend;
#[cfg(target_os = "macos")]
mod delegate;
#[cfg(target_os = "macos")]
mod ffi;
#[cfg(target_os = "macos")]
mod main_thread;

#[cfg(target_os = "macos")]
pub use backend::Ica;

/// The backend's registry name, and the prefix of its device ids.
pub const NAME: &str = "ica";

/// Register the ImageCaptureCore backend, with default options (local scanners only), with the
/// process-wide `printscan::scan` registry. Safe to call more than once: a second registration is
/// ignored. Returns whether an `ica` backend is registered afterwards, which is `false` on
/// platforms other than macOS. An app that wants network scanners registers
/// `Ica::with_options(IcaOptions::default().with_network(true))` itself instead.
pub fn register() -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            // A duplicate (the app registered it itself) is not an error here.
            if let Err(error) = printscan::scan::register(Box::new(Ica::new())) {
                log::debug!("scan: {error}");
            }
        });
        printscan::scan::backend_names().contains(&NAME)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
