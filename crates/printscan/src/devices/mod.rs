//! Device tools shared by applications. UI-free and platform-free: an app registers its platform
//! backends into `printscan::scan` / `printscan::print` and mounts these tools into its own CLI or tool list.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod replace;
pub mod scan_jobs;
