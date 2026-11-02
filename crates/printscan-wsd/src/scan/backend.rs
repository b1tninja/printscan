//! [`WsdScanBackend`]: the WS-Scan client as a [`printscan::scan::Backend`].

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

use printscan::scan::{
    self, Acquired, Backend, Connection, HandlingStatus, ImageChoices, InterruptKind, Page, ScanError, ScanSettings, Scanner, ScannerRef, Source,
    ValueSpan, bw,
};

use super::client::{self, Client, Section};
use super::fault;
use super::model::{ColorMode, Configuration, CreatedJob, Input, ScannerElements};
use super::ticket::{self, Ticket};
use crate::{CancelToken, DeviceUrl, Error, NetworkLimits, Transport};

/// The registry name and id prefix.
const NAME: &str = "wsd";

/// A transport the backend can hold and share between threads.
type SharedTransport = Arc<dyn Transport + Send + Sync>;

#[derive(Clone, Debug)]
struct Entry {
    url: DeviceUrl,
    name: Option<String>,
}

#[derive(Clone, Debug)]
struct Config {
    user_name: String,
    job_name: String,
    limits: NetworkLimits,
    image_limits: NetworkLimits,
}

impl Default for Config {
    fn default() -> Self {
        Self { user_name: "printscan".into(), job_name: "printscan".into(), limits: NetworkLimits::default(), image_limits: client::image_limits() }
    }
}

/// A WS-Scan backend. Registers as `wsd`; its scanners are the `ScannerService` URLs added to it,
/// and their native ids are those URLs. It discovers nothing and opens no connection until it is
/// asked for a scanner's details, its status or a scan.
///
/// Clones share the scanner list: register one clone and keep the other to [`add`](Self::add) or
/// [`remove`](Self::remove) scanners later.
#[derive(Clone)]
pub struct WsdScanBackend {
    scanners: Arc<RwLock<Vec<Entry>>>,
    transport: SharedTransport,
    config: Arc<Config>,
}

impl fmt::Debug for WsdScanBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WsdScanBackend").field("scanners", &self.urls()).field("config", &self.config).finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for WsdScanBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// The scan core's error for a URL the policy refuses.
fn refused(error: &Error) -> ScanError {
    ScanError::Failed(error.to_string())
}

impl WsdScanBackend {
    /// A backend with no scanners and the plain-HTTP transport. Opens no connection.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Self {
        Self::with_transport(Arc::new(crate::http::HttpTransport::new()))
    }

    /// A backend that sends through `transport` (a test double, a recording proxy).
    pub fn with_transport(transport: SharedTransport) -> Self {
        Self { scanners: Arc::default(), transport, config: Arc::default() }
    }

    /// Apply `change` to a copy of the configuration; clones made earlier keep the old one.
    fn configured(mut self, change: impl FnOnce(&mut Config)) -> Self {
        let mut config = (*self.config).clone();
        change(&mut config);
        self.config = Arc::new(config);
        self
    }

    /// The `JobOriginatingUserName` of every job (default `printscan`).
    #[must_use]
    pub fn with_user_name(self, name: &str) -> Self {
        self.configured(|c| c.user_name = name.to_owned())
    }

    /// The `JobName` of every job (default `printscan`).
    #[must_use]
    pub fn with_job_name(self, name: &str) -> Self {
        self.configured(|c| c.job_name = name.to_owned())
    }

    /// Limits for every operation but `RetrieveImage`.
    #[must_use]
    pub fn with_limits(self, limits: NetworkLimits) -> Self {
        self.configured(|c| c.limits = limits)
    }

    /// Limits for `RetrieveImage` (default [`client::image_limits`]).
    #[must_use]
    pub fn with_image_limits(self, limits: NetworkLimits) -> Self {
        self.configured(|c| c.image_limits = limits)
    }

    /// Add a scanner by its `ScannerService` URL (`http://<numeric local address>[:port]/path`) and
    /// return its native id, the canonical URL. Adding one twice keeps one entry. Opens no connection.
    pub fn add(&self, url: &str) -> Result<String, ScanError> {
        self.add_entry(url, None)
    }

    /// [`Self::add`] with the name a person sees until the scanner reports its own.
    pub fn add_named(&self, url: &str, name: &str) -> Result<String, ScanError> {
        self.add_entry(url, Some(name.to_owned()))
    }

    fn add_entry(&self, url: &str, name: Option<String>) -> Result<String, ScanError> {
        let url = DeviceUrl::parse_as(url, "scanner").map_err(|e| refused(&e))?;
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
        let Ok(url) = DeviceUrl::parse(url) else { return false };
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
            return all.into_iter().next().ok_or_else(|| ScanError::Failed("no WS-Scan scanner has been added".into()));
        }
        if let Ok(url) = DeviceUrl::parse(device)
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
            (Some(_), Some(_)) => Err(ScanError::Failed(format!("{device:?} names more than one WS-Scan scanner"))),
            (None, _) => Err(ScanError::Failed(format!("no WS-Scan scanner {device:?} has been added"))),
        }
    }

    fn client(&self, entry: &Entry) -> Client<SharedTransport> {
        Client::new(Arc::clone(&self.transport), entry.url.clone()).with_limits(self.config.limits).with_image_limits(self.config.image_limits)
    }

    fn elements(&self, entry: &Entry, sections: &[Section]) -> Result<ScannerElements, ScanError> {
        self.client(entry).get_scanner_elements(sections).map_err(|e| fault::scan_error("GetScannerElements", &e))
    }
}

/// What one input offers, in the scan core's terms.
fn choices(input: &Input, config: &Configuration) -> ImageChoices {
    let colors = input
        .colors
        .iter()
        .filter_map(|c| match c {
            ColorMode::Rgb24 => Some(scan::Color::Rgb),
            ColorMode::Grayscale8 => Some(scan::Color::Gray),
            ColorMode::BlackAndWhite1 => Some(scan::Color::BlackAndWhite),
            ColorMode::Other(_) => None,
        })
        .collect();
    let span = |on: bool| on.then_some(ValueSpan { min: -1000, max: 1000, step: 1 });
    ImageChoices {
        dpi: input.widths.clone(),
        dpi_y: input.heights.clone(),
        colors,
        brightness: span(config.brightness),
        contrast: span(config.contrast),
        formats: config.formats.clone(),
        ..ImageChoices::default()
    }
}

/// A scanner's details from what it reported.
fn scanner(entry: &Entry, elements: &ScannerElements) -> Scanner {
    let config = elements.configuration.clone().unwrap_or_default();
    let status = elements.status.as_ref().map(fault::handling_status).unwrap_or_default();
    let url = entry.url.as_url();
    let name = elements.description.as_ref().and_then(|d| d.name.clone()).or_else(|| entry.name.clone()).unwrap_or_else(|| url.clone());
    let feeder = config.feeder.as_ref();
    let platen = config.platen.as_ref();
    let optical = platen.or(feeder.map(|f| &f.front)).and_then(|i| i.optical);
    Scanner {
        id: url.clone(),
        name,
        feeder: feeder.is_some(),
        flatbed: platen.is_some(),
        duplex: feeder.is_some_and(|f| f.duplex),
        feeder_ready: status.feeder_ready,
        handling_status: status,
        feeder_capacity: None,
        connection: Connection::Wsd,
        port: Some(url),
        optical_x: optical.map(|o| o.0),
        optical_y: optical.map(|o| o.1),
        pages: None,
        bed: platen.and_then(|p| p.maximum),
        sheet: feeder.and_then(|f| f.front.maximum),
        glass: platen.map(|p| choices(p, &config)),
        feeder_choices: feeder.map(|f| choices(&f.front, &config)),
        reported: scan::Reported::default(),
    }
}

/// The source a request means: `Auto` is the glass when there is one (WS-Scan never says that
/// paper is loaded), otherwise the feeder.
fn resolve_source(requested: Source, config: &Configuration) -> Source {
    match requested {
        Source::Auto if config.platen.is_some() => Source::Glass,
        Source::Auto => Source::Feeder,
        other => other,
    }
}

/// A DIB as a BMP file: some scanners send `dib` without the 14-byte file header, which every
/// reader of the page needs. Anything else is returned as it came.
fn as_file(bytes: Vec<u8>) -> Vec<u8> {
    let le32 = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let header = match le32(0) {
        Some(h @ (40 | 108 | 124)) if !bytes.starts_with(b"BM") => h,
        _ => return bytes,
    };
    let bits = bytes.get(14..16).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
    let compression = le32(16).unwrap_or(0);
    let used = le32(32).unwrap_or(0);
    let palette = if used > 0 {
        used
    } else if bits <= 8 {
        1 << bits
    } else {
        0
    };
    // BI_BITFIELDS with the plain header keeps its three masks after it.
    let masks = if header == 40 && compression == 3 { 12 } else { 0 };
    let offset = 14 + header + palette * 4 + masks;
    let size = u32::try_from(bytes.len() + 14).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(bytes.len() + 14);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&bytes);
    out
}

/// Create the job, falling back through other formats when the scanner refuses the one asked
/// for and the request left the format open.
fn create_job<T: Transport>(client: &Client<T>, mut ticket: Ticket, may_substitute: bool) -> Result<(CreatedJob, Ticket), ScanError> {
    let mut tried = Vec::new();
    loop {
        match client.create_scan_job(&ticket) {
            Ok(job) => return Ok((job, ticket)),
            Err(e) if may_substitute && fault::is_format_refused(&e) => {
                tried.push(ticket.format.clone());
                let Some(next) = ticket::fallbacks(&ticket.color, &tried).first().copied() else {
                    return Err(fault::scan_error("CreateScanJob", &e));
                };
                ticket = ticket.with_format(next);
            }
            Err(e) => return Err(fault::scan_error("CreateScanJob", &e)),
        }
    }
}

/// How a transfer ended.
enum End {
    /// The scanner said it had no more images (the fault that says so).
    Exhausted(Error),
    /// Every image asked for arrived.
    Counted,
    /// The caller cancelled.
    Cancelled,
    /// An operation failed.
    Failed(Error),
}

impl WsdScanBackend {
    /// Pull images until the scanner has no more, `wanted` arrived, the caller cancels or an
    /// operation fails.
    fn transfer<T: Transport>(&self, client: &Client<T>, job: &CreatedJob, settings: &ScanSettings, wanted: usize, pages: &mut Vec<Page>) -> End {
        let rule = bw::PageRule::for_settings(settings);
        while pages.len() < wanted {
            if settings.cancel.as_ref().is_some_and(scan::CancelToken::is_requested) {
                return End::Cancelled;
            }
            if let (Some(progress), Some((_, lines, stride))) = (&settings.progress, job.front)
                && stride > 0
            {
                progress.set_expected_bytes(Some(u64::from(lines) * u64::from(stride)));
            }
            match client.retrieve_image(job, &format!("page-{}", pages.len() + 1)) {
                Ok(bytes) => {
                    let (page, _) = bw::page_for(&as_file(bytes), rule);
                    if let Some(progress) = &settings.progress {
                        progress.push_page(page.clone());
                    }
                    pages.push(page);
                }
                Err(e) if fault::is(&e, "ClientErrorNoImagesAvailable") => return End::Exhausted(e),
                Err(Error::Cancelled) => return End::Cancelled,
                Err(e) => return End::Failed(e),
            }
        }
        End::Counted
    }

    /// The scanner says `job` was cancelled at the device: its status gives `JobCanceledAtDevice`,
    /// or it answers that the job was cancelled. Anything else, a failed read included, is no.
    fn cancelled_at_device(&self, entry: &Entry, job: &CreatedJob) -> bool {
        match self.client(entry).get_job_status(job) {
            Ok(status) => status.is_some_and(|s| s.cancelled_at_device()),
            Err(e) => fault::is(&e, "ClientErrorJobCancelled"),
        }
    }

    /// A failure with no named cause, restated from a fresh status read when the status names one.
    fn explain(&self, entry: &Entry, error: ScanError) -> ScanError {
        if !matches!(error.interrupt_kind(), InterruptKind::Other | InterruptKind::Attention) {
            return error;
        }
        let status = self.elements(entry, &[Section::Status]).ok().and_then(|e| e.status);
        match status.map(|s| fault::handling_status(&s)).and_then(HandlingStatus::acquire_block_kind) {
            Some(kind) => error.restated(kind, kind.sentence()),
            None => error,
        }
    }

    fn acquire_from(&self, entry: &Entry, settings: &ScanSettings) -> Result<Acquired, ScanError> {
        let wire_cancel = CancelToken::new();
        let client = self.client(entry).with_cancel(wire_cancel.clone());
        let elements =
            client.get_scanner_elements(&[Section::Configuration, Section::Status]).map_err(|e| fault::scan_error("GetScannerElements", &e))?;
        let config = elements.configuration.ok_or_else(|| ScanError::Failed("the scanner sent no configuration".into()))?;
        let source = resolve_source(settings.source, &config);
        scan::preflight(source, elements.status.as_ref().map(fault::handling_status))?;
        if let Some(progress) = &settings.progress {
            progress.set_source(source);
        }
        let planned = ticket::plan(settings, &config, source, &self.config.job_name, &self.config.user_name)?;
        let (job, ticket) = create_job(&client, planned, settings.format.is_none())?;
        if let Some(token) = &settings.cancel {
            let wire = wire_cancel.clone();
            token.set_canceller(Arc::new(move || wire.cancel()));
        }
        let sides = if settings.duplex { 2 } else { 1 };
        let wanted = if ticket.images == 0 { scan::MAX_PAGES * sides } else { ticket.images };
        let mut pages = Vec::new();
        let end = self.transfer(&client, &job, settings, usize::try_from(wanted).unwrap_or(usize::MAX), &mut pages);
        if let Some(token) = &settings.cancel {
            token.clear_canceller();
        }
        // A reply cut off without a fault may be the scanner's own cancel (its panel), which a
        // scanner shows in the job's status; asked before our CancelJob, which would look the same.
        let at_device = matches!(&end, End::Failed(e) if e.fault().is_none()) && self.cancelled_at_device(entry, &job);
        // The scanner ends a job by itself once it has no more images or sent the count it was
        // asked for; any other stop leaves the job open, so it is cancelled (best effort, on a
        // client without the caller's cancel token).
        let finished = matches!(end, End::Exhausted(_)) || (matches!(end, End::Counted) && ticket.images > 0);
        if !finished && !at_device {
            let _ = self.client(entry).cancel_job(job.id);
        }
        let cancelled = matches!(end, End::Cancelled);
        let download_error = match end {
            End::Failed(e) if at_device => {
                Some(fault::scan_error("RetrieveImage", &e).restated(InterruptKind::Cancelled, InterruptKind::Cancelled.sentence()))
            }
            End::Failed(e) => Some(self.explain(entry, fault::scan_error("RetrieveImage", &e))),
            // No image at all from the feeder: it was empty.
            End::Exhausted(e) if pages.is_empty() && source == Source::Feeder => Some(fault::scan_error("RetrieveImage", &e)),
            End::Exhausted(_) | End::Counted | End::Cancelled => None,
        };
        // Two-sided scans count sides, not sheets, so the page-count check is skipped there.
        let checked = if settings.duplex { 0 } else { usize::try_from(settings.pages).unwrap_or(0) };
        if let Err(error) = scan::after_transfer_checked(checked, source == Source::Feeder, pages.len(), cancelled, download_error) {
            if pages.is_empty() {
                return Err(error);
            }
            return Err(ScanError::Incomplete { message: error.to_string(), cause: error.cause(), acquired: Acquired { source, pages } });
        }
        Ok(Acquired { source, pages })
    }
}

impl Backend for WsdScanBackend {
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
                r.connection = Connection::Wsd;
                r
            })
            .collect())
    }

    fn scanner_details(&self, native_id: &str) -> Result<Scanner, ScanError> {
        let entry = self.entry(native_id)?;
        let elements = self.elements(&entry, &[Section::Description, Section::Configuration, Section::Status])?;
        Ok(scanner(&entry, &elements))
    }

    fn scanners(&self) -> Result<Vec<Scanner>, ScanError> {
        Ok(self
            .snapshot()
            .iter()
            .filter_map(|e| self.elements(e, &[Section::Description, Section::Configuration, Section::Status]).ok().map(|el| scanner(e, &el)))
            .collect())
    }

    fn status(&self, native_id: &str) -> Option<HandlingStatus> {
        let entry = self.entry(native_id).ok()?;
        self.elements(&entry, &[Section::Status]).ok()?.status.map(|s| fault::handling_status(&s))
    }

    fn diagnostic_dump(&self, filter: &str) -> Result<String, ScanError> {
        let needle = filter.to_lowercase();
        let mut out = String::new();
        for e in self.snapshot() {
            let url = e.url.as_url();
            if !needle.is_empty() && !url.to_lowercase().contains(&needle) && !e.name.as_deref().is_some_and(|n| n.to_lowercase().contains(&needle)) {
                continue;
            }
            out.push_str(&format!("== {url}\n"));
            match self.elements(&e, &[Section::Description, Section::Configuration, Section::Status]) {
                Ok(elements) => out.push_str(&format!("{elements:#?}\n")),
                Err(error) => out.push_str(&format!("error: {error}\n")),
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
    fn a_headerless_dib_gets_a_file_header() {
        let mut dib = vec![0u8; 40];
        dib[..4].copy_from_slice(&40u32.to_le_bytes());
        dib[14..16].copy_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[1, 2, 3, 0]);
        let file = as_file(dib.clone());
        assert_eq!(&file[..2], b"BM");
        assert_eq!(u32::from_le_bytes(file[10..14].try_into().unwrap()), 54);
        assert_eq!(&file[14..], &dib[..]);
        // Grey: a 256-entry palette sits between the header and the pixels.
        dib[14..16].copy_from_slice(&8u16.to_le_bytes());
        assert_eq!(u32::from_le_bytes(as_file(dib).get(10..14).unwrap().try_into().unwrap()), 54 + 1024);
        // A file, a PNG and junk are left alone.
        for other in [b"BM\x28\0\0\0".to_vec(), b"\x89PNG\r\n".to_vec(), vec![1, 2]] {
            assert_eq!(as_file(other.clone()), other);
        }
    }
}
