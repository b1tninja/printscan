//! The eSCL backend against a small scripted scanner served over loopback HTTP by this file: the
//! real `ureq` transport, a real socket, nothing but 127.0.0.1.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use printscan::scan::{Backend, CancelToken, Color, InterruptKind, Registry, ScanError, ScanSettings, Source};
use printscan_escl::EsclBackend;

const NS: &str = r#"xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03" xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm""#;

fn input_caps(tag: &str, height: u32) -> String {
    format!(
        "<scan:{tag}><scan:MinWidth>16</scan:MinWidth><scan:MaxWidth>2550</scan:MaxWidth><scan:MinHeight>16</scan:MinHeight><scan:MaxHeight>{height}</scan:MaxHeight>\
         <scan:SettingProfiles><scan:SettingProfile>\
         <scan:ColorModes><scan:ColorMode>RGB24</scan:ColorMode><scan:ColorMode>Grayscale8</scan:ColorMode></scan:ColorModes>\
         <scan:DocumentFormats><pwg:DocumentFormat>image/jpeg</pwg:DocumentFormat><pwg:DocumentFormat>image/png</pwg:DocumentFormat></scan:DocumentFormats>\
         <scan:SupportedResolutions><scan:DiscreteResolutions>\
         <scan:DiscreteResolution><scan:XResolution>150</scan:XResolution><scan:YResolution>150</scan:YResolution></scan:DiscreteResolution>\
         <scan:DiscreteResolution><scan:XResolution>300</scan:XResolution><scan:YResolution>300</scan:YResolution></scan:DiscreteResolution>\
         </scan:DiscreteResolutions></scan:SupportedResolutions></scan:SettingProfile></scan:SettingProfiles>\
         <scan:MaxOpticalXResolution>600</scan:MaxOpticalXResolution></scan:{tag}>"
    )
}

fn capabilities() -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><scan:ScannerCapabilities {NS}><pwg:Version>2.63</pwg:Version><pwg:MakeAndModel>Fake eSCL Scanner</pwg:MakeAndModel>\
         <scan:Platen>{}</scan:Platen><scan:Adf>{}{}<scan:FeederCapacity>35</scan:FeederCapacity></scan:Adf>\
         <scan:BrightnessSupport><scan:Min>0</scan:Min><scan:Max>100</scan:Max><scan:Step>1</scan:Step></scan:BrightnessSupport></scan:ScannerCapabilities>",
        input_caps("PlatenInputCaps", 3507),
        input_caps("AdfSimplexInputCaps", 4200),
        input_caps("AdfDuplexInputCaps", 4200),
    )
}

/// A 4 x 4 PNG; `shade` tells pages apart.
fn png(shade: u8) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(4, 4, image::Rgb([shade, shade, shade]));
    let mut out = Vec::new();
    image.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

/// What the scripted scanner does.
struct Script {
    capabilities: String,
    /// `scan:AdfState` in the status before any job.
    adf: Option<&'static str>,
    /// Sheets in the feeder.
    sheets: usize,
    /// 503s before each document (`usize::MAX`: never ready).
    busy: usize,
    /// `POST ScanJobs` answers this status, and the feeder state then reads `adf_after_refusal`.
    refuse: Option<u16>,
    adf_after_refusal: Option<&'static str>,
    /// After this many documents the feeder jams: 409, the job `Aborted`, the feeder `ScannerAdfJam`.
    jam_after: Option<usize>,
    /// After this many documents the panel's Cancel key ends the job: 404, the job `Canceled`.
    panel_cancel_after: Option<usize>,
    /// Cancel this token once the first document is sent.
    cancel_after_first: Option<CancelToken>,
    /// Answer capabilities with a redirect.
    redirect: bool,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            capabilities: capabilities(),
            adf: Some("ScannerAdfLoaded"),
            sheets: 3,
            busy: 0,
            refuse: None,
            adf_after_refusal: None,
            jam_after: None,
            panel_cancel_after: None,
            cancel_after_first: None,
            redirect: false,
        }
    }
}

struct Job {
    path: String,
    feeder: bool,
    sent: usize,
    busy_left: usize,
    state: &'static str,
}

#[derive(Default)]
struct State {
    /// `METHOD /path` of every request.
    log: Vec<String>,
    /// The bodies of `POST ScanJobs`.
    posted: Vec<String>,
    adf: Option<&'static str>,
    job: Option<Job>,
    jobs: usize,
}

struct Fake {
    script: Script,
    state: Mutex<State>,
}

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

fn reply(status: u16, body: impl Into<Vec<u8>>) -> Reply {
    Reply { status, headers: Vec::new(), body: body.into() }
}

impl Fake {
    fn status_xml(&self, state: &State) -> String {
        let mut s = format!("<scan:ScannerStatus {NS}><pwg:Version>2.63</pwg:Version><pwg:State>Idle</pwg:State>");
        if let Some(adf) = state.adf {
            s.push_str(&format!("<scan:AdfState>{adf}</scan:AdfState>"));
        }
        if let Some(job) = &state.job {
            s.push_str(&format!(
                "<scan:Jobs><scan:JobInfo><pwg:JobUri>{}</pwg:JobUri><pwg:ImagesCompleted>{}</pwg:ImagesCompleted><pwg:JobState>{}</pwg:JobState></scan:JobInfo></scan:Jobs>",
                job.path, job.sent, job.state
            ));
        }
        s.push_str("</scan:ScannerStatus>");
        s
    }

    fn handle(&self, method: &str, path: &str, body: &[u8]) -> Reply {
        let mut state = self.state.lock().unwrap();
        state.log.push(format!("{method} {path}"));
        match (method, path) {
            ("GET", "/eSCL/ScannerCapabilities") if self.script.redirect => {
                Reply { status: 301, headers: vec![("Location", "https://127.0.0.1/eSCL/ScannerCapabilities".into())], body: Vec::new() }
            }
            ("GET", "/eSCL/ScannerCapabilities") => reply(200, self.script.capabilities.clone()),
            ("GET", "/eSCL/ScannerStatus") => reply(200, self.status_xml(&state)),
            ("POST", "/eSCL/ScanJobs") => {
                let text = String::from_utf8_lossy(body).into_owned();
                state.posted.push(text.clone());
                if let Some(status) = self.script.refuse {
                    state.adf = self.script.adf_after_refusal.or(state.adf);
                    return reply(status, "");
                }
                state.jobs += 1;
                let path = format!("/eSCL/ScanJobs/job-{}", state.jobs);
                // Another host and port in the Location, as some scanners write their own name: the client keeps its origin.
                let location = format!("http://192.0.2.1:9/{}", path.trim_start_matches('/'));
                state.job = Some(Job {
                    path,
                    feeder: text.contains("<pwg:InputSource>Feeder</pwg:InputSource>"),
                    sent: 0,
                    busy_left: self.script.busy,
                    state: "Processing",
                });
                Reply { status: 201, headers: vec![("Location", location)], body: Vec::new() }
            }
            ("DELETE", p) => {
                if let Some(job) = state.job.as_mut().filter(|j| j.path == p) {
                    job.state = "Canceled";
                    reply(200, "")
                } else {
                    reply(404, "")
                }
            }
            ("GET", p) if p.ends_with("/NextDocument") => {
                let script = &self.script;
                let mut adf = state.adf;
                let Some(job) = state.job.as_mut().filter(|j| format!("{}/NextDocument", j.path) == p && j.state == "Processing") else {
                    return reply(404, "");
                };
                if job.busy_left > 0 {
                    job.busy_left = job.busy_left.saturating_sub(1);
                    return reply(503, "");
                }
                let r = if script.jam_after == Some(job.sent) && job.feeder {
                    job.state = "Aborted";
                    adf = Some("ScannerAdfJam");
                    reply(409, "")
                } else if script.panel_cancel_after == Some(job.sent) {
                    job.state = "Canceled";
                    reply(404, "")
                } else if (!job.feeder && job.sent == 0) || (job.feeder && job.sent < script.sheets) {
                    job.sent += 1;
                    job.busy_left = script.busy;
                    if job.sent == 1
                        && let Some(token) = &script.cancel_after_first
                    {
                        token.request();
                    }
                    if job.feeder && job.sent == script.sheets {
                        adf = Some("ScannerAdfEmpty");
                    }
                    Reply { status: 200, headers: vec![("Content-Type", "image/png".into())], body: png(u8::try_from(job.sent * 40).unwrap()) }
                } else {
                    job.state = "Completed";
                    reply(404, "")
                };
                state.adf = adf;
                r
            }
            _ => reply(404, ""),
        }
    }
}

/// Read one request from `stream`: the method, the path and the body.
fn read_request(stream: &TcpStream) -> Option<(String, String, Vec<u8>)> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next()?.to_owned(), parts.next()?.to_owned());
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().ok()?;
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some((method, path, body))
}

/// Serve `script` on 127.0.0.1; the root URL and the fake.
fn serve(script: Script) -> (String, Arc<Fake>) {
    let adf = script.adf;
    let fake = Arc::new(Fake { script, state: Mutex::new(State { adf, ..State::default() }) });
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = Arc::clone(&fake);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let Some((method, path, body)) = read_request(&stream) else { continue };
            let r = server.handle(&method, &path, &body);
            let mut head = format!("HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n", r.status, r.body.len());
            for (name, value) in &r.headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            head.push_str("\r\n");
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&r.body);
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}/eSCL"), fake)
}

fn backend(url: &str) -> (EsclBackend, String) {
    let backend = EsclBackend::new().with_poll_interval(Duration::from_millis(5)).with_busy_wait(Duration::from_millis(300));
    let id = backend.add(url).unwrap();
    (backend, id)
}

fn log(fake: &Fake) -> Vec<String> {
    fake.state.lock().unwrap().log.clone()
}

fn count(fake: &Fake, prefix: &str) -> usize {
    log(fake).iter().filter(|l| l.starts_with(prefix)).count()
}

#[test]
fn details_map_the_capabilities_and_the_status() {
    let (url, fake) = serve(Script::default());
    let (backend, id) = backend(&url);
    assert!(log(&fake).is_empty(), "adding a scanner opens no connection");
    let s = backend.scanner_details(&id).unwrap();
    assert_eq!((s.id.as_str(), s.name.as_str()), (id.as_str(), "Fake eSCL Scanner"));
    assert!(s.flatbed && s.feeder && s.duplex);
    assert_eq!(s.feeder_ready, Some(true));
    assert_eq!(s.feeder_capacity, Some(35));
    assert_eq!(s.connection, printscan::scan::Connection::Other);
    assert_eq!(s.bed.map(|a| (a.width, a.height)), Some((8500, 11690)));
    assert_eq!(s.optical_x, Some(600));
    let glass = s.glass.unwrap();
    assert_eq!(glass.dpi, [150, 300]);
    assert_eq!(glass.colors, [Color::Rgb, Color::Gray]);
    assert_eq!(glass.formats, ["jpeg", "png"]);
    assert_eq!(glass.brightness.map(|b| (b.min, b.max)), Some((0, 100)));
    assert_eq!(log(&fake), ["GET /eSCL/ScannerCapabilities", "GET /eSCL/ScannerStatus"]);
    assert_eq!(backend.status(&id).unwrap().feeder_ready, Some(true));
    let dump = backend.diagnostic_dump("").unwrap();
    assert!(dump.contains("ScannerCapabilities") && dump.contains("Loaded"), "{dump}");
}

#[test]
fn the_registry_lists_escl_ids_without_contacting_anything() {
    let (url, fake) = serve(Script::default());
    let (backend, _) = backend(&url);
    let registry = Registry::new();
    registry.register(Box::new(backend)).unwrap();
    let listing = registry.scanner_list().unwrap();
    assert_eq!(listing.items[0].id, format!("escl:{url}"));
    assert!(log(&fake).is_empty());
}

#[test]
fn a_glass_scan_is_one_png_page_and_the_job_stays_on_this_scanner() {
    let (url, fake) = serve(Script::default());
    let (backend, id) = backend(&url);
    let settings = ScanSettings::new(id).with_source(Source::Glass).with_dpi(200).with_color(Color::Rgb);
    let acquired = backend.acquire(&settings).unwrap();
    assert_eq!((acquired.source, acquired.pages.len()), (Source::Glass, 1));
    let page = &acquired.pages[0];
    assert_eq!((page.format, page.width, page.height), ("png", 4, 4));
    let posted = fake.state.lock().unwrap().posted[0].clone();
    for part in [
        "<pwg:InputSource>Platen</pwg:InputSource>",
        "<scan:ColorMode>RGB24</scan:ColorMode>",
        "<scan:XResolution>300</scan:XResolution>",
        "image/png",
    ] {
        assert!(posted.contains(part), "{part} in {posted}");
    }
    // The Location named another host: the document was fetched from this one, once, and the finished job not deleted.
    assert_eq!(count(&fake, "GET /eSCL/ScanJobs/job-1/NextDocument"), 1);
    assert_eq!(count(&fake, "DELETE"), 0);
}

#[test]
fn a_whole_feeder_is_read_until_the_scanner_has_no_more() {
    let (url, fake) = serve(Script { sheets: 3, ..Script::default() });
    let (backend, id) = backend(&url);
    let acquired = backend.acquire(&ScanSettings::new(id).with_source(Source::Feeder).with_pages(0)).unwrap();
    assert_eq!(acquired.pages.len(), 3);
    assert_ne!(acquired.pages[0].bytes, acquired.pages[1].bytes);
    assert_eq!(count(&fake, "DELETE"), 0, "the 404 ended the job");
}

#[test]
fn auto_takes_the_loaded_feeder_and_two_sides_ask_for_duplex() {
    let (url, fake) = serve(Script { sheets: 4, ..Script::default() });
    let (backend, id) = backend(&url);
    let acquired = backend.acquire(&ScanSettings::new(id).with_pages(2).with_duplex(true)).unwrap();
    assert_eq!((acquired.source, acquired.pages.len()), (Source::Feeder, 4));
    assert!(fake.state.lock().unwrap().posted[0].contains("<scan:Duplex>true</scan:Duplex>"));
}

#[test]
fn a_fixed_count_from_a_fuller_feeder_ends_the_job() {
    let (url, fake) = serve(Script { sheets: 5, ..Script::default() });
    let (backend, id) = backend(&url);
    let acquired = backend.acquire(&ScanSettings::new(id).with_source(Source::Feeder).with_pages(2)).unwrap();
    assert_eq!(acquired.pages.len(), 2);
    assert_eq!(log(&fake).last().map(String::as_str), Some("DELETE /eSCL/ScanJobs/job-1"));
}

#[test]
fn an_empty_feeder_is_named_before_any_job_is_made() {
    let (url, fake) = serve(Script { adf: Some("ScannerAdfEmpty"), ..Script::default() });
    let (backend, id) = backend(&url);
    let error = backend.acquire(&ScanSettings::new(id.clone()).with_source(Source::Feeder)).unwrap_err();
    assert_eq!(error.interrupt_kind(), InterruptKind::FeederEmpty);
    assert_eq!(count(&fake, "POST"), 0);
    // Auto takes the glass instead.
    assert_eq!(backend.acquire(&ScanSettings::new(id)).unwrap().source, Source::Glass);
}

#[test]
fn a_refused_job_is_explained_from_the_feeder_state() {
    let (url, _) = serve(Script { adf: None, refuse: Some(409), adf_after_refusal: Some("ScannerAdfEmpty"), ..Script::default() });
    let (backend, id) = backend(&url);
    let error = backend.acquire(&ScanSettings::new(id).with_source(Source::Feeder)).unwrap_err();
    assert_eq!(error.interrupt_kind(), InterruptKind::FeederEmpty, "{error}");
    assert_eq!(error.native().and_then(|n| n.name), Some("ScannerAdfEmpty"));
}

#[test]
fn a_busy_scanner_is_busy_with_its_http_status() {
    let (url, _) = serve(Script { refuse: Some(503), ..Script::default() });
    let (backend, id) = backend(&url);
    let error = backend.acquire(&ScanSettings::new(id).with_source(Source::Glass)).unwrap_err();
    assert_eq!(error.interrupt_kind(), InterruptKind::Busy);
    assert_eq!(error.native().map(|n| (n.domain, n.code)), Some(("http", 503)));
}

#[test]
fn a_document_that_is_not_ready_is_asked_for_again() {
    let (url, fake) = serve(Script { busy: 2, ..Script::default() });
    let (backend, id) = backend(&url);
    assert_eq!(backend.acquire(&ScanSettings::new(id).with_source(Source::Glass)).unwrap().pages.len(), 1);
    assert_eq!(count(&fake, "GET /eSCL/ScanJobs/job-1/NextDocument"), 3);
}

#[test]
fn a_document_that_never_gets_ready_is_busy_and_the_job_is_ended() {
    let (url, fake) = serve(Script { busy: usize::MAX, ..Script::default() });
    let (backend, id) = backend(&url);
    let error = backend.acquire(&ScanSettings::new(id).with_source(Source::Glass)).unwrap_err();
    assert_eq!(error.interrupt_kind(), InterruptKind::Busy, "{error}");
    assert_eq!(count(&fake, "DELETE /eSCL/ScanJobs/job-1"), 1);
}

#[test]
fn a_jam_keeps_the_pages_that_arrived_and_names_the_jam() {
    let (url, _) = serve(Script { sheets: 5, jam_after: Some(2), ..Script::default() });
    let (backend, id) = backend(&url);
    match backend.acquire(&ScanSettings::new(id).with_source(Source::Feeder).with_pages(0)).unwrap_err() {
        ScanError::Incomplete { acquired, cause, .. } => {
            assert_eq!(acquired.pages.len(), 2);
            assert_eq!(cause.map(|c| c.kind), Some(InterruptKind::Jam));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_cancel_key_on_the_panel_is_a_cancel_at_the_device() {
    let (url, fake) = serve(Script { sheets: 5, panel_cancel_after: Some(1), ..Script::default() });
    let (backend, id) = backend(&url);
    let error = backend.acquire(&ScanSettings::new(id).with_source(Source::Feeder).with_pages(0)).unwrap_err();
    assert_eq!(error.interrupt_kind(), InterruptKind::Cancelled, "{error}");
    assert_eq!(printscan::scan::pages_in_error(&error), 1);
    assert_eq!(count(&fake, "DELETE"), 0, "the scanner already ended it");
}

#[test]
fn the_callers_cancel_stops_between_pages_and_ends_the_job() {
    let token = CancelToken::new();
    let (url, fake) = serve(Script { sheets: 5, cancel_after_first: Some(token.clone()), ..Script::default() });
    let (backend, id) = backend(&url);
    let acquired = backend.acquire(&ScanSettings::new(id).with_source(Source::Feeder).with_pages(0).with_cancel(token)).unwrap();
    assert_eq!(acquired.pages.len(), 1, "a cancel with pages in hand keeps them");
    assert_eq!(count(&fake, "DELETE /eSCL/ScanJobs/job-1"), 1);
}

#[test]
fn black_and_white_is_thresholded_from_grey() {
    let (url, fake) = serve(Script::default());
    let (backend, id) = backend(&url);
    let acquired = backend.acquire(&ScanSettings::new(id).with_source(Source::Glass).with_color(Color::BlackAndWhite)).unwrap();
    assert!(fake.state.lock().unwrap().posted[0].contains("<scan:ColorMode>Grayscale8</scan:ColorMode>"));
    assert!(acquired.pages[0].thresholded);
    assert_eq!(acquired.pages[0].format, "bmp");
}

#[test]
fn what_the_scanner_does_not_offer_is_refused_before_a_job() {
    let (url, fake) = serve(Script::default());
    let (backend, id) = backend(&url);
    assert!(backend.acquire(&ScanSettings::new(id).with_source(Source::Glass).with_format("tiff")).is_err());
    assert_eq!(count(&fake, "POST"), 0);
}

#[test]
fn redirects_and_hostile_documents_are_errors_not_panics() {
    let (url, _) = serve(Script { redirect: true, ..Script::default() });
    let (b, id) = backend(&url);
    let why = b.scanner_details(&id).unwrap_err().to_string();
    assert!(why.contains("redirect"), "{why}");
    for hostile in [
        "<?xml version=\"1.0\"?><!DOCTYPE x [<!ENTITY a \"a\">]><x>&a;</x>".to_owned(),
        "<scan:ScannerCapabilities".to_owned(),
        "<a>".repeat(500),
        format!(
            "<scan:ScannerCapabilities {NS}><scan:Platen><scan:PlatenInputCaps><scan:MaxWidth>-5</scan:MaxWidth></scan:PlatenInputCaps></scan:Platen></scan:ScannerCapabilities>"
        ),
    ] {
        let (url, _) = serve(Script { capabilities: hostile, ..Script::default() });
        let (b, id) = backend(&url);
        // Whatever the document, an answer, never a panic; a scan fails before any job when nothing usable was published.
        let _ = b.scanner_details(&id);
        let _ = b.acquire(&ScanSettings::new(id).with_source(Source::Glass));
    }
}

#[test]
fn a_scanner_that_is_off_is_offline() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let (b, id) = backend(&format!("http://127.0.0.1:{port}/eSCL"));
    assert_eq!(b.scanner_details(&id).unwrap_err().interrupt_kind(), InterruptKind::Offline);
    assert!(b.status(&id).is_none());
}

#[test]
fn only_numeric_local_addresses_are_added() {
    let b = EsclBackend::new();
    for bad in ["http://scanner.local/eSCL", "http://8.8.8.8/eSCL", "ftp://127.0.0.1/eSCL", "http://127.0.0.1/eSCL?x"] {
        assert!(b.add(bad).is_err(), "{bad}");
    }
    assert_eq!(b.add("http://192.168.7.7").unwrap(), "http://192.168.7.7:80/eSCL");
    assert_eq!(b.add("https://192.168.7.7/eSCL/").unwrap(), "https://192.168.7.7:443/eSCL");
    assert!(b.remove("http://192.168.7.7/eSCL"));
    assert_eq!(b.urls(), ["https://192.168.7.7:443/eSCL"]);
}

#[cfg(not(feature = "tls"))]
#[test]
fn https_without_the_tls_feature_says_so_and_connects_to_nothing() {
    let (url, fake) = serve(Script::default());
    let (b, id) = backend(&url.replace("http://", "https://"));
    let why = b.scanner_details(&id).unwrap_err().to_string();
    assert!(why.contains("tls"), "{why}");
    assert!(log(&fake).is_empty());
}

#[cfg(feature = "tls")]
#[test]
fn an_https_handshake_with_a_plain_http_server_fails_cleanly_in_both_modes() {
    use printscan_escl::TlsMode;
    for mode in [TlsMode::default(), TlsMode::AcceptAnyCertificate] {
        let (url, _) = serve(Script::default());
        let b = EsclBackend::with_tls(&mode);
        let id = b.add(&url.replace("http://", "https://")).unwrap();
        assert!(b.scanner_details(&id).is_err());
    }
    assert!(TlsMode::trust_pem(b"not pem").is_err());
    // The plain transport of a tls build still refuses https rather than guessing a trust mode.
    let (url, fake) = serve(Script::default());
    let (b, id) = backend(&url.replace("http://", "https://"));
    assert!(b.scanner_details(&id).unwrap_err().to_string().contains("TLS"));
    assert!(log(&fake).is_empty());
}
