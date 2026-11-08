//! CUPS print queues as a [`printscan::print::backends::Backend`]. Not compiled for `wasm32`.
//!
//! **Experimental, not yet published**: the crate is `publish = false` and left out of releases
//! until it has run against a real scheduler.
//!
//! [`CupsBackend`] prints to the queues of a CUPS scheduler (Linux, macOS and the other systems that
//! print through CUPS). It speaks IPP to the scheduler, which listens on `localhost:631`, instead of
//! linking libcups: the crate has no C dependency, builds and tests on every OS, and reuses
//! `printscan-ipp`'s codec, client, job planning and error classification. It registers as `cups`,
//! has [`printscan::print::BackendClass::Network`] (so the registry only hands it requests that
//! carry a [`printscan::print::UserConfirmed`]), names its printers by queue name (`cups:<queue>`)
//! and its jobs `cups:<queue>#<job id>` (see [`CupsBackend::job_id`]).
//!
//! # What it sends, and when
//!
//! * Building a backend opens no connection. The scheduler is `ipp://127.0.0.1:631/` unless the app
//!   names another with [`CupsBackend::with_server`] (a numeric loopback, link-local or private
//!   address, under `printscan-ipp`'s printer URI policy). Nothing is discovered and no multicast
//!   is sent: the queues are the scheduler's, and the scheduler does any discovery it is set up for.
//! * Listing printers is one `CUPS-Get-Printers` and one `CUPS-Get-Default`
//!   ([`CupsBackend::queues`], [`CupsBackend::default_queue`]).
//! * A queue's details are one `Get-Printer-Attributes` on `ipp://<server>/printers/<queue>` (or
//!   `/classes/<queue>`), read as `printscan-ipp` reads a printer, plus one `CUPS-Get-Default`.
//! * A job is `printscan-ipp`'s submit on the queue's URI; following, listing, finding and
//!   cancelling it are `Get-Job-Attributes`, `Get-Jobs` and `Cancel-Job` there.
//!
//! Queue names are checked before they are put in a URI and percent-encoded there ([`queue`]).
//!
//! # Not here yet
//!
//! * The scheduler's Unix domain socket (`/run/cups/cups.sock`, or `/private/var/run/cupsd` on
//!   macOS), which would let CUPS identify the local user by peer credentials. Over TCP the job owner
//!   is the `requesting-user-name` ([`CupsBackend::with_user_name`]), and CUPS may ask for
//!   authentication for operations its policy restricts, such as cancelling (HTTP 401, reported as
//!   an error).
//! * TLS (`ipps://`), authentication, PPD options, and CUPS's own administrative operations.
//!
//! # Tests
//!
//! The unit tests use static values. `tests/fake_cups.rs` runs the backend through the registry
//! against a small fake scheduler on a loopback port, whose IPP encoding is written separately
//! from `printscan-ipp`'s codec. No test talks to a real scheduler.

#![cfg(not(target_arch = "wasm32"))]

pub mod queue;
pub mod scheduler;

mod backend;

pub use backend::{CupsBackend, CupsJob, DEFAULT_SERVER};
pub use scheduler::Queue;

#[cfg(test)]
mod tests {
    use super::*;
    use printscan::print::ErrorKind;
    use printscan::print::backends::Backend;

    #[test]
    fn a_new_backend_names_the_local_scheduler_and_opens_nothing() {
        let backend = CupsBackend::new();
        assert_eq!(backend.server(), DEFAULT_SERVER);
        assert_eq!(backend.name(), "cups");
        assert_eq!(backend.class(), printscan::print::BackendClass::Network);
        assert_eq!(backend.queue_uri("Office").unwrap(), "ipp://127.0.0.1:631/printers/Office");
        assert_eq!(backend.queue_uri("Büro 2").unwrap_err().kind(), ErrorKind::InvalidRequest);
        assert_eq!(backend.queue_uri("Büro").unwrap(), "ipp://127.0.0.1:631/printers/B%C3%BCro");
    }

    #[test]
    fn the_server_must_be_a_scheduler_root_under_the_address_policy() {
        let backend = CupsBackend::new().with_server("ipp://[::1]:8631").unwrap();
        assert_eq!(backend.server(), "ipp://[::1]:8631/");
        assert_eq!(backend.queue_uri("q").unwrap(), "ipp://[::1]:8631/printers/q");
        assert_eq!(CupsBackend::new().with_server("http://10.0.0.2/").unwrap().server(), "ipp://10.0.0.2:631/");
        for no in ["ipp://127.0.0.1:631/printers/q", "ipp://localhost:631/", "ipp://8.8.8.8/", "ipps://127.0.0.1/", "127.0.0.1:631"] {
            assert_eq!(CupsBackend::new().with_server(no).unwrap_err().kind(), ErrorKind::InvalidRequest, "{no}");
        }
    }

    #[test]
    fn job_ids_round_trip_and_refuse_what_is_not_one() {
        let id = CupsBackend::job_id("Office", 42).unwrap();
        assert_eq!(id.to_string(), "cups:Office#42");
        let job = CupsBackend::parse_job_id(&id).unwrap();
        assert_eq!((job.queue.as_str(), job.job_number), ("Office", 42));
        assert_eq!(CupsBackend::parse_native("a#b#7").unwrap_err().kind(), ErrorKind::InvalidRequest);
        for no in ["Office", "Office#", "Office#0", "Office#x", "#3", "a/b#3", "Office#2147483648"] {
            assert_eq!(CupsBackend::parse_native(no).unwrap_err().kind(), ErrorKind::InvalidRequest, "{no}");
        }
        assert!(CupsBackend::job_id("Office", 0).is_err());
        assert!(CupsBackend::job_id("a b", 1).is_err());
        let other = printscan::print::JobId::new("ipp", "Office#1");
        assert_eq!(CupsBackend::parse_job_id(&other).unwrap_err().kind(), ErrorKind::InvalidRequest);
    }
}
