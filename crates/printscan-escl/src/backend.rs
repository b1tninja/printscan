//! [`EsclBackend`]: the eSCL client as a [`printscan::scan::Backend`].

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, Instant};

use printscan::scan::{
    self, Acquired, Area, Backend, Color, Connection, HandlingStatus, ImageChoices, InterruptKind, NativeCode, Page, ScanError, ScanSettings,
    Scanner, ScannerRef, Source, ValueSpan, bw,
};
use printscan_wsd::{CancelToken, NetworkLimits};

use crate::classify::{self, HTTP_DOMAIN};
use crate::client::{self, Client, Job, NextDocument};
use crate::model::{AdfState, Capabilities, ColorMode, InputCaps, Range, Status};
use crate::settings::{self, format_name, intent_from, listed_resolutions, to_mils};
use crate::{Error, ScannerUrl, Transport};

/// The registry name and id prefix.
const NAME: &str = "escl";

/// A transport the backend can hold and share between threads.
type SharedTransport = Arc<dyn Transport + Send + Sync>;

#[derive(Clone, Debug)]
struct Entry {
    url: ScannerUrl,
    name: Option<String>,
}

#[derive(Clone, Debug)]
struct Config {
    limits: NetworkLimits,
    document_limits: NetworkLimits,
    poll: Duration,
    busy_wait: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            limits: NetworkLimits::default(),
            document_limits: client::document_limits(),
            poll: Duration::from_secs(1),
            busy_wait: Duration::from_secs(60),
        }
    }
}

/// An eSCL backend. Registers as `escl`; its scanners are the eSCL roots added to it, and their
/// native ids are those URLs (`http://192.168.1.20:80/eSCL`). It discovers nothing and opens no
/// connection until it is asked for a scanner's details, its status or a scan.
///
/// Clones share the scanner list: register one clone and keep the other to [`add`](Self::add) or
/// [`remove`](Self::remove) scanners later.
#[derive(Clone)]
pub struct EsclBackend {
    scanners: Arc<RwLock<Vec<Entry>>>,
    transport: SharedTransport,
    config: Arc<Config>,
}

impl fmt::Debug for EsclBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EsclBackend").field("scanners", &self.urls()).field("config", &self.config).finish_non_exhaustive()
    }
}

impl Default for EsclBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl EsclBackend {
    /// A backend with no scanners and the plain-`http` transport. Opens no connection.
    pub fn new() -> Self {
        Self::with_transport(Arc::new(crate::http::HttpTransport::new()))
    }

    /// A backend that also speaks `https`, checking scanners as `mode` says (cargo feature `tls`).
    #[cfg(feature = "tls")]
    pub fn with_tls(mode: &crate::TlsMode) -> Self {
        Self::with_transport(Arc::new(crate::http::HttpTransport::with_tls(mode)))
    }

    /// A backend that sends through `transport` (a test double, a recording proxy).
    pub fn with_transport(transport: SharedTransport) -> Self {
        Self { scanners: Arc::default(), transport, config: Arc::default() }
    }

    fn configured(mut self, change: impl FnOnce(&mut Config)) -> Self {
        let mut config = (*self.config).clone();
        change(&mut config);
        self.config = Arc::new(config);
        self
    }

    /// Limits for every request but `NextDocument`.
    #[must_use]
    pub fn with_limits(self, limits: NetworkLimits) -> Self {
        self.configured(|c| c.limits = limits)
    }

    /// Limits for `NextDocument` (default [`client::document_limits`]).
    #[must_use]
    pub fn with_document_limits(self, limits: NetworkLimits) -> Self {
        self.configured(|c| c.document_limits = limits)
    }

    /// How long to wait before asking again for a document the scanner says is not ready (503)
    /// when it names no `Retry-After` (default one second).
    #[must_use]
    pub fn with_poll_interval(self, poll: Duration) -> Self {
        self.configured(|c| c.poll = poll)
    }

    /// How long one document may stay not ready before the scan fails as busy (default 60 seconds).
    #[must_use]
    pub fn with_busy_wait(self, wait: Duration) -> Self {
        self.configured(|c| c.busy_wait = wait)
    }

    /// Add a scanner by its eSCL root (`http://<numeric local address>[:port]/eSCL`; a URL without a
    /// path gets `/eSCL`) and return its native id, the canonical URL. Adding one twice keeps one
    /// entry. Opens no connection.
    pub fn add(&self, url: &str) -> Result<String, ScanError> {
        self.add_entry(url, None)
    }

    /// [`Self::add`] with the name a person sees until the scanner reports its own.
    pub fn add_named(&self, url: &str, name: &str) -> Result<String, ScanError> {
        self.add_entry(url, Some(name.to_owned()))
    }

    fn add_entry(&self, url: &str, name: Option<String>) -> Result<String, ScanError> {
        let url = ScannerUrl::parse(url).map_err(|e| ScanError::Failed(e.to_string()))?;
        let id = url.as_url();
        let mut scanners = self.scanners.write().unwrap_or_else(PoisonError::into_inner);
        match scanners.iter_mut().find(|e| e.url == url) {
            Some(existing) if name.is_some() => existing.name = name,
            Some(_) => {}
            None => scanners.push(Entry { url, name }),
        }
        Ok(id)
    }

    /// Remove a scanner; `false` when it was not added.
    pub fn remove(&self, url: &str) -> bool {
        let Ok(url) = ScannerUrl::parse(url) else { return false };
        let mut scanners = self.scanners.write().unwrap_or_else(PoisonError::into_inner);
        let before = scanners.len();
        scanners.retain(|e| e.url != url);
        scanners.len() != before
    }

    /// The added scanners' native ids (canonical URLs).
    pub fn urls(&self) -> Vec<String> {
        self.snapshot().iter().map(|e| e.url.as_url()).collect()
    }

    fn snapshot(&self) -> Vec<Entry> {
        self.scanners.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The scanner `device` names: its URL, a fragment of its URL or name, or (empty) the first.
    fn entry(&self, device: &str) -> Result<Entry, ScanError> {
        let all = self.snapshot();
        if device.is_empty() {
            return all.into_iter().next().ok_or_else(|| ScanError::Failed("no eSCL scanner has been added".into()));
        }
        if let Ok(url) = ScannerUrl::parse(device)
            && let Some(e) = all.iter().find(|e| e.url == url)
        {
            return Ok(e.clone());
        }
        let needle = device.to_lowercase();
        let mut found = all
            .into_iter()
            .filter(|e| e.url.as_url().to_lowercase().contains(&needle) || e.name.as_deref().is_some_and(|n| n.to_lowercase().contains(&needle)));
        match (found.next(), found.next()) {
            (Some(e), None) => Ok(e),
            (Some(_), Some(_)) => Err(ScanError::Failed(format!("{device:?} names more than one eSCL scanner"))),
            (None, _) => Err(ScanError::Failed(format!("no eSCL scanner {device:?} has been added"))),
        }
    }

    fn client(&self, entry: &Entry) -> Client<SharedTransport> {
        Client::new(Arc::clone(&self.transport), entry.url.clone()).with_limits(self.config.limits).with_document_limits(self.config.document_limits)
    }

    fn details(&self, entry: &Entry) -> Result<Scanner, ScanError> {
        let client = self.client(entry);
        let caps = client.capabilities().map_err(|e| classify::scan_error("ScannerCapabilities", &e))?;
        let status = client.status().ok();
        Ok(scanner(entry, &caps, status.as_ref()))
    }
}

fn span(range: Option<Range>) -> Option<ValueSpan> {
    range.map(|r| ValueSpan::new(r.min, r.max, r.step))
}

/// What one input offers, in the scan core's terms.
fn choices(input: &InputCaps, caps: &Capabilities) -> ImageChoices {
    let mut choices = ImageChoices::default();
    choices.dpi = listed_resolutions(input);
    for mode in &input.color_modes {
        let color = match mode {
            ColorMode::Rgb24 => Color::Rgb,
            ColorMode::Grayscale8 => Color::Gray,
            ColorMode::BlackAndWhite1 => Color::BlackAndWhite,
            _ => continue,
        };
        if !choices.colors.contains(&color) {
            choices.colors.push(color);
        }
    }
    choices.brightness = span(caps.brightness);
    choices.contrast = span(caps.contrast);
    choices.threshold = span(caps.threshold);
    choices.formats = input.formats.iter().map(|f| format_name(f)).collect();
    choices.intents = input.intents.iter().filter_map(|i| intent_from(i)).collect();
    choices
}

fn area(input: Option<&InputCaps>) -> Option<Area> {
    input.and_then(InputCaps::max_area).map(|(w, h)| Area::new(to_mils(w), to_mils(h)))
}

/// A scanner's details from what it reported.
fn scanner(entry: &Entry, caps: &Capabilities, status: Option<&Status>) -> Scanner {
    let url = entry.url.as_url();
    let name = caps.make_and_model.clone().or_else(|| entry.name.clone()).unwrap_or_else(|| url.clone());
    let handling = status.map(classify::handling_status).unwrap_or_default();
    let platen = caps.platen.as_ref();
    let feeder = caps.feeder(false);
    let mut scanner = Scanner::new(url.clone(), name);
    scanner.flatbed = platen.is_some();
    scanner.feeder = feeder.is_some();
    scanner.duplex = caps.duplex();
    scanner.reported.units = true;
    scanner.feeder_ready = handling.feeder_ready;
    scanner.handling_status = handling;
    scanner.feeder_capacity = caps.feeder_capacity;
    scanner.connection = Connection::Other;
    scanner.port = Some(url);
    let optical = platen.or(feeder);
    scanner.optical_x = optical.and_then(|i| i.max_optical_x);
    scanner.optical_y = optical.and_then(|i| i.max_optical_y.or(i.max_optical_x));
    scanner.bed = area(platen);
    scanner.sheet = area(feeder);
    scanner.glass = platen.map(|p| choices(p, caps));
    scanner.feeder_choices = feeder.map(|f| choices(f, caps));
    scanner
}

/// The source a request means: `Auto` is the feeder only when the scanner says paper is loaded,
/// otherwise the glass when there is one.
fn resolve_source(requested: Source, caps: &Capabilities, handling: Option<HandlingStatus>) -> Result<Source, ScanError> {
    let (glass, feeder) = (caps.platen.is_some(), caps.feeder(false).is_some());
    match requested {
        Source::Auto if feeder && handling.and_then(|h| h.feeder_ready) == Some(true) => Ok(Source::Feeder),
        Source::Auto if glass => Ok(Source::Glass),
        Source::Auto if feeder => Ok(Source::Feeder),
        Source::Auto => Err(ScanError::Failed("the scanner publishes no input source".into())),
        other => Ok(other),
    }
}

/// The page for a delivered document: the scan core's reading of it (black and white enforced), and
/// `pdf` for a PDF, which the core does not name.
fn page(bytes: &[u8], rule: bw::PageRule) -> Page {
    let (mut page, _) = bw::page_for(bytes, rule);
    if bytes.starts_with(b"%PDF-") {
        page.format = "pdf";
    }
    page
}

/// How a transfer ended.
enum End {
    /// The scanner said it had no more documents (404).
    Done,
    /// Every page asked for arrived.
    Counted,
    /// The caller cancelled.
    Cancelled,
    /// A request failed.
    Failed(Error),
}

impl EsclBackend {
    /// Sleep `wait`, in slices, returning early when the request is cancelled.
    fn pause(&self, wait: Duration, settings: &ScanSettings) {
        let until = Instant::now() + wait;
        while !settings.cancel.as_ref().is_some_and(scan::CancelToken::is_requested) {
            let now = Instant::now();
            if now >= until {
                return;
            }
            std::thread::sleep((until - now).min(Duration::from_millis(100)));
        }
    }

    /// The next document, waiting through 503s for at most the busy wait; `None` when the job has no more.
    fn next_document<T: Transport>(&self, client: &Client<T>, job: &Job, settings: &ScanSettings) -> Result<Option<Vec<u8>>, Error> {
        let deadline = Instant::now() + self.config.busy_wait;
        loop {
            match client.next_document(job)? {
                NextDocument::Document { bytes, .. } => return Ok(Some(bytes)),
                NextDocument::Done => return Ok(None),
                NextDocument::NotReady { retry_after } => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(Error::Http { status: 503, retry_after });
                    }
                    self.pause(retry_after.unwrap_or(self.config.poll).min(left), settings);
                    if settings.cancel.as_ref().is_some_and(scan::CancelToken::is_requested) {
                        return Err(Error::Cancelled);
                    }
                }
            }
        }
    }

    /// Pull documents until the scanner has no more, `wanted` arrived, the caller cancels or a request fails.
    fn transfer<T: Transport>(&self, client: &Client<T>, job: &Job, settings: &ScanSettings, wanted: usize, pages: &mut Vec<Page>) -> End {
        let rule = bw::PageRule::for_settings(settings);
        while pages.len() < wanted {
            if settings.cancel.as_ref().is_some_and(scan::CancelToken::is_requested) {
                return End::Cancelled;
            }
            match self.next_document(client, job, settings) {
                Ok(Some(bytes)) => {
                    let page = page(&bytes, rule);
                    if let Some(progress) = &settings.progress {
                        progress.push_page(page.clone());
                    }
                    pages.push(page);
                }
                Ok(None) => return End::Done,
                Err(Error::Cancelled) => return End::Cancelled,
                Err(e) => return End::Failed(e),
            }
        }
        End::Counted
    }

    /// Why `job` ended early, from a fresh status read: the job's own state when the status lists it,
    /// otherwise a feeder state that stops paper (a jam, an open hatch, a double feed). An empty
    /// feeder is not a cause here: it is how every feeder job ends.
    fn ended_by_scanner(&self, entry: &Entry, job: &Job) -> Option<ScanError> {
        let status = self.client(entry).status().ok()?;
        let adf = status.adf_state.as_ref().filter(|a| **a != AdfState::Empty);
        if let Some(info) = status.job(job.url.path()) {
            return classify::job_error(info, adf);
        }
        adf.and_then(|a| classify::adf_error(a, "NextDocument")).filter(|e| e.interrupt_kind() != InterruptKind::Attention)
    }

    /// A refused `POST ScanJobs`, explained from a fresh status read when the scanner answered 409
    /// or 503 for a feeder scan and its feeder state names a cause (an empty feeder, a jam).
    fn refused(&self, entry: &Entry, error: &Error, source: Source) -> ScanError {
        if source == Source::Feeder
            && matches!(error.http_status(), Some(409 | 503))
            && let Ok(status) = self.client(entry).status()
            && let Some(e) = status.adf_state.as_ref().and_then(|a| classify::adf_error(a, "ScanJobs"))
        {
            return e;
        }
        classify::scan_error("ScanJobs", error)
    }

    fn acquire_from(&self, entry: &Entry, settings: &ScanSettings) -> Result<Acquired, ScanError> {
        let wire_cancel = CancelToken::new();
        let client = self.client(entry).with_cancel(wire_cancel.clone());
        let caps = client.capabilities().map_err(|e| classify::scan_error("ScannerCapabilities", &e))?;
        // A scanner whose status cannot be read is not preflighted, which is not the same as all clear.
        let handling = client.status().ok().as_ref().map(classify::handling_status);
        let source = resolve_source(settings.source, &caps, handling)?;
        scan::preflight(source, handling)?;
        if let Some(progress) = &settings.progress {
            progress.set_source(source);
        }
        let request = settings::plan(settings, &caps, source)?;
        let job = client.create_job(&request).map_err(|e| self.refused(entry, &e, source))?;
        if let Some(token) = &settings.cancel {
            let wire = wire_cancel.clone();
            token.set_canceller(Arc::new(move || wire.cancel()));
        }
        let sides = if request.duplex { 2 } else { 1 };
        let wanted = match source {
            Source::Glass => 1,
            _ if settings.pages == 0 => scan::MAX_PAGES * sides,
            _ => settings.pages.min(scan::MAX_PAGES) * sides,
        };
        let mut pages = Vec::new();
        let end = self.transfer(&client, &job, settings, usize::try_from(wanted).unwrap_or(usize::MAX), &mut pages);
        if let Some(token) = &settings.cancel {
            token.clear_canceller();
        }
        let ended = match &end {
            End::Failed(_) => self.ended_by_scanner(entry, &job),
            End::Done if source == Source::Feeder => self.ended_by_scanner(entry, &job),
            _ => None,
        };
        // A glass job ends with its one page and a feeder job with the 404; any other stop leaves
        // the job open, so it is ended (best effort, on a client without the caller's cancel token).
        let finished = matches!(end, End::Done) || (matches!(end, End::Counted) && source == Source::Glass) || ended.is_some();
        if !finished {
            let _ = self.client(entry).cancel(&job);
        }
        let cancelled = matches!(end, End::Cancelled);
        let download_error = match end {
            End::Failed(e) => Some(ended.unwrap_or_else(|| classify::scan_error("NextDocument", &e))),
            End::Done if ended.is_some() => ended,
            End::Done if pages.is_empty() && source == Source::Feeder => Some(ScanError::native_error(
                InterruptKind::FeederEmpty,
                NativeCode::new(HTTP_DOMAIN, 404, Some("Not Found")),
                format!("{} (the scanner had no document for the job)", InterruptKind::FeederEmpty.sentence()),
            )),
            End::Done if pages.is_empty() => Some(ScanError::Failed("the scanner ended the job without a page".into())),
            End::Done | End::Counted | End::Cancelled => None,
        };
        // Two-sided scans count sides, not sheets, so the page-count check is skipped there.
        let checked = if request.duplex { 0 } else { usize::try_from(settings.pages).unwrap_or(0) };
        if let Err(error) = scan::after_transfer_checked(checked, source == Source::Feeder, pages.len(), cancelled, download_error) {
            if pages.is_empty() {
                return Err(error);
            }
            return Err(ScanError::Incomplete { message: error.to_string(), cause: error.cause(), acquired: Acquired::new(source, pages) });
        }
        Ok(Acquired::new(source, pages))
    }
}

impl Backend for EsclBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn scanner_list(&self) -> Result<Vec<ScannerRef>, ScanError> {
        Ok(self
            .snapshot()
            .into_iter()
            .map(|e| {
                let url = e.url.as_url();
                let mut r = ScannerRef::new(url.clone(), e.name.unwrap_or_else(|| url.clone()), Some(url));
                r.connection = Connection::Other;
                r
            })
            .collect())
    }

    fn scanner_details(&self, native_id: &str) -> Result<Scanner, ScanError> {
        self.details(&self.entry(native_id)?)
    }

    fn scanners(&self) -> Result<Vec<Scanner>, ScanError> {
        Ok(self.snapshot().iter().filter_map(|e| self.details(e).ok()).collect())
    }

    fn status(&self, native_id: &str) -> Option<HandlingStatus> {
        let entry = self.entry(native_id).ok()?;
        self.client(&entry).status().ok().map(|s| classify::handling_status(&s))
    }

    /// The `ScannerCapabilities` document of each scanner, as it sent it, and its status as read.
    fn diagnostic_dump(&self, filter: &str) -> Result<String, ScanError> {
        let needle = filter.to_lowercase();
        let mut out = String::new();
        for e in self.snapshot() {
            let url = e.url.as_url();
            if !needle.is_empty() && !url.to_lowercase().contains(&needle) && !e.name.as_deref().is_some_and(|n| n.to_lowercase().contains(&needle)) {
                continue;
            }
            out.push_str(&format!("== {url}\n"));
            let client = self.client(&e);
            match client.capabilities_document() {
                Ok(doc) => out.push_str(&format!("{}\n", String::from_utf8_lossy(&doc).trim_end())),
                Err(error) => out.push_str(&format!("capabilities error: {error}\n")),
            }
            match client.status() {
                Ok(status) => out.push_str(&format!("{status:#?}\n")),
                Err(error) => out.push_str(&format!("status error: {error}\n")),
            }
        }
        Ok(out)
    }

    fn acquire(&self, settings: &ScanSettings) -> Result<Acquired, ScanError> {
        let entry = self.entry(&settings.device)?;
        self.acquire_from(&entry, settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_is_the_feeder_only_when_paper_is_loaded() {
        let both = Capabilities { platen: Some(InputCaps::default()), adf_simplex: Some(InputCaps::default()), ..Capabilities::default() };
        let loaded = Some(HandlingStatus::default().with_feeder_ready(Some(true)));
        assert_eq!(resolve_source(Source::Auto, &both, loaded).unwrap(), Source::Feeder);
        assert_eq!(resolve_source(Source::Auto, &both, None).unwrap(), Source::Glass);
        let feeder_only = Capabilities { adf_simplex: Some(InputCaps::default()), ..Capabilities::default() };
        assert_eq!(resolve_source(Source::Auto, &feeder_only, None).unwrap(), Source::Feeder);
        assert!(resolve_source(Source::Auto, &Capabilities::default(), None).is_err());
        assert_eq!(resolve_source(Source::Glass, &feeder_only, None).unwrap(), Source::Glass);
    }

    #[test]
    fn a_pdf_page_is_named_pdf() {
        assert_eq!(page(b"%PDF-1.7 ...", bw::PageRule::for_settings(&ScanSettings::default())).format, "pdf");
    }
}
