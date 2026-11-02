//! The WS-Print backend against fakeprint (<https://github.com/b1tninja/fakeprint>), a separate
//! program. Skipped unless `PRINTSCAN_FAKEPRINT_URL` names fakeprint's base URL on loopback; the print
//! service is `/WebServices/PrinterService` on it, and the URL goes through the same policy as any
//! printer URL.
//!
//! Run: `PRINTSCAN_FAKEPRINT_URL=http://127.0.0.1:PORT cargo test -p printscan-wsd --test wsd_live`
//! (docs/testing.md).
#![cfg(all(feature = "print", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use printscan::print::backends::Backend;
use printscan::print::{
    Document, DocumentFormat, Job, JobState, Registry, SubmitRequest, UserConfirmed, WaitPolicy, WatchOutcome, namespaced_id, wait_for_job,
};
use printscan_wsd::print::{PrinterUrl, WsdBackend};

/// The tests share one fakeprint: one at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// fakeprint's `host:port`, and a registry with its WS-Print printer, or `None` to skip.
fn live() -> Option<(MutexGuard<'static, ()>, String, String, Registry)> {
    let base = std::env::var("PRINTSCAN_FAKEPRINT_URL").ok().filter(|u| !u.is_empty())?;
    let addr = base.trim_end_matches('/').strip_prefix("http://").expect("PRINTSCAN_FAKEPRINT_URL is http://host:port").to_owned();
    let url = format!("http://{addr}/WebServices/PrinterService");
    let parsed = PrinterUrl::parse(&url).expect("PRINTSCAN_FAKEPRINT_URL must be a numeric loopback http URL");
    assert!(parsed.host().is_loopback(), "the live test only talks to loopback");
    let turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    let backend = WsdBackend::new().with_poll_interval(Duration::from_millis(200));
    let native = backend.add(&url).expect("the URL is allowed");
    let registry = Registry::new();
    registry.register(Box::new(backend)).expect("register");
    Some((turn, addr, namespaced_id("wsd", &native), registry))
}

/// The `captured_jobs` count in fakeprint's `/v1/status`.
fn captured(addr: &str) -> u64 {
    let mut s = TcpStream::connect(addr).expect("connect to fakeprint");
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(s, "GET /v1/status HTTP/1.0\r\nHost: {addr}\r\n\r\n").unwrap();
    let mut reply = String::new();
    s.read_to_string(&mut reply).expect("read the reply");
    let at = reply.find("\"captured_jobs\":").expect("captured_jobs in /v1/status") + "\"captured_jobs\":".len();
    reply[at..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().expect("a count")
}

#[test]
fn the_printer_describes_itself() {
    let Some((_turn, _, id, registry)) = live() else { return };
    let listing = registry.list_printers().expect("listing");
    assert_eq!(listing.items.len(), 1);
    let details = registry.printer_details(&id).expect("GetPrinterElements");
    assert!(!details.formats.is_empty(), "a conforming printer lists its document formats");
}

#[test]
fn a_document_prints_to_completed_and_reaches_the_printer() {
    let Some((_turn, addr, id, registry)) = live() else { return };
    let details = registry.printer_details(&id).expect("GetPrinterElements");
    let format =
        if details.formats.contains(&DocumentFormat::Pdf) { DocumentFormat::Pdf } else { *details.formats.first().expect("a listed format") };
    let before = captured(&addr);
    let bytes: &[u8] = b"%PDF-1.4\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n";
    let job = Job { printer: Some(id), title: "printscan live WS-Print test".into(), ..Job::default() };
    let request = SubmitRequest::new(Document::new(format, bytes), job).with_consent(UserConfirmed::attest(Some(true)).unwrap());
    let submitted = registry.submit_job(request).expect("CreatePrintJob and SendDocument");
    assert_eq!(submitted.backend, "wsd");
    // Accepted is not printed: follow it for a bounded time.
    let job = submitted.job.expect("a job handle");
    let outcome = wait_for_job(&registry, &job, WaitPolicy::default().with_max_wait(Duration::from_secs(30))).expect("routable");
    assert!(matches!(&outcome, WatchOutcome::Terminal(s) if s.state == JobState::Completed), "{outcome:?}");
    assert_eq!(captured(&addr), before + 1, "fakeprint captured the document");
}

#[test]
fn the_backend_is_a_network_backend() {
    assert_eq!(WsdBackend::new().class(), printscan::print::BackendClass::Network);
}
