//! The CUPS backend, through the registry, against a small fake CUPS scheduler on a loopback port.
//!
//! The fake reads and writes the IPP encoding with its own few lines below, not with
//! `printscan-ipp`'s codec, so an encoding bug cannot pass on both sides. It keeps queues (one of
//! them a class) and jobs, answers `CUPS-Get-Printers`, `CUPS-Get-Default`,
//! `Get-Printer-Attributes`, `Validate-Job`, `Print-Job`, `Get-Job-Attributes`, `Get-Jobs` and
//! `Cancel-Job`, and records every request. It talks to nothing but the test, on 127.0.0.1.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use printscan::print::backends::Backend;
use printscan::print::{
    Document, ErrorKind, Fingerprint, Job, JobFilter, JobState, Reconciled, Registry, SubmitRequest, Tag, Timestamp, UserConfirmed,
};
use printscan_cups::CupsBackend;
use printscan_wsd::testing::{HttpRequest, Server, raw_response};

const PDF: &[u8] = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\ntrailer\n<<>>\n%%EOF\n";

// ---- the IPP encoding, written for the fake --------------------------------------------------

const OPERATION: u8 = 0x01;
const JOB: u8 = 0x02;
const END: u8 = 0x03;
const PRINTER: u8 = 0x04;

const INTEGER: u8 = 0x21;
const BOOLEAN: u8 = 0x22;
const ENUM: u8 = 0x23;
const RANGE: u8 = 0x33;
const TEXT: u8 = 0x41;
const NAME: u8 = 0x42;
const KEYWORD: u8 = 0x44;
const URI: u8 = 0x45;
const CHARSET: u8 = 0x47;
const LANGUAGE: u8 = 0x48;
const MIME: u8 = 0x49;

/// One attribute of a request: its first value tag and its values' bytes.
struct Attr {
    name: String,
    tag: u8,
    values: Vec<Vec<u8>>,
}

/// A decoded request.
struct Req {
    major: u8,
    op: u16,
    id: u32,
    groups: Vec<(u8, Vec<Attr>)>,
    data: Vec<u8>,
}

fn take<'a>(bytes: &'a [u8], at: &mut usize, n: usize) -> Option<&'a [u8]> {
    let out = bytes.get(*at..*at + n)?;
    *at += n;
    Some(out)
}

fn take_u16(bytes: &[u8], at: &mut usize) -> Option<usize> {
    take(bytes, at, 2).map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
}

/// Header, then groups of tag/name/value records (a record with an empty name is another value of
/// the attribute before it), then the end tag and the document.
fn decode(bytes: &[u8]) -> Option<Req> {
    let mut at = 0;
    let head = take(bytes, &mut at, 8)?;
    let mut req = Req {
        major: head[0],
        op: u16::from_be_bytes([head[2], head[3]]),
        id: u32::from_be_bytes([head[4], head[5], head[6], head[7]]),
        groups: Vec::new(),
        data: Vec::new(),
    };
    loop {
        let tag = *take(bytes, &mut at, 1)?.first()?;
        if tag == END {
            req.data = bytes[at..].to_vec();
            return Some(req);
        }
        if tag <= 0x0F {
            req.groups.push((tag, Vec::new()));
            continue;
        }
        let name_len = take_u16(bytes, &mut at)?;
        let name = String::from_utf8(take(bytes, &mut at, name_len)?.to_vec()).ok()?;
        let value_len = take_u16(bytes, &mut at)?;
        let value = take(bytes, &mut at, value_len)?.to_vec();
        let (_, attrs) = req.groups.last_mut()?;
        if name.is_empty() {
            attrs.last_mut()?.values.push(value);
        } else {
            attrs.push(Attr { name, tag, values: vec![value] });
        }
    }
}

impl Req {
    fn attr(&self, group: u8, name: &str) -> Option<&Attr> {
        self.groups.iter().filter(|(t, _)| *t == group).flat_map(|(_, a)| a).find(|a| a.name == name)
    }

    fn text(&self, name: &str) -> Option<String> {
        self.attr(OPERATION, name).and_then(|a| String::from_utf8(a.values.first()?.clone()).ok())
    }

    fn int(&self, name: &str) -> Option<i32> {
        let value = self.attr(OPERATION, name)?.values.first()?;
        Some(i32::from_be_bytes(value.as_slice().try_into().ok()?))
    }

    fn names(&self, group: u8) -> Vec<String> {
        self.groups.iter().filter(|(t, _)| *t == group).flat_map(|(_, a)| a).map(|a| a.name.clone()).collect()
    }
}

/// Writes a reply.
struct Writer(Vec<u8>);

impl Writer {
    fn new(major: u8, status: u16, id: u32) -> Self {
        let mut out = vec![major, 0];
        out.extend(status.to_be_bytes());
        out.extend(id.to_be_bytes());
        let mut w = Self(out);
        w.group(OPERATION);
        w.value(CHARSET, "attributes-charset", b"utf-8");
        w.value(LANGUAGE, "attributes-natural-language", b"en");
        w
    }

    fn group(&mut self, tag: u8) -> &mut Self {
        self.0.push(tag);
        self
    }

    fn value(&mut self, tag: u8, name: &str, value: &[u8]) -> &mut Self {
        self.0.push(tag);
        self.0.extend(u16::try_from(name.len()).unwrap().to_be_bytes());
        self.0.extend(name.as_bytes());
        self.0.extend(u16::try_from(value.len()).unwrap().to_be_bytes());
        self.0.extend(value);
        self
    }

    fn str(&mut self, tag: u8, name: &str, value: &str) -> &mut Self {
        self.value(tag, name, value.as_bytes())
    }

    fn strs(&mut self, tag: u8, name: &str, values: &[&str]) -> &mut Self {
        for (i, v) in values.iter().enumerate() {
            self.value(tag, if i == 0 { name } else { "" }, v.as_bytes());
        }
        self
    }

    fn int(&mut self, tag: u8, name: &str, value: i32) -> &mut Self {
        self.value(tag, name, &value.to_be_bytes())
    }

    fn ints(&mut self, tag: u8, name: &str, values: &[i32]) -> &mut Self {
        for (i, v) in values.iter().enumerate() {
            self.value(tag, if i == 0 { name } else { "" }, &v.to_be_bytes());
        }
        self
    }

    fn boolean(&mut self, name: &str, value: bool) -> &mut Self {
        self.value(BOOLEAN, name, &[u8::from(value)])
    }

    fn finish(mut self) -> Vec<u8> {
        self.0.push(END);
        self.0
    }
}

// ---- the fake scheduler ------------------------------------------------------------------------

#[derive(Clone)]
struct Queue {
    name: &'static str,
    info: &'static str,
    class: bool,
    accepting: bool,
}

#[derive(Clone, Debug)]
struct FakeJob {
    id: i32,
    queue: String,
    name: String,
    user: String,
    state: i32,
    format: String,
    document: Vec<u8>,
}

/// One request as the fake saw it.
#[derive(Clone, Debug)]
struct Seen {
    op: u16,
    printer_uri: Option<String>,
    user: Option<String>,
    attributes: Vec<String>,
}

struct State {
    queues: Vec<Queue>,
    default: Option<&'static str>,
    jobs: Vec<FakeJob>,
    next_job: i32,
    cancel_needs_auth: bool,
    seen: Vec<Seen>,
}

struct Fake {
    server: Server,
    state: Arc<Mutex<State>>,
}

impl Fake {
    /// Queues `Office` (the default), `Team` (a class) and `Büro`, and no jobs.
    fn start() -> Self {
        let state = Arc::new(Mutex::new(State {
            queues: vec![
                Queue { name: "Office", info: "Office laser", class: false, accepting: true },
                Queue { name: "Team", info: "", class: true, accepting: true },
                Queue { name: "Büro", info: "Büro, 2. Stock", class: false, accepting: true },
            ],
            default: Some("Office"),
            jobs: Vec::new(),
            next_job: 100,
            cancel_needs_auth: false,
            seen: Vec::new(),
        }));
        let shared = Arc::clone(&state);
        let server = Server::start(move |http| respond(&shared, http));
        Self { server, state }
    }

    fn backend(&self) -> CupsBackend {
        CupsBackend::new().with_server(&format!("ipp://127.0.0.1:{}/", self.server.port)).unwrap()
    }

    fn registry(&self) -> Registry {
        let registry = Registry::new();
        registry.register(Box::new(self.backend())).unwrap();
        registry
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    fn ops(&self) -> Vec<u16> {
        self.state().seen.iter().map(|s| s.op).collect()
    }
}

/// `/printers/<name>` or `/classes/<name>` of a printer URI, percent-decoded: (is a class, name).
fn resource(uri: &str) -> Option<(bool, String)> {
    let path = &uri[uri.find("://")? + 3..];
    let path = &path[path.find('/')?..];
    let (class, encoded) = if let Some(rest) = path.strip_prefix("/classes/") { (true, rest) } else { (false, path.strip_prefix("/printers/")?) };
    let bytes = encoded.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            out.push(u8::from_str_radix(std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some((class, String::from_utf8(out).ok()?))
}

fn respond(state: &Mutex<State>, http: &HttpRequest) -> Vec<u8> {
    if http.header("content-type") != Some("application/ipp") {
        return raw_response(400, "text/plain", b"not ipp");
    }
    let Some(req) = decode(&http.body) else { return raw_response(400, "text/plain", b"bad ipp") };
    let mut state = state.lock().unwrap();
    state.seen.push(Seen {
        op: req.op,
        printer_uri: req.text("printer-uri"),
        user: req.text("requesting-user-name"),
        attributes: req.names(OPERATION),
    });
    // The operation group starts with the charset, as a charset, then the language (RFC 8011).
    let first = req.groups.first().and_then(|(_, a)| a.first()).map(|a| (a.name.as_str(), a.tag));
    let second = req.groups.first().and_then(|(_, a)| a.get(1)).map(|a| (a.name.as_str(), a.tag));
    if first != Some(("attributes-charset", CHARSET)) || second != Some(("attributes-natural-language", LANGUAGE)) {
        return raw_response(200, "application/ipp", &Writer::new(req.major, 0x0400, req.id).finish());
    }
    if req.op == 0x0008 && state.cancel_needs_auth {
        return raw_response(401, "text/plain", b"authenticate");
    }
    let body = answer(&mut state, &req);
    raw_response(200, "application/ipp", &body)
}

fn printer_group(w: &mut Writer, queue: &Queue, full: bool) {
    w.group(PRINTER);
    w.str(NAME, "printer-name", queue.name);
    w.str(TEXT, "printer-info", queue.info);
    w.str(TEXT, "printer-location", "Room 2");
    w.int(ENUM, "printer-type", if queue.class { 0x0001 | 0x0004 } else { 0x0004 | 0x0002_0000 });
    w.int(ENUM, "printer-state", 3);
    w.strs(KEYWORD, "printer-state-reasons", &["none"]);
    w.boolean("printer-is-accepting-jobs", queue.accepting);
    w.strs(KEYWORD, "sides-supported", &["one-sided", "two-sided-long-edge", "two-sided-short-edge"]);
    if full {
        w.ints(ENUM, "operations-supported", &[0x0002, 0x0004, 0x0008, 0x0009, 0x000A, 0x000B, 0x4001, 0x4002]);
        w.strs(MIME, "document-format-supported", &["application/pdf", "application/octet-stream", "image/jpeg"]);
        w.boolean("color-supported", true);
        w.strs(KEYWORD, "print-color-mode-supported", &["monochrome", "color"]);
        w.strs(KEYWORD, "media-supported", &["iso_a4_210x297mm", "na_letter_8.5x11in"]);
        let mut range = 1i32.to_be_bytes().to_vec();
        range.extend(99i32.to_be_bytes());
        w.value(RANGE, "copies-supported", &range);
        w.strs(KEYWORD, "ipp-versions-supported", &["1.1", "2.0"]);
        w.int(INTEGER, "multiple-operation-time-out", 300);
    }
}

fn job_group(w: &mut Writer, job: &FakeJob) {
    w.group(JOB);
    w.int(INTEGER, "job-id", job.id);
    w.str(URI, "job-uri", &format!("ipp://localhost/jobs/{}", job.id));
    w.int(ENUM, "job-state", job.state);
    w.strs(KEYWORD, "job-state-reasons", &["none"]);
    w.str(NAME, "job-name", &job.name);
    w.str(NAME, "job-originating-user-name", &job.user);
}

fn answer(state: &mut State, req: &Req) -> Vec<u8> {
    let ok = || Writer::new(req.major, 0, req.id);
    let status = |code: u16| Writer::new(req.major, code, req.id).finish();
    let target = req.text("printer-uri").and_then(|u| resource(&u));
    let queue = target.as_ref().and_then(|(class, name)| state.queues.iter().find(|q| q.name == name && q.class == *class).cloned());
    match req.op {
        0x4002 => {
            let limit = usize::try_from(req.int("limit").unwrap_or(1000)).unwrap();
            let mut w = ok();
            for queue in state.queues.iter().take(limit) {
                printer_group(&mut w, queue, false);
            }
            w.finish()
        }
        0x4001 => match state.default.and_then(|d| state.queues.iter().find(|q| q.name == d)) {
            Some(queue) => {
                let mut w = ok();
                printer_group(&mut w, queue, false);
                w.finish()
            }
            None => status(0x0406),
        },
        0x000B | 0x0004 | 0x0002 | 0x000A => {
            let Some(queue) = queue else { return status(0x0406) };
            match req.op {
                0x000B => {
                    let mut w = ok();
                    printer_group(&mut w, &queue, true);
                    w.finish()
                }
                0x0004 => ok().finish(),
                0x0002 => {
                    if !queue.accepting {
                        return status(0x0506);
                    }
                    let format = req.text("document-format").unwrap_or_else(|| "application/octet-stream".into());
                    let job = FakeJob {
                        id: state.next_job,
                        queue: queue.name.to_owned(),
                        name: req.text("job-name").unwrap_or_default(),
                        user: req.text("requesting-user-name").unwrap_or_default(),
                        state: 3,
                        format,
                        document: req.data.clone(),
                    };
                    state.next_job += 1;
                    let mut w = ok();
                    job_group(&mut w, &job);
                    state.jobs.push(job);
                    w.finish()
                }
                _ => {
                    let completed = req.text("which-jobs").as_deref() == Some("completed");
                    let mut w = ok();
                    for job in state.jobs.iter().filter(|j| j.queue == queue.name && (j.state >= 7) == completed) {
                        job_group(&mut w, job);
                    }
                    w.finish()
                }
            }
        }
        0x0009 | 0x0008 => {
            let Some(job) = req.int("job-id").and_then(|id| state.jobs.iter_mut().find(|j| j.id == id)) else { return status(0x0406) };
            if req.op == 0x0009 {
                let job = job.clone();
                let mut w = ok();
                job_group(&mut w, &job);
                return w.finish();
            }
            if job.state >= 7 {
                return status(0x0404);
            }
            job.state = 7;
            ok().finish()
        }
        _ => status(0x0501),
    }
}

fn consented(printer: Option<&str>, title: &str) -> SubmitRequest<'static> {
    let mut job = Job::default().with_title(title);
    job.printer = printer.map(str::to_owned);
    SubmitRequest::new(Document::pdf(PDF), job).with_consent(UserConfirmed::attest(Some(true)).unwrap())
}

// ---- the tests ---------------------------------------------------------------------------------

#[test]
fn queues_are_listed_with_the_default_and_the_classes() {
    let fake = Fake::start();
    let listing = fake.registry().list_printers().unwrap();
    assert!(listing.is_complete());
    let rows: Vec<(String, String, bool, Option<bool>)> =
        listing.items.iter().map(|p| (p.id.clone(), p.name.clone(), p.default, p.two_sided)).collect();
    assert_eq!(
        rows,
        [
            ("cups:Office".to_owned(), "Office laser".to_owned(), true, Some(true)),
            ("cups:Team".to_owned(), "Team".to_owned(), false, Some(true)),
            ("cups:Büro".to_owned(), "Büro, 2. Stock".to_owned(), false, Some(true)),
        ]
    );
    let seen = fake.state().seen.clone();
    assert_eq!(seen.iter().map(|s| s.op).collect::<Vec<_>>(), [0x4002, 0x4001]);
    for s in &seen {
        // Scheduler operations name no printer, and the user is the fixed one, not the account's.
        assert_eq!(s.printer_uri, None);
        assert_eq!(s.user.as_deref(), Some("printscan"));
        assert_eq!(s.attributes[..2], ["attributes-charset", "attributes-natural-language"]);
        assert!(s.attributes.contains(&"requested-attributes".to_owned()));
    }
    assert!(seen[0].attributes.contains(&"limit".to_owned()));

    let queues = fake.backend().queues().unwrap();
    assert!(queues[1].is_class && !queues[0].is_class);
    assert_eq!(queues[0].location.as_deref(), Some("Room 2"));
}

#[test]
fn the_default_queue_is_read_and_its_absence_is_not_an_error() {
    let fake = Fake::start();
    let backend = fake.backend();
    let default = backend.default_queue().unwrap().unwrap();
    assert_eq!((default.name.as_str(), default.default), ("Office", true));
    fake.state().default = None;
    assert_eq!(backend.default_queue().unwrap(), None);
    let listing = fake.registry().list_printers().unwrap();
    assert!(listing.items.iter().all(|p| !p.default));
    // Without a default, a job that names no queue has nowhere to go.
    let error = backend.submit(consented(None, "Report")).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NotFound);
    assert!(fake.state().jobs.is_empty());
}

#[test]
fn a_queue_describes_itself_and_a_class_is_asked_at_its_own_path() {
    let fake = Fake::start();
    let registry = fake.registry();
    let details = registry.printer_details("cups:Office").unwrap();
    assert_eq!(details.id, "cups:Office");
    assert!(details.default);
    assert!(details.formats.contains(&printscan::print::DocumentFormat::Pdf));
    assert_eq!(details.two_sided, Some(true));
    assert_eq!(details.colour, Some(true));
    assert_eq!(details.max_copies, Some(99));
    assert!(details.paper.contains(&"iso_a4_210x297mm".to_owned()));
    let get_attributes = fake.state().seen.iter().find(|s| s.op == 0x000B).cloned().unwrap();
    assert_eq!(get_attributes.printer_uri.as_deref(), Some(format!("ipp://127.0.0.1:{}/printers/Office", fake.server.port).as_str()));

    // A class is known as one once it has been listed, and is then asked at /classes/.
    let backend = fake.backend();
    backend.queues().unwrap();
    let details = backend.printer_details("Team").unwrap();
    assert_eq!(details.id, "Team");
    let last = fake.state().seen.iter().rev().find(|s| s.op == 0x000B).cloned().unwrap();
    assert_eq!(last.printer_uri.as_deref(), Some(format!("ipp://127.0.0.1:{}/classes/Team", fake.server.port).as_str()));

    // The listing's printer-info is the name, and a name outside ASCII is percent-encoded.
    backend.queues().unwrap();
    assert_eq!(backend.printer_details("Büro").unwrap().name, "Büro, 2. Stock");
}

#[test]
fn unknown_and_malformed_queues_are_refused() {
    let fake = Fake::start();
    let registry = fake.registry();
    assert_eq!(registry.printer_details("cups:Nope").unwrap_err().kind(), ErrorKind::NotFound);
    let before = fake.ops().len();
    for bad in ["cups:a/b", "cups:..", "cups:with space", "cups:"] {
        assert_eq!(registry.printer_details(bad).unwrap_err().kind(), ErrorKind::InvalidRequest, "{bad}");
    }
    // Nothing was sent for a name that is not one.
    assert_eq!(fake.ops().len(), before);
}

#[test]
fn a_pdf_prints_with_consent_and_is_followed_to_the_end() {
    let fake = Fake::start();
    let registry = fake.registry();
    let submitted = registry.submit_job(consented(Some("cups:Office"), "Quarterly report")).unwrap();
    let job = submitted.job.clone().unwrap();
    assert_eq!(job.to_string(), "cups:Office#100");
    assert_eq!(submitted.backend, "cups");
    assert!(submitted.message.contains("CUPS queue Office"), "{}", submitted.message);
    {
        let state = fake.state();
        let printed = &state.jobs[0];
        assert_eq!((printed.queue.as_str(), printed.format.as_str(), printed.user.as_str()), ("Office", "application/pdf", "printscan"));
        assert_eq!(printed.document, PDF);
        assert!(printed.name.starts_with("Quarterly report"), "{}", printed.name);
    }
    assert_eq!(registry.job_status(&job).unwrap().state, JobState::Pending);
    fake.state().jobs[0].state = 5;
    assert_eq!(registry.job_status(&job).unwrap().state, JobState::Processing);
    fake.state().jobs[0].state = 9;
    assert_eq!(registry.job_status(&job).unwrap().state, JobState::Completed);
    // A job the scheduler has forgotten is not found.
    fake.state().jobs.clear();
    assert_eq!(registry.job_status(&job).unwrap_err().kind(), ErrorKind::NotFound);
}

#[test]
fn a_job_without_consent_never_reaches_the_scheduler() {
    let fake = Fake::start();
    let request = SubmitRequest::new(Document::pdf(PDF), Job::default().with_printer("cups:Office"));
    assert_eq!(fake.registry().submit_job(request).unwrap_err().kind(), ErrorKind::ConsentRequired);
    // The backend refuses on its own too, when called without the registry.
    let request = SubmitRequest::new(Document::pdf(PDF), Job::default().with_printer("Office"));
    assert_eq!(fake.backend().submit(request).unwrap_err().kind(), ErrorKind::ConsentRequired);
    assert!(fake.ops().iter().all(|op| *op != 0x0002));
    assert!(fake.state().jobs.is_empty());
}

#[test]
fn a_job_that_names_no_queue_goes_to_the_default() {
    let fake = Fake::start();
    let submitted = fake.backend().submit(consented(None, "Default")).unwrap();
    assert_eq!(submitted.job.unwrap().to_string(), "cups:Office#100");
    assert_eq!(fake.state().jobs[0].queue, "Office");
}

#[test]
fn a_queue_named_outside_ascii_prints() {
    let fake = Fake::start();
    let submitted = fake.registry().submit_job(consented(Some("cups:Büro"), "Brief")).unwrap();
    assert_eq!(submitted.job.unwrap().to_string(), "cups:Büro#100");
    assert_eq!(fake.state().jobs[0].queue, "Büro");
}

#[test]
fn a_queue_that_does_not_accept_jobs_refuses_safely() {
    let fake = Fake::start();
    fake.state().queues[0].accepting = false;
    let error = fake.registry().submit_job(consented(Some("cups:Office"), "Report")).unwrap_err();
    assert!(!error.outcome_unknown(), "{error}");
    assert!(fake.state().jobs.is_empty());
}

#[test]
fn jobs_are_cancelled_and_cancelling_a_finished_one_is_not_an_error() {
    let fake = Fake::start();
    let registry = fake.registry();
    let job = registry.submit_job(consented(Some("cups:Office"), "Draft")).unwrap().job.unwrap();
    registry.cancel_job(&job).unwrap();
    assert_eq!(fake.state().jobs[0].state, 7);
    assert_eq!(registry.job_status(&job).unwrap().state, JobState::Cancelled);
    // Already ended: client-error-not-possible, confirmed as terminal.
    registry.cancel_job(&job).unwrap();

    // A scheduler that wants authentication for the cancel says so as an error, and the job stays.
    let job = registry.submit_job(consented(Some("cups:Office"), "Second")).unwrap().job.unwrap();
    fake.state().cancel_needs_auth = true;
    assert!(registry.cancel_job(&job).is_err());
    assert_eq!(fake.state().jobs[1].state, 3);
}

#[test]
fn jobs_are_listed_and_found_by_tag_with_cups_ids() {
    let fake = Fake::start();
    let registry = fake.registry();
    let tag = Tag::from_seed(7);
    let request = consented(Some("cups:Office"), "Tagged").with_tag(tag);
    let job = registry.submit_job(request).unwrap().job.unwrap();
    registry.submit_job(consented(Some("cups:Office"), "Other")).unwrap();
    fake.state().jobs[1].state = 9;

    let jobs = registry.list_jobs("cups:Office", &JobFilter::default()).unwrap();
    let ids: Vec<String> = jobs.iter().map(|j| j.id.to_string()).collect();
    assert_eq!(ids, ["cups:Office#101", "cups:Office#100"]);

    let fingerprint = Fingerprint::new("Tagged", Timestamp::from_system_time(SystemTime::now()).unwrap()).with_tag(tag);
    match registry.find_job("cups:Office", &fingerprint).unwrap() {
        Reconciled::Found(found) => assert_eq!(found.id, job),
        other => panic!("expected the tagged job, got {other:?}"),
    }
}
