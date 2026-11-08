//! The SANE backend against the fake daemon in `common`, over loopback TCP.

#![cfg(feature = "scan")]

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use common::{COVER_OPEN, DEVICE, DEVICE_BUSY, FakeSaned, JAMMED, LINES, Scene, WIDTH};
use printscan::job::{RetryAdvice, Timestamp};
use printscan::scan::job::{ScanJob, ScanJobConfig};
use printscan::scan::{Area, Backend, CancelToken, Color, InterruptKind, ProgressToken, Registry, ScanError, ScanSettings, Source};
use printscan_sane::scan::{AUTHORIZATION_REQUIRED, SaneBackend};
use printscan_sane::{Client, ClientOptions, SaneHost, Status};

fn backend(fake: &FakeSaned) -> (SaneBackend, String) {
    let backend = SaneBackend::new();
    backend.add_host(&fake.host()).unwrap();
    let id = format!("{}/{DEVICE}", fake.host());
    (backend, id)
}

fn scan(id: &str, source: Source) -> ScanSettings {
    ScanSettings::new(id).with_source(source).with_dpi(300)
}

/// The value the client last set on `name`.
fn last_set(fake: &FakeSaned, name: &str) -> Option<String> {
    fake.log().sets.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.clone())
}

#[test]
fn the_client_speaks_the_protocol() {
    let fake = FakeSaned::start(Scene::default());
    let host = SaneHost::parse(&fake.host()).unwrap();
    let mut client = Client::connect(host, &ClientOptions::default().with_user_name("tester".into())).unwrap();
    assert_eq!(printscan_sane::protocol::version_parts(client.daemon_version()), (1, 0, 3));
    let devices = client.devices().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, DEVICE);
    assert_eq!(devices[0].display_name(), "Fake Flatbed ADF");
    let handle = client.open(DEVICE).unwrap();
    let table = client.option_descriptors(handle).unwrap();
    let (index, mode) = table.find("mode").unwrap();
    let mode = mode.clone();
    assert_eq!(client.get_option(handle, index, &mode).unwrap().as_str(), Some("Gray"));
    let reply = client.set_option(handle, index, &mode, &printscan_sane::Value::String("Color".into())).unwrap();
    assert!(reply.reload_options());
    assert_eq!(reply.value.as_str(), Some("Color"));
    // A device the daemon does not have.
    let error = client.open("nothing:0").unwrap_err();
    assert_eq!(error.status(), Some(Status::Inval));
    client.close(handle).unwrap();
    client.exit();
    assert_eq!(fake.log().users, vec!["tester".to_owned()]);
}

#[test]
fn listing_asks_the_added_daemon_and_ids_are_namespaced() {
    let fake = FakeSaned::start(Scene::default());
    let (backend, id) = backend(&fake);
    let registry = Registry::new();
    registry.register(Box::new(backend.clone())).unwrap();
    let listing = registry.scanner_list().unwrap();
    assert!(listing.is_complete());
    assert_eq!(listing.items.len(), 1);
    assert_eq!(listing.items[0].id, format!("sane:{id}"));
    assert_eq!(listing.items[0].name, "Fake Flatbed ADF");
    // A device added by name is listed without asking anyone.
    let quiet = SaneBackend::new();
    let added = quiet.add_device_named("192.168.200.1", "test:0", "Desk scanner").unwrap();
    let rows = quiet.scanner_list().unwrap();
    assert_eq!((rows[0].id.as_str(), rows[0].name.as_str()), (added.as_str(), "Desk scanner"));
}

#[test]
fn details_map_the_standard_options() {
    let fake = FakeSaned::start(Scene::default());
    let (backend, id) = backend(&fake);
    let scanner = backend.scanner_details(&id).unwrap();
    assert!(scanner.flatbed && scanner.feeder && scanner.duplex);
    assert_eq!(scanner.feeder_ready, None);
    let glass = scanner.glass.as_ref().unwrap();
    assert_eq!(glass.dpi, vec![75, 150, 300, 600]);
    assert_eq!(glass.colors, vec![Color::BlackAndWhite, Color::Gray, Color::Rgb]);
    assert_eq!(glass.brightness.map(|b| (b.min, b.max)), Some((-100, 100)));
    assert_eq!(scanner.bed, Some(Area::new(8500, 11693)));
    assert_eq!(scanner.feeder_choices.as_ref().unwrap().dpi, glass.dpi);
    // Nothing was set and nothing scanned.
    assert!(fake.log().sets.is_empty());
    assert!(!fake.log().rpcs.contains(&7));
    let dump = backend.diagnostic_dump("").unwrap();
    assert!(dump.contains("--resolution Int 75|150|300|600 dpi [settable]"), "{dump}");
    assert!(dump.contains("--source String Flatbed|ADF|ADF Duplex"), "{dump}");
}

#[test]
fn a_glass_scan_is_one_png_page() {
    let fake = FakeSaned::start(Scene::default());
    let (backend, id) = backend(&fake);
    let progress = ProgressToken::new();
    let settings = scan(&id, Source::Glass).with_color(Color::Rgb).with_region(printscan::scan::Region::new(1000, 1000, 2000, 3000));
    let acquired = backend.acquire(&settings.with_progress(progress.clone())).unwrap();
    assert_eq!(acquired.source, Source::Glass);
    assert_eq!(acquired.pages.len(), 1);
    let page = &acquired.pages[0];
    assert_eq!((page.format, page.width, page.height), ("png", WIDTH as u32, LINES as u32));
    assert_eq!(page.dpi, 300);
    assert_eq!(progress.pages_done(), 1);
    assert_eq!(last_set(&fake, "source").as_deref(), Some("Flatbed"));
    assert_eq!(last_set(&fake, "mode").as_deref(), Some("Color"));
    assert_eq!(last_set(&fake, "resolution").as_deref(), Some("[300]"));
    // One inch in: 25.4 mm as 16.16 fixed point.
    assert_eq!(last_set(&fake, "tl-x").as_deref(), Some(format!("[{}]", (25.4f64 * 65536.0).round() as i32).as_str()));
    let rpcs = fake.log().rpcs;
    // The scan ends with cancel, close and exit.
    assert_eq!(&rpcs[rpcs.len() - 3..], &[8, 3, 10]);
}

#[test]
fn a_whole_feeder_ends_when_it_is_empty() {
    let fake = FakeSaned::start(Scene { sheets: 3, ..Scene::default() });
    let (backend, id) = backend(&fake);
    let acquired = backend.acquire(&scan(&id, Source::Feeder).with_pages(0)).unwrap();
    assert_eq!(acquired.source, Source::Feeder);
    assert_eq!(acquired.pages.len(), 3);
    assert_eq!(last_set(&fake, "source").as_deref(), Some("ADF"));
}

#[test]
fn duplex_takes_both_sides_of_each_sheet() {
    let fake = FakeSaned::start(Scene { sheets: 2, ..Scene::default() });
    let (backend, id) = backend(&fake);
    let acquired = backend.acquire(&scan(&id, Source::Feeder).with_pages(0).with_duplex(true)).unwrap();
    assert_eq!(acquired.pages.len(), 4);
    assert_eq!(last_set(&fake, "source").as_deref(), Some("ADF Duplex"));
    let simplex = FakeSaned::start(Scene { sources: vec!["Flatbed", "ADF"], ..Scene::default() });
    let (backend, id) = self::backend(&simplex);
    let error = backend.acquire(&scan(&id, Source::Feeder).with_duplex(true)).unwrap_err();
    assert!(error.to_string().contains("two-sided"), "{error}");
}

#[test]
fn an_empty_feeder_is_named() {
    let fake = FakeSaned::start(Scene::default());
    let (backend, id) = backend(&fake);
    let error = backend.acquire(&scan(&id, Source::Feeder)).unwrap_err();
    assert_eq!(error.interrupt_kind(), InterruptKind::FeederEmpty);
    assert_eq!(error.native().unwrap().name, Some("SANE_STATUS_NO_DOCS"));
    assert_eq!(error.native().unwrap().code, 7);
}

#[test]
fn auto_tries_the_feeder_then_the_glass() {
    let fake = FakeSaned::start(Scene::default());
    let (backend, id) = backend(&fake);
    let acquired = backend.acquire(&scan(&id, Source::Auto).with_pages(0)).unwrap();
    assert_eq!(acquired.source, Source::Glass);
    assert_eq!(acquired.pages.len(), 1);
    let sources: Vec<String> = fake.log().sets.iter().filter(|(n, _)| n == "source").map(|(_, v)| v.clone()).collect();
    assert_eq!(sources, ["ADF", "Flatbed"]);
    // With paper loaded, Auto is the feeder.
    let loaded = FakeSaned::start(Scene { sheets: 2, ..Scene::default() });
    let (backend, id) = self::backend(&loaded);
    assert_eq!(backend.acquire(&scan(&id, Source::Auto).with_pages(0)).unwrap().source, Source::Feeder);
}

#[test]
fn a_counted_feeder_scan_that_runs_short_keeps_its_pages() {
    let fake = FakeSaned::start(Scene { sheets: 2, ..Scene::default() });
    let (backend, id) = backend(&fake);
    let ScanError::Incomplete { acquired, cause, .. } = backend.acquire(&scan(&id, Source::Feeder).with_pages(5)).unwrap_err() else {
        panic!("not incomplete")
    };
    assert_eq!(acquired.pages.len(), 2);
    assert_eq!(cause.unwrap().kind, InterruptKind::FeederEmpty);
}

#[test]
fn a_jam_after_two_pages_keeps_them() {
    let fake = FakeSaned::start(Scene { sheets: 5, fail_at: Some((2, JAMMED)), ..Scene::default() });
    let (backend, id) = backend(&fake);
    let error = backend.acquire(&scan(&id, Source::Feeder).with_pages(0)).unwrap_err();
    let ScanError::Incomplete { acquired, cause, .. } = &error else { panic!("{error:?}") };
    assert_eq!(acquired.pages.len(), 2);
    let cause = cause.unwrap();
    assert_eq!(cause.kind, InterruptKind::Jam);
    assert_eq!(cause.native.unwrap().name, Some("SANE_STATUS_JAMMED"));
    // The device is still reset afterwards.
    assert!(fake.log().rpcs.contains(&8));
}

#[test]
fn statuses_become_causes_and_retry_advice() {
    let advice = |error: ScanError| {
        let mut job = ScanJob::new(ScanSettings::new("sane:x"), ScanJobConfig::default());
        job.begin_attempt(Timestamp::from_secs(1)).unwrap();
        job.finish_attempt(Err(error), Timestamp::from_secs(2)).unwrap();
        job.retry_advice()
    };
    let busy = FakeSaned::start(Scene { open_status: DEVICE_BUSY, ..Scene::default() });
    let (backend, id) = backend(&busy);
    let error = backend.acquire(&scan(&id, Source::Glass)).unwrap_err();
    assert_eq!(error.interrupt_kind(), InterruptKind::Busy);
    assert!(matches!(advice(error), RetryAdvice::RetryNow | RetryAdvice::RetryAfter(_)));

    let cover = FakeSaned::start(Scene { fail_at: Some((0, COVER_OPEN)), ..Scene::default() });
    let (backend, id) = self::backend(&cover);
    let error = backend.acquire(&scan(&id, Source::Glass)).unwrap_err();
    assert_eq!(error.interrupt_kind(), InterruptKind::CoverOpen);
    assert_eq!(advice(error), RetryAdvice::DoNotRetry);

    // The data ends with a jam instead of the end of the frame.
    let jam = FakeSaned::start(Scene { end_status: JAMMED, ..Scene::default() });
    let (backend, id) = self::backend(&jam);
    let error = backend.acquire(&scan(&id, Source::Glass)).unwrap_err();
    assert_eq!(error.interrupt_kind(), InterruptKind::Jam);

    // Nothing listens: offline, and the outcome is known (nothing was sent).
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let nobody = SaneBackend::new();
    nobody.add_host(&format!("127.0.0.1:{port}")).unwrap();
    let error = nobody.acquire(&ScanSettings::new(format!("127.0.0.1:{port}/{DEVICE}"))).unwrap_err();
    assert_eq!(error.interrupt_kind(), InterruptKind::Offline);
}

#[test]
fn a_request_for_credentials_is_reported_and_nothing_is_sent() {
    let fake = FakeSaned::start(Scene { auth_resource: Some("fake$MD5$salt123".into()), ..Scene::default() });
    let (backend, id) = backend(&fake);
    let error = backend.acquire(&scan(&id, Source::Glass)).unwrap_err();
    assert_eq!(error.native(), Some(AUTHORIZATION_REQUIRED));
    let text = error.to_string();
    assert!(text.contains("\"fake\""), "{text}");
    assert!(!text.contains("salt123"), "{text}");
    // No authorize, no password, nothing after the open.
    let rpcs = fake.log().rpcs;
    assert!(!rpcs.contains(&9));
    assert_eq!(rpcs.last(), Some(&2));
}

#[test]
fn cancel_stops_a_transfer_in_progress() {
    let fake = FakeSaned::start(Scene { slow: true, ..Scene::default() });
    let (backend, id) = backend(&fake);
    let cancel = CancelToken::new();
    let progress = ProgressToken::new();
    let settings = scan(&id, Source::Glass).with_cancel(cancel.clone()).with_progress(progress.clone());
    let worker = thread::spawn(move || backend.acquire(&settings));
    for _ in 0..500 {
        if progress.bytes_received() > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(progress.bytes_received() > 0, "no data arrived");
    cancel.request();
    let error = worker.join().unwrap().unwrap_err();
    assert!(error.to_string().contains("cancelled"), "{error}");
    // The scan was cancelled on the daemon as well.
    assert!(fake.log().rpcs.contains(&8));
}

#[test]
fn three_pass_colour_is_one_page() {
    let fake = FakeSaned::start(Scene { three_pass: true, ..Scene::default() });
    let (backend, id) = backend(&fake);
    let acquired = backend.acquire(&scan(&id, Source::Glass).with_color(Color::Rgb)).unwrap();
    assert_eq!(acquired.pages.len(), 1);
    let mut reader = png::Decoder::new(std::io::Cursor::new(&acquired.pages[0].bytes)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgb);
    // Pixel 0: red, green and blue frames carry their own pattern (x ^ channel * 40).
    assert_eq!(&buf[..3], &[0, 40, 80]);
    assert_eq!(fake.log().rpcs.iter().filter(|&&r| r == 7).count(), 3);
}

#[test]
fn black_and_white_is_lineart_from_the_device() {
    let fake = FakeSaned::start(Scene::default());
    let (backend, id) = backend(&fake);
    let acquired = backend.acquire(&scan(&id, Source::Glass).with_color(Color::BlackAndWhite)).unwrap();
    let page = &acquired.pages[0];
    assert_eq!(page.format, "png");
    assert!(!page.thresholded);
    assert_eq!(last_set(&fake, "mode").as_deref(), Some("Lineart"));
    // Without a 1-bit mode the device scans grey and the page is thresholded.
    let grey_only = FakeSaned::start(Scene { modes: vec!["Gray", "Color"], ..Scene::default() });
    let (backend, id) = self::backend(&grey_only);
    let page = backend.acquire(&scan(&id, Source::Glass).with_color(Color::BlackAndWhite)).unwrap().pages.remove(0);
    assert!(page.thresholded);
    assert_eq!(page.format, "bmp");
}

#[test]
fn a_resolution_the_device_lacks_is_refused_before_scanning() {
    let fake = FakeSaned::start(Scene::default());
    let (backend, id) = backend(&fake);
    let error = backend.acquire(&scan(&id, Source::Glass).with_dpi(200)).unwrap_err();
    assert!(error.to_string().contains("75, 150, 300, 600"), "{error}");
    assert!(!fake.log().rpcs.contains(&7));
}

#[test]
fn only_added_daemons_are_contacted() {
    let fake = FakeSaned::start(Scene::default());
    let backend = SaneBackend::new();
    let error = backend.acquire(&ScanSettings::new(format!("{}/{DEVICE}", fake.host()))).unwrap_err();
    assert!(error.to_string().contains("has not been added"), "{error}");
    assert_eq!(fake.log().connections, 0);
}

#[test]
fn a_hostile_daemon_is_an_error_not_a_panic() {
    // Answers init with a status, then a version, then claims a 2 GiB device list.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming().take(2) {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0; 64];
            let _ = stream.read(&mut request);
            let mut reply = Vec::new();
            reply.extend_from_slice(&0i32.to_be_bytes());
            reply.extend_from_slice(&0x0100_0003i32.to_be_bytes());
            let _ = stream.write_all(&reply);
            let _ = stream.read(&mut request);
            let mut reply = 0i32.to_be_bytes().to_vec();
            reply.extend_from_slice(&i32::MAX.to_be_bytes());
            let _ = stream.write_all(&reply);
        }
    });
    let backend = SaneBackend::new();
    backend.add_host(&addr.to_string()).unwrap();
    let error = backend.scanner_list().unwrap_err();
    assert!(error.to_string().contains("larger than the limit"), "{error}");
}
