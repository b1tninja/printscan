//! The WS-Scan backend against fakescan (<https://github.com/b1tninja/fakescan>), a separate program,
//! so a parser bug cannot pass on both sides. Skipped unless `PRINTSCAN_FAKESCAN_URL` names a fakescan
//! base URL on loopback, as its `--headless` ready line gives it (`base_url`):
//!
//! ```text
//! fakescan --headless --exit-on-stdin-close --profiles profiles --profile wia-lossless
//! PRINTSCAN_FAKESCAN_URL=http://127.0.0.1:PORT cargo test -p printscan-wsd --test live_fakescan
//! ```
//!
//! Each test resets fakescan and sets its scene through the control plane (paper, a fault plan), then
//! scans through the backend at `<base>/ScannerService`. docs/testing.md has the whole set-up.
#![cfg(all(feature = "scan", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use printscan::scan::{Backend, InterruptKind, ScanError, ScanSettings, Source};
use printscan_wsd::scan::WsdScanBackend;

/// The tests share one fakescan: one at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

struct Fake {
    _turn: MutexGuard<'static, ()>,
    /// `host:port`.
    addr: String,
    backend: WsdScanBackend,
}

/// fakescan, reset, or `None` to skip.
fn fake() -> Option<Fake> {
    let base = std::env::var("PRINTSCAN_FAKESCAN_URL").ok().filter(|u| !u.is_empty())?;
    let addr = base.trim_end_matches('/').strip_prefix("http://").expect("PRINTSCAN_FAKESCAN_URL is http://host:port").to_owned();
    let host: std::net::SocketAddr = addr.parse().expect("a numeric host:port");
    assert!(host.ip().is_loopback(), "the live tests only talk to loopback");
    let turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    let fake = Fake { _turn: turn, backend: WsdScanBackend::new(), addr };
    fake.control("/v1/reset", "{}");
    fake.backend.add(&format!("http://{}/ScannerService", fake.addr)).expect("the scanner URL is allowed");
    Some(fake)
}

impl Fake {
    /// `POST path` with a JSON body on the control plane; panics unless it answers 2xx.
    fn control(&self, path: &str, json: &str) {
        let mut s = TcpStream::connect(&self.addr).expect("connect to fakescan");
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(s, "POST {path} HTTP/1.0\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{json}", self.addr, json.len())
            .unwrap();
        let mut reply = String::new();
        s.read_to_string(&mut reply).expect("read the reply");
        let status = reply.split(' ').nth(1).unwrap_or("");
        assert!(status.starts_with('2'), "POST {path} {json}: {}", reply.lines().next().unwrap_or(""));
    }

    fn paper(&self, sheets: u32) {
        self.control("/v1/pages", &format!("{{\"count\":{sheets}}}"));
    }

    fn plan(&self, entry: &str) {
        self.control("/v1/faults/plan", &format!("{{\"faults\":[{entry}]}}"));
    }

    fn feeder(&self, pages: u32) -> ScanSettings {
        ScanSettings { source: Source::Feeder, pages, ..ScanSettings::default() }
    }
}

#[test]
fn a_glass_scan_is_one_page_with_the_density_it_carries() {
    let Some(fake) = fake() else { return };
    let acquired = fake.backend.acquire(&ScanSettings { source: Source::Glass, dpi: 100, ..ScanSettings::default() }).expect("glass scan");
    assert_eq!(acquired.pages.len(), 1);
    let page = &acquired.pages[0];
    assert_eq!(page.format, "png", "the lossless format first");
    assert_eq!(page.dpi, 100);
    assert!(page.width > 0 && page.height > page.width, "{}x{}", page.width, page.height);
}

#[test]
fn an_open_ended_feeder_scan_takes_every_sheet() {
    let Some(fake) = fake() else { return };
    fake.paper(3);
    let acquired = fake.backend.acquire(&ScanSettings { dpi: 100, ..fake.feeder(0) }).expect("feeder scan");
    assert_eq!(acquired.pages.len(), 3);
}

#[test]
fn an_empty_feeder_is_named_before_a_job_is_made() {
    let Some(fake) = fake() else { return };
    let err = fake.backend.acquire(&fake.feeder(1)).unwrap_err();
    assert_eq!(err.interrupt_kind(), InterruptKind::FeederEmpty, "{err:?}");
}

#[test]
fn a_jam_after_two_sheets_keeps_them_and_names_the_jam() {
    let Some(fake) = fake() else { return };
    fake.paper(4);
    fake.plan(r#"{"kind":"jam","when":{"after_pages":2}}"#);
    let err = fake.backend.acquire(&ScanSettings { dpi: 100, ..fake.feeder(4) }).unwrap_err();
    let ScanError::Incomplete { acquired, cause, .. } = err else { panic!("{err:?}") };
    assert_eq!(acquired.pages.len(), 2);
    assert_eq!(cause.map(|c| c.kind), Some(InterruptKind::Jam));
}

#[test]
fn a_reply_cut_off_mid_page_is_a_failure_not_a_cancel() {
    let Some(fake) = fake() else { return };
    fake.paper(3);
    fake.plan(r#"{"kind":"abort","when":{"page":2,"fraction":0.5}}"#);
    let err = fake.backend.acquire(&ScanSettings { dpi: 100, ..fake.feeder(3) }).unwrap_err();
    let ScanError::Incomplete { acquired, cause, .. } = err else { panic!("{err:?}") };
    assert_eq!(acquired.pages.len(), 1, "page 2 was not delivered");
    assert_ne!(cause.map(|c| c.kind), Some(InterruptKind::Cancelled), "nobody pressed Cancel");
}
