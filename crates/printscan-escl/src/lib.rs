//! eSCL ("AirScan", the Mopria and Apple network scan protocol) as a [`printscan::scan::Backend`].
//! Not compiled for `wasm32`.
//!
//! [`EsclBackend`] scans from network scanners that serve eSCL: XML documents and images over HTTP,
//! under a root such as `http://192.168.1.20/eSCL`. It registers as `escl`, names its scanners by
//! that root URL (registry ids `escl:http://…/eSCL`) and reports them as
//! [`printscan::scan::Connection::Other`].
//!
//! # Nothing happens until the app asks
//!
//! * The backend **lists only the scanners the app added** ([`EsclBackend::add`]). It does no
//!   discovery (no mDNS, no multicast); finding scanners is the app's separate decision.
//! * Building a backend, adding a scanner and listing scanners open no connection. Details are a
//!   `GET ScannerCapabilities` and a `GET ScannerStatus`, status is the second alone, and a scan is
//!   the job below.
//! * Root URLs are `http://` or `https://` with a numeric loopback, link-local or private address
//!   ([`ScannerUrl`]). Host names, public addresses, proxies and redirects are refused. `https` needs
//!   the cargo feature `tls`, and then a `TlsMode`: a scanner's certificate is self-signed, so the
//!   app either supplies the certificates it trusts or opts in to an unverified connection.
//!
//! # A scan
//!
//! `GET ScannerCapabilities` and `GET ScannerStatus` (for the plan and a preflight), `POST ScanJobs`
//! with a [`ScanRequest`] planned from the request, then `GET <job>/NextDocument` until the scanner
//! answers 404 (no more documents) or every page asked for arrived. A job stopped early (cancelled,
//! failed, or a fixed-count feeder job whose count is reached) is ended with `DELETE <job>`.
//!
//! * **Source.** `Source::Auto` is the feeder only when the status says paper is loaded
//!   (`ScannerAdfLoaded`), otherwise the glass when there is one.
//! * **Formats.** A request that names a format gets that format or an error. One that leaves it
//!   open gets the first of PNG, TIFF, JPEG and PDF the source lists. Black and white is asked for
//!   as `BlackAndWhite1` when listed, otherwise as `Grayscale8` and thresholded in software.
//! * **Busy.** A `NextDocument` answered 503 is asked again after the scanner's `Retry-After` (or
//!   [`EsclBackend::with_poll_interval`]) until [`EsclBackend::with_busy_wait`] runs out.
//! * **Errors** become [`printscan::scan::ScanError::Native`] with an [`InterruptKind`] and a
//!   [`printscan::scan::NativeCode`]: HTTP statuses in [`classify::HTTP_DOMAIN`], feeder states in
//!   [`classify::ADF_DOMAIN`]. A refused job or a job that ended early is explained from a fresh
//!   `ScannerStatus` read (an empty feeder, a jam, an open door, a job cancelled at the panel).
//!
//! # Layers
//!
//! * [`EsclBackend`]: the registry-facing backend (`backend.rs`).
//! * [`client::Client`]: one method per eSCL resource over any [`Transport`]; [`settings`] plans
//!   the `ScanSettings` document; [`model`] reads `ScannerCapabilities` and `ScannerStatus`.
//! * [`url`]: the address policy; [`transport`] and [`http`]: the HTTP seam and its `ureq`
//!   implementation; [`error`]: the transport-level [`Error`].
//!
//! From `printscan-wsd` this crate uses the address check ([`printscan_wsd::policy::address_allowed`]),
//! the bounded XML checks, [`printscan_wsd::NetworkLimits`] and `Retry-After` parsing. Its URL type
//! and transport are its own: WSD's are plain-`http` POST only, and eSCL needs GET, DELETE, the
//! `Location` header and `https`.
//!
//! [`InterruptKind`]: printscan::scan::InterruptKind

#![cfg(not(target_arch = "wasm32"))]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod classify;
pub mod client;
pub mod error;
pub mod http;
pub mod model;
pub mod settings;
pub mod transport;
pub mod url;

mod backend;
mod xml;

pub use backend::EsclBackend;
pub use client::{Client, Job, NextDocument};
pub use error::{Error, Result, TransportKind};
pub use http::HttpTransport;
#[cfg(feature = "tls")]
pub use http::TlsMode;
pub use model::{AdfState, Capabilities, ColorMode, InputCaps, JobInfo, JobState, Range, ScannerState, Status};
pub use printscan_wsd::NetworkLimits;
pub use settings::{InputSource, ScanRequest};
pub use transport::{Method, Request, Response, Transport};
pub use url::ScannerUrl;
