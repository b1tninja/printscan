//! The WS-Scan backend (`printscan_wsd::scan`) against a small scripted scanner: no sockets for
//! most tests (the transport answers directly), and one over the loopback HTTP server.
#![cfg(all(feature = "scan", feature = "testing", not(target_arch = "wasm32")))]

use std::sync::{Arc, Mutex};

use printscan::scan::{Backend, CancelToken, Color, InterruptKind, ScanError, ScanSettings, Source};
use printscan_wsd::ns::FAULT_ACTION;
use printscan_wsd::scan::WsdScanBackend;
use printscan_wsd::scan::ns::WSCN;
use printscan_wsd::soap::reply_envelope;
use printscan_wsd::{DeviceUrl, Request, Response, Transport, mtom};

const URL: &str = "http://127.0.0.1:5358/ScannerService";

/// A 2 x 2, 24-bit BMP at 300 dpi; `shade` tells pages apart.
fn bmp(shade: u8) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"BM");
    for v in [54u32 + 16, 0, 54, 40, 2, 2] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    for v in [0u32, 16, 11811, 11811, 0, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    for _ in 0..2 {
        b.extend_from_slice(&[shade; 6]);
        b.extend_from_slice(&[0, 0]);
    }
    b
}

/// What the scripted scanner does.
#[derive(Default)]
struct Script {
    formats: Vec<&'static str>,
    /// Formats `CreateScanJob` refuses with `ClientErrorFormatNotSupported`.
    refuse: Vec<&'static str>,
    /// Sheets in the feeder.
    sheets: usize,
    /// `RetrieveImage` number n (1-based) fails with `OperationFailed`, and the status then names a jam.
    fail_at: Option<usize>,
    /// Cancel this token after the first image.
    cancel_after_first: Option<CancelToken>,
    /// `RetrieveImage` number n (1-based) is cut off in the middle of the image part, and
    /// `GetJobElements` then answers as given.
    cut_at: Option<(usize, AfterCut)>,
}

/// What `GetJobElements` says after a cut-off image.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AfterCut {
    /// The job's status: `Canceled`, `JobCanceledAtDevice` (the Cancel key on the panel).
    CancelledStatus,
    /// The fault a scanner that dropped the job sends: `ClientErrorJobCancelled`.
    CancelledFault,
    /// Still processing: the connection broke, nobody cancelled.
    Processing,
}

#[derive(Default)]
struct State {
    actions: Vec<String>,
    tickets: Vec<String>,
    jammed: bool,
    left: usize,
    sent: usize,
    /// Images still due for the open job; `None` once it ended.
    due: Option<usize>,
    /// The image reply was cut off.
    cut: bool,
}

struct Fake {
    script: Script,
    state: Mutex<State>,
}

impl Fake {
    fn new(script: Script) -> Arc<Self> {
        let left = script.sheets;
        Arc::new(Self { script, state: Mutex::new(State { left, ..State::default() }) })
    }

    fn actions(&self) -> Vec<String> {
        self.state.lock().unwrap().actions.clone()
    }

    fn tickets(&self) -> Vec<String> {
        self.state.lock().unwrap().tickets.clone()
    }

    fn reply(op: &str, relates: &str, body: &str) -> Response {
        let action = format!("{WSCN}/{op}Response");
        Response::new(200, Some("application/soap+xml".into()), reply_envelope(&action, relates, &[("wscn", WSCN)], body).into_bytes())
    }

    fn fault(relates: &str, code: &str) -> Response {
        let body = format!(
            "<s:Fault><s:Code><s:Value>s:Sender</s:Value><s:Subcode><s:Value>wscn:{code}</s:Value></s:Subcode></s:Code><s:Reason><s:Text xml:lang=\"en\">{code}</s:Text></s:Reason></s:Fault>"
        );
        Response::new(400, Some("application/soap+xml".into()), reply_envelope(FAULT_ACTION, relates, &[("wscn", WSCN)], &body).into_bytes())
    }

    fn elements(&self, jammed: bool) -> String {
        let formats: String = self.script.formats.iter().map(|f| format!("<wscn:FormatValue>{f}</wscn:FormatValue>")).collect();
        let input = |p: &str, h: u32| {
            format!(
                "<wscn:{p}OpticalResolution><wscn:Width>600</wscn:Width></wscn:{p}OpticalResolution>\
                 <wscn:{p}Resolutions><wscn:Widths><wscn:Width>150</wscn:Width><wscn:Width>300</wscn:Width></wscn:Widths><wscn:Heights><wscn:Height>150</wscn:Height><wscn:Height>300</wscn:Height></wscn:Heights></wscn:{p}Resolutions>\
                 <wscn:{p}Color><wscn:ColorEntry>RGB24</wscn:ColorEntry><wscn:ColorEntry>Grayscale8</wscn:ColorEntry></wscn:{p}Color>\
                 <wscn:{p}MinimumSize><wscn:Width>100</wscn:Width><wscn:Height>100</wscn:Height></wscn:{p}MinimumSize>\
                 <wscn:{p}MaximumSize><wscn:Width>8500</wscn:Width><wscn:Height>{h}</wscn:Height></wscn:{p}MaximumSize>"
            )
        };
        let reason = if jammed { "MediaJam" } else { "None" };
        format!(
            "<wscn:GetScannerElementsResponse><wscn:ScannerElements>\
             <wscn:ElementData Name=\"wscn:ScannerDescription\" Valid=\"true\"><wscn:ScannerDescription><wscn:ScannerName>Scripted</wscn:ScannerName></wscn:ScannerDescription></wscn:ElementData>\
             <wscn:ElementData Name=\"wscn:ScannerConfiguration\" Valid=\"true\"><wscn:ScannerConfiguration><wscn:DeviceSettings><wscn:FormatsSupported>{formats}</wscn:FormatsSupported>\
             <wscn:BrightnessSupported>true</wscn:BrightnessSupported></wscn:DeviceSettings>\
             <wscn:Platen>{}</wscn:Platen><wscn:ADF><wscn:ADFSupportsDuplex>false</wscn:ADFSupportsDuplex><wscn:ADFFront>{}</wscn:ADFFront></wscn:ADF></wscn:ScannerConfiguration></wscn:ElementData>\
             <wscn:ElementData Name=\"wscn:ScannerStatus\" Valid=\"true\"><wscn:ScannerStatus><wscn:ScannerCurrentTime>2026-11-01T00:00:00Z</wscn:ScannerCurrentTime><wscn:ScannerState>Idle</wscn:ScannerState>\
             <wscn:ScannerStateReasons><wscn:ScannerStateReason>{reason}</wscn:ScannerStateReason></wscn:ScannerStateReasons><wscn:ActiveConditions/></wscn:ScannerStatus></wscn:ElementData>\
             </wscn:ScannerElements></wscn:GetScannerElementsResponse>",
            input("Platen", 11690),
            input("ADF", 14000)
        )
    }

    fn answer(&self, request: &Request) -> Response {
        let body = String::from_utf8_lossy(&request.body).into_owned();
        let relates = body.split("<wsa:MessageID>urn:uuid:").nth(1).and_then(|r| r.split('<').next()).unwrap_or("").to_owned();
        let op = request.soap_action.rsplit('/').next().unwrap_or("").to_owned();
        let mut st = self.state.lock().unwrap();
        st.actions.push(op.clone());
        match op.as_str() {
            "GetScannerElements" => Self::reply(&op, &relates, &self.elements(st.jammed)),
            "CreateScanJob" => {
                st.tickets.push(body.clone());
                let format = body.split("<wscn:Format>").nth(1).and_then(|r| r.split('<').next()).unwrap_or("");
                if self.script.refuse.contains(&format) {
                    return Self::fault(&relates, "ClientErrorFormatNotSupported");
                }
                let images: usize =
                    body.split("<wscn:ImagesToTransfer>").nth(1).and_then(|r| r.split('<').next()).and_then(|n| n.parse().ok()).unwrap_or(1);
                let feeder = body.contains("<wscn:InputSource>ADF");
                st.due = Some(match (feeder, images) {
                    (true, 0) => st.left,
                    (true, n) => n.min(st.left),
                    (false, _) => 1,
                });
                if feeder {
                    st.left -= st.due.unwrap_or(0);
                }
                Self::reply(
                    &op,
                    &relates,
                    &format!(
                        "<wscn:CreateScanJobResponse><wscn:JobId>9</wscn:JobId><wscn:JobToken>tok</wscn:JobToken><wscn:ImageInformation><wscn:MediaFrontImageInfo><wscn:PixelsPerLine>2</wscn:PixelsPerLine><wscn:NumberOfLines>2</wscn:NumberOfLines><wscn:BytesPerLine>8</wscn:BytesPerLine></wscn:MediaFrontImageInfo></wscn:ImageInformation><wscn:DocumentFinalParameters><wscn:Format>{format}</wscn:Format></wscn:DocumentFinalParameters></wscn:CreateScanJobResponse>"
                    ),
                )
            }
            "RetrieveImage" => {
                assert!(body.contains("<wscn:JobToken>tok</wscn:JobToken>"), "the job token travels");
                match st.due {
                    Some(n) if n > 0 => {}
                    _ => return Self::fault(&relates, "ClientErrorNoImagesAvailable"),
                }
                st.sent += 1;
                if self.script.fail_at == Some(st.sent) {
                    st.jammed = true;
                    st.due = None;
                    return Self::fault(&relates, "OperationFailed");
                }
                let cut = self.script.cut_at.is_some_and(|(n, _)| n == st.sent);
                if cut {
                    st.cut = true;
                    st.due = None;
                }
                st.due = st.due.map(|n| n - 1);
                if st.sent == 1
                    && let Some(token) = &self.script.cancel_after_first
                {
                    token.request();
                }
                let cid = format!("img{}@scripted", st.sent);
                let envelope = reply_envelope(
                    &format!("{WSCN}/RetrieveImageResponse"),
                    &relates,
                    &[("wscn", WSCN)],
                    &format!(
                        "<wscn:RetrieveImageResponse><wscn:ScanData>{}</wscn:ScanData></wscn:RetrieveImageResponse>",
                        mtom::xop_include_ref(&cid)
                    ),
                );
                let package = mtom::package(&envelope, &cid, &bmp(u8::try_from(st.sent).unwrap()));
                let mut body = package.body;
                if cut {
                    body.truncate(body.len() - 40);
                }
                Response::new(200, Some(package.content_type), body)
            }
            "GetJobElements" => {
                let status = |state: &str, reason: &str| {
                    format!(
                        "<wscn:GetJobElementsResponse><wscn:JobElements><wscn:ElementData Name=\"wscn:JobStatus\" Valid=\"true\"><wscn:JobStatus>                         <wscn:JobId>9</wscn:JobId><wscn:JobState>{state}</wscn:JobState><wscn:JobStateReasons><wscn:JobStateReason>{reason}</wscn:JobStateReason></wscn:JobStateReasons>                         <wscn:ScansCompleted>0</wscn:ScansCompleted></wscn:JobStatus></wscn:ElementData></wscn:JobElements></wscn:GetJobElementsResponse>"
                    )
                };
                match self.script.cut_at.filter(|_| st.cut).map(|(_, after)| after) {
                    Some(AfterCut::CancelledStatus) => Self::reply(&op, &relates, &status("Canceled", "JobCanceledAtDevice")),
                    Some(AfterCut::CancelledFault) => Self::fault(&relates, "ClientErrorJobCancelled"),
                    _ => Self::reply(&op, &relates, &status("Processing", "JobScanning")),
                }
            }
            "CancelJob" => {
                st.due = None;
                Self::reply(&op, &relates, "<wscn:CancelJobResponse/>")
            }
            other => panic!("unexpected operation {other}"),
        }
    }
}

struct Wire(Arc<Fake>);

impl Transport for Wire {
    fn post(&self, _: &DeviceUrl, request: Request) -> printscan_wsd::Result<Response> {
        Ok(self.0.answer(&request))
    }
}

fn backend(fake: &Arc<Fake>) -> WsdScanBackend {
    let b = WsdScanBackend::with_transport(Arc::new(Wire(Arc::clone(fake))));
    b.add_named(URL, "lab").unwrap();
    b
}

fn lossless() -> Script {
    Script { formats: vec!["jfif", "png"], sheets: 3, ..Script::default() }
}

#[test]
fn listing_opens_no_connection_and_details_read_the_scanner() {
    let fake = Fake::new(lossless());
    let b = backend(&fake);
    let list = b.scanner_list().unwrap();
    assert_eq!((list.len(), list[0].name.as_str(), list[0].connection), (1, "lab", printscan::scan::Connection::Wsd));
    assert!(fake.actions().is_empty());
    let s = b.scanner_details(URL).unwrap();
    assert_eq!(s.name, "Scripted");
    assert!(s.flatbed && s.feeder && !s.duplex);
    assert_eq!(s.bed, Some((8500, 11690)));
    assert_eq!(s.glass.unwrap().dpi, [150, 300]);
    assert_eq!(fake.actions(), ["GetScannerElements"]);
    assert!(WsdScanBackend::with_transport(Arc::new(Wire(fake))).add("http://scanner.example/ScannerService").is_err(), "host names are refused");
}

#[test]
fn a_glass_scan_is_one_lossless_image() {
    let fake = Fake::new(lossless());
    let got = backend(&fake).acquire(&ScanSettings { dpi: 300, color: Color::Rgb, ..ScanSettings::default() }).unwrap();
    assert_eq!((got.source, got.pages.len()), (Source::Glass, 1));
    let page = &got.pages[0];
    assert_eq!((page.format, page.width, page.height, page.dpi), ("bmp", 2, 2, 300));
    assert_eq!(fake.actions(), ["GetScannerElements", "CreateScanJob", "RetrieveImage"], "a finished job is not cancelled");
    let ticket = &fake.tickets()[0];
    assert!(ticket.contains("<wscn:Format>png</wscn:Format>"), "png before jfif");
    assert!(ticket.contains("<wscn:InputSource>Platen</wscn:InputSource>"));
}

#[test]
fn an_open_ended_feeder_scan_runs_until_the_scanner_has_no_more() {
    let fake = Fake::new(lossless());
    let settings = ScanSettings { source: Source::Feeder, pages: 0, ..ScanSettings::default() };
    let got = backend(&fake).acquire(&settings).unwrap();
    assert_eq!(got.pages.len(), 3);
    assert_eq!(fake.actions().iter().filter(|a| *a == "RetrieveImage").count(), 4, "the fourth answers no images");
    assert!(!fake.actions().contains(&"CancelJob".to_string()));
}

#[test]
fn a_feeder_that_runs_short_keeps_the_pages_and_says_so() {
    let fake = Fake::new(lossless());
    let err = backend(&fake).acquire(&ScanSettings { source: Source::Feeder, pages: 5, ..ScanSettings::default() }).unwrap_err();
    let ScanError::Incomplete { acquired, cause, .. } = err else { panic!("{err:?}") };
    assert_eq!(acquired.pages.len(), 3);
    assert_eq!(cause.unwrap().kind, InterruptKind::FeederEmpty);

    let empty = Fake::new(Script { sheets: 0, ..lossless() });
    let err = backend(&empty).acquire(&ScanSettings { source: Source::Feeder, pages: 1, ..ScanSettings::default() }).unwrap_err();
    assert_eq!(err.interrupt_kind(), InterruptKind::FeederEmpty, "{err}");
}

#[test]
fn a_refused_format_falls_back_like_the_windows_driver_unless_one_was_asked_for() {
    let fake = Fake::new(Script { formats: vec![], refuse: vec!["png", "exif"], ..lossless() });
    backend(&fake).acquire(&ScanSettings { color: Color::Gray, ..ScanSettings::default() }).unwrap();
    let formats: Vec<String> =
        fake.tickets().iter().map(|t| t.split("<wscn:Format>").nth(1).unwrap().split('<').next().unwrap().to_owned()).collect();
    assert_eq!(formats, ["png", "exif", "tiff-single-uncompressed"]);

    let fake = Fake::new(Script { formats: vec![], refuse: vec!["png"], ..lossless() });
    let err = backend(&fake).acquire(&ScanSettings { format: Some("png".into()), ..ScanSettings::default() }).unwrap_err();
    assert_eq!(err.native().unwrap().name, Some("ClientErrorFormatNotSupported"));
    assert_eq!(fake.tickets().len(), 1);
}

#[test]
fn a_failure_mid_stack_keeps_the_pages_and_names_the_jam_the_status_shows() {
    let fake = Fake::new(Script { fail_at: Some(2), ..lossless() });
    let err = backend(&fake).acquire(&ScanSettings { source: Source::Feeder, pages: 3, ..ScanSettings::default() }).unwrap_err();
    let ScanError::Incomplete { acquired, cause, .. } = err else { panic!("{err:?}") };
    assert_eq!(acquired.pages.len(), 1);
    assert_eq!(cause.unwrap().kind, InterruptKind::Jam);
    assert_eq!(fake.actions().last().map(String::as_str), Some("GetScannerElements"), "the status was read after the job was cancelled");
    assert!(fake.actions().contains(&"CancelJob".to_string()));
    // The jam now blocks the next scan before a job is made.
    let err = backend(&fake).acquire(&ScanSettings::default()).unwrap_err();
    assert_eq!(err.interrupt_kind(), InterruptKind::Jam);
}

#[test]
fn a_cancel_stops_between_images_and_cancels_the_job() {
    // As for every backend (`after_transfer`), a cancel with pages in hand returns them.
    let token = CancelToken::new();
    let fake = Fake::new(Script { cancel_after_first: Some(token.clone()), ..lossless() });
    let settings = ScanSettings { source: Source::Feeder, pages: 3, cancel: Some(token), ..ScanSettings::default() };
    let acquired = backend(&fake).acquire(&settings).unwrap();
    assert_eq!(acquired.pages.len(), 1);
    assert_eq!(fake.actions().iter().filter(|a| *a == "RetrieveImage").count(), 1);
    assert_eq!(fake.actions().last().map(String::as_str), Some("CancelJob"));
}

#[test]
fn the_cancel_key_on_the_scanner_ends_the_scan_as_cancelled_at_the_device() {
    // The scanner cuts the image reply off; its job status (or a fault) then says why.
    for after in [AfterCut::CancelledStatus, AfterCut::CancelledFault] {
        let fake = Fake::new(Script { cut_at: Some((1, after)), ..lossless() });
        let err = backend(&fake).acquire(&ScanSettings::default()).unwrap_err();
        assert_eq!(err.interrupt_kind(), InterruptKind::Cancelled, "{err:?}");
        assert_eq!(err.to_string(), InterruptKind::Cancelled.sentence());
        assert_eq!(fake.actions(), ["GetScannerElements", "CreateScanJob", "RetrieveImage", "GetJobElements"], "the scanner ended the job itself");

        // With pages in hand from the feeder, they are kept.
        let fake = Fake::new(Script { cut_at: Some((2, after)), ..lossless() });
        let err = backend(&fake).acquire(&ScanSettings { source: Source::Feeder, pages: 3, ..ScanSettings::default() }).unwrap_err();
        let ScanError::Incomplete { acquired, cause, .. } = err else { panic!("{err:?}") };
        assert_eq!(acquired.pages.len(), 1);
        assert_eq!(cause.unwrap().kind, InterruptKind::Cancelled);
    }
}

#[test]
fn a_cut_off_image_nobody_cancelled_stays_a_failure_and_the_job_is_cancelled() {
    let fake = Fake::new(Script { cut_at: Some((1, AfterCut::Processing)), ..lossless() });
    let err = backend(&fake).acquire(&ScanSettings::default()).unwrap_err();
    assert_ne!(err.interrupt_kind(), InterruptKind::Cancelled, "{err:?}");
    let actions = fake.actions();
    let at = |op: &str| actions.iter().position(|a| a == op).unwrap();
    assert!(at("GetJobElements") < at("CancelJob"), "the job's status is read before our own CancelJob: {actions:?}");
}

#[test]
fn the_registry_routes_wsd_ids_to_the_backend() {
    let fake = Fake::new(lossless());
    let registry = printscan::scan::Registry::new();
    registry.register(Box::new(backend(&fake))).unwrap();
    let listing = registry.scanner_list().unwrap();
    assert_eq!(listing.items[0].id, format!("wsd:{URL}"));
    let got = registry.acquire(&ScanSettings { device: format!("wsd:{URL}"), ..ScanSettings::default() }).unwrap();
    assert_eq!(got.pages.len(), 1);
}

#[test]
fn a_scan_over_loopback_http_reads_the_mtom_image() {
    use printscan_wsd::testing::{Server, raw_response};
    let fake = Fake::new(lossless());
    let inner = Arc::clone(&fake);
    let server = Server::start(move |req| {
        let request = Request {
            content_type: req.header("content-type").unwrap_or_default().to_owned(),
            soap_action: req.header("soapaction").unwrap_or_default().trim_matches('"').to_owned(),
            body: req.body.clone(),
            timeout: std::time::Duration::from_secs(5),
            max_response: usize::MAX,
        };
        let response = inner.answer(&request);
        raw_response(response.status, response.content_type.as_deref().unwrap_or(""), &response.body)
    });
    let b = WsdScanBackend::new();
    let url = format!("http://127.0.0.1:{}/ScannerService", server.port);
    b.add(&url).unwrap();
    let got = b.acquire(&ScanSettings { device: url, ..ScanSettings::default() }).unwrap();
    assert_eq!((got.pages.len(), got.pages[0].width), (1, 2));
}
