//! The IPP backend against fakeprint (<https://github.com/b1tninja/fakeprint>), a separate program,
//! so a codec bug cannot pass on both sides. Skipped unless `PRINTSCAN_FAKEPRINT_URL` names a
//! fakeprint base URL on loopback (its IPP printer is `/ipp/print` on the same port):
//!
//! ```text
//! fakeprint --listen 127.0.0.1:3921 --no-discovery --no-jetdirect --profile generic-wsd --inbox <dir>
//! PRINTSCAN_FAKEPRINT_URL=http://127.0.0.1:3921 cargo test -p printscan-ipp --test live_fakeprint
//! ```
//!
//! docs/testing.md has the whole set-up.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use printscan::print::{Document, DocumentFormat, Job, JobState, Registry, SubmitRequest, UserConfirmed, namespaced_id};
use printscan_ipp::IppBackend;

/// The tests share one fakeprint: one at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// fakeprint's `host:port` and a registry with its IPP printer, or `None` to skip.
fn fake() -> Option<(MutexGuard<'static, ()>, String, Registry, String)> {
    let base = std::env::var("PRINTSCAN_FAKEPRINT_URL").ok().filter(|u| !u.is_empty())?;
    let addr = base.trim_end_matches('/').strip_prefix("http://").expect("PRINTSCAN_FAKEPRINT_URL is http://host:port").to_owned();
    let host: std::net::SocketAddr = addr.parse().expect("a numeric host:port");
    assert!(host.ip().is_loopback(), "the live tests only talk to loopback");
    let turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    let uri = format!("ipp://{addr}/ipp/print");
    let backend = IppBackend::with_printers([uri.as_str()]).expect("the printer URI is allowed");
    let registry = Registry::new();
    registry.register(Box::new(backend)).expect("register");
    Some((turn, addr, registry, namespaced_id("ipp", &uri)))
}

/// `GET path` on fakeprint's control plane: the reply body.
fn get(addr: &str, path: &str) -> String {
    let mut s = TcpStream::connect(addr).expect("connect to fakeprint");
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(s, "GET {path} HTTP/1.0\r\nHost: {addr}\r\n\r\n").unwrap();
    let mut reply = String::new();
    s.read_to_string(&mut reply).expect("read the reply");
    reply.split_once("\r\n\r\n").map(|(_, body)| body.to_owned()).unwrap_or_default()
}

/// The `captured_jobs` count in fakeprint's status.
fn captured(addr: &str) -> u64 {
    let status = get(addr, "/v1/status");
    let at = status.find("\"captured_jobs\":").expect("captured_jobs in /v1/status") + "\"captured_jobs\":".len();
    status[at..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().expect("a count")
}

/// A one-page PDF with its cross-reference table at the right offsets.
fn one_page_pdf() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>",
        "<< /Length 31 >>\nstream\n0 0 1 rg 100 600 200 150 re f\nendstream",
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    pdf
}

#[test]
fn the_printer_describes_itself() {
    let Some((_turn, _, registry, id)) = fake() else { return };
    let details = registry.printer_details(&id).expect("Get-Printer-Attributes");
    assert!(details.formats.contains(&DocumentFormat::Pdf), "{:?}", details.formats);
    assert!(!details.resolutions.is_empty());
}

#[test]
fn a_pdf_prints_to_completed_and_reaches_the_printer() {
    let Some((_turn, addr, registry, id)) = fake() else { return };
    let before = captured(&addr);
    let pdf = one_page_pdf();
    let job = Job::default().with_printer(id).with_title("printscan live IPP test");
    let request = SubmitRequest::new(Document::pdf(&pdf), job).with_consent(UserConfirmed::attest(Some(true)).unwrap());
    let submitted = registry.submit_job(request).expect("Print-Job");
    let job = submitted.job.expect("IPP gives a job to follow");

    let started = Instant::now();
    let state = loop {
        let status = registry.job_status(&job).expect("Get-Job-Attributes");
        if status.state.is_terminal() {
            break status.state;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "not finished after 60 s: {status:?}");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(state, JobState::Completed);
    assert_eq!(captured(&addr), before + 1, "fakeprint captured the document");
}
