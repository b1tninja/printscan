//! [`SaneBackend`]: the SANE network client as a [`printscan::scan::Backend`].

use std::fmt;
use std::net::Shutdown;
use std::sync::{Arc, PoisonError, RwLock};

use printscan::scan::{
    self, Acquired, Backend, Connection, Page, ScanError, ScanSettings, Scanner, ScannerRef, Source, auto_fallback_to_glass, bw, pages_in_error,
};

use super::frame::{self, RawFrame};
use super::options::{self, Sources};
use super::status::scan_error;
use crate::client::{Client, ClientOptions, Handle, Timeouts};
use crate::error::Error;
use crate::policy::SaneHost;
use crate::protocol::{ByteOrder, OptionTable, Status, Value};
use crate::wire::Limits;

/// The registry name and id prefix.
pub const NAME: &str = "sane";

/// What the app added: a whole daemon, or one device on it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Added {
    host: SaneHost,
    device: Option<String>,
    name: Option<String>,
}

/// One device to talk to.
#[derive(Clone, Debug)]
struct Target {
    host: SaneHost,
    device: String,
    /// What a person sees: `vendor model`, the name the app gave, or the device name.
    name: String,
    /// The device type the daemon reported (`flatbed scanner`), empty when not listed.
    kind: String,
}

impl Target {
    fn id(&self) -> String {
        native_id(self.host, &self.device)
    }
}

/// `<host>:<port>/<device>`, the native id of a device.
fn native_id(host: SaneHost, device: &str) -> String {
    format!("{host}/{device}")
}

/// The host and device of a native id. The host has no `/`, so the first one separates them.
fn parse_id(id: &str) -> Option<(SaneHost, &str)> {
    let (host, device) = id.split_once('/')?;
    let host = SaneHost::parse(host).ok()?;
    (!device.is_empty()).then_some((host, device))
}

/// A SANE network backend. Registers as `sane`; its devices are the ones the app added, directly
/// ([`Self::add_device`]) or as the devices of a daemon it added ([`Self::add_host`]). Native ids
/// are `<address>:<port>/<device name>` (`192.168.1.20:6566/pixma:04A91234_12AB34`), so a registry
/// id reads `sane:192.168.1.20:6566/pixma:...`.
///
/// Building a backend and adding to it open no connection. Listing asks each added daemon for its
/// devices (one connection each); details, a dump and a scan open the device.
///
/// Clones share what was added: register one clone and keep the other to add or remove later.
#[derive(Clone)]
pub struct SaneBackend {
    added: Arc<RwLock<Vec<Added>>>,
    options: Arc<ClientOptions>,
}

impl fmt::Debug for SaneBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SaneBackend").field("added", &self.added()).field("options", &self.options).finish()
    }
}

impl Default for SaneBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// The scan core's error for an address the policy refuses.
fn refused(error: &Error) -> ScanError {
    ScanError::Failed(error.to_string())
}

impl SaneBackend {
    /// A backend with nothing added. Opens no connection.
    pub fn new() -> Self {
        Self { added: Arc::default(), options: Arc::default() }
    }

    /// Apply `change` to a copy of the options; clones made earlier keep the old ones.
    fn configured(mut self, change: impl FnOnce(&mut ClientOptions)) -> Self {
        let mut options = (*self.options).clone();
        change(&mut options);
        self.options = Arc::new(options);
        self
    }

    /// The user name sent with init (default `printscan`). No password is ever sent.
    #[must_use]
    pub fn with_user_name(self, name: &str) -> Self {
        self.configured(|o| o.user_name = name.to_owned())
    }

    /// How long connections, replies and image data are waited for.
    #[must_use]
    pub fn with_timeouts(self, timeouts: Timeouts) -> Self {
        self.configured(|o| o.timeouts = timeouts)
    }

    /// Bounds on replies and on the image data of one frame.
    #[must_use]
    pub fn with_limits(self, limits: Limits) -> Self {
        self.configured(|o| o.limits = limits)
    }

    fn push(&self, entry: Added) {
        let mut added = self.added.write().unwrap_or_else(PoisonError::into_inner);
        match added.iter_mut().find(|a| a.host == entry.host && a.device == entry.device) {
            Some(existing) if entry.name.is_some() => existing.name = entry.name,
            Some(_) => {}
            None => added.push(entry),
        }
    }

    /// Add a daemon (`192.168.1.20`, `192.168.1.20:6566`, `[fd00::20]:6566`; port 6566 when none is
    /// given) and return its canonical address. Its devices are listed by asking it, when the app
    /// lists scanners. Adding one twice keeps one entry. Opens no connection.
    pub fn add_host(&self, host: &str) -> Result<String, ScanError> {
        let host = SaneHost::parse(host).map_err(|e| refused(&e))?;
        self.push(Added { host, device: None, name: None });
        Ok(host.to_string())
    }

    /// Add one device of a daemon by its SANE name (`pixma:04A91234_12AB34`, `test:0`) and return
    /// its native id. It is listed without asking the daemon. Opens no connection.
    pub fn add_device(&self, host: &str, device: &str) -> Result<String, ScanError> {
        self.add_device_entry(host, device, None)
    }

    /// [`Self::add_device`] with the name a person sees.
    pub fn add_device_named(&self, host: &str, device: &str, name: &str) -> Result<String, ScanError> {
        self.add_device_entry(host, device, Some(name.to_owned()))
    }

    fn add_device_entry(&self, host: &str, device: &str, name: Option<String>) -> Result<String, ScanError> {
        let host = SaneHost::parse(host).map_err(|e| refused(&e))?;
        if device.is_empty() || device.contains('\0') {
            return Err(ScanError::Failed(format!("{device:?} is not a SANE device name")));
        }
        self.push(Added { host, device: Some(device.to_owned()), name });
        Ok(native_id(host, device))
    }

    /// Remove a daemon (by its address) or a device (by its native id); `false` when it was not added.
    pub fn remove(&self, host_or_id: &str) -> bool {
        let (host, device) = match parse_id(host_or_id) {
            Some((host, device)) => (host, Some(device.to_owned())),
            None => match SaneHost::parse(host_or_id) {
                Ok(host) => (host, None),
                Err(_) => return false,
            },
        };
        let mut added = self.added.write().unwrap_or_else(PoisonError::into_inner);
        let before = added.len();
        added.retain(|a| !(a.host == host && a.device == device));
        added.len() != before
    }

    /// What was added: daemon addresses and device native ids.
    pub fn added(&self) -> Vec<String> {
        self.snapshot().iter().map(|a| a.device.as_deref().map_or_else(|| a.host.to_string(), |d| native_id(a.host, d))).collect()
    }

    fn snapshot(&self) -> Vec<Added> {
        self.added.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn connect(&self, host: SaneHost) -> Result<Client, ScanError> {
        Client::connect(host, &self.options).map_err(|e| scan_error(&format!("connect to the SANE daemon at {host}"), &e))
    }

    /// The devices of every added daemon and every added device. A daemon that fails is left out;
    /// when nothing at all is found, its error is the answer.
    fn targets(&self) -> Result<Vec<Target>, ScanError> {
        let mut found: Vec<Target> = Vec::new();
        let mut first_error = None;
        for entry in self.snapshot() {
            let listed = match &entry.device {
                Some(device) => {
                    let name = entry.name.clone().unwrap_or_else(|| device.clone());
                    vec![Target { host: entry.host, device: device.clone(), name, kind: String::new() }]
                }
                None => match self.host_devices(entry.host) {
                    Ok(listed) => listed,
                    Err(error) => {
                        first_error.get_or_insert(error);
                        continue;
                    }
                },
            };
            for target in listed {
                if !found.iter().any(|t| t.id() == target.id()) {
                    found.push(target);
                }
            }
        }
        match first_error {
            Some(error) if found.is_empty() => Err(error),
            _ => Ok(found),
        }
    }

    /// One daemon's devices: init, get devices, exit.
    fn host_devices(&self, host: SaneHost) -> Result<Vec<Target>, ScanError> {
        let mut client = self.connect(host)?;
        let devices = client.devices().map_err(|e| scan_error(&format!("list the devices of {host}"), &e));
        client.exit();
        Ok(devices?.into_iter().map(|d| Target { host, name: d.display_name(), kind: d.kind.clone(), device: d.name }).collect())
    }

    /// The device `device` names: a native id on an added daemon, a fragment of a listed id or name,
    /// or (empty) the first listed.
    fn target(&self, device: &str) -> Result<Target, ScanError> {
        if let Some((host, name)) = parse_id(device) {
            let added = self.snapshot();
            if !added.iter().any(|a| a.host == host) {
                return Err(ScanError::Failed(format!("the SANE daemon {host} has not been added")));
            }
            // An added device keeps the name it was given; a daemon's listing knows the vendor, model and
            // type. An id neither has is still opened: the daemon decides whether it exists.
            if let Some(entry) = added.iter().find(|a| a.host == host && a.device.as_deref() == Some(name)) {
                let shown = entry.name.clone().unwrap_or_else(|| name.to_owned());
                return Ok(Target { host, device: name.to_owned(), name: shown, kind: String::new() });
            }
            let listed = self.host_devices(host).ok().and_then(|all| all.into_iter().find(|t| t.device == name));
            return Ok(listed.unwrap_or_else(|| Target { host, device: name.to_owned(), name: name.to_owned(), kind: String::new() }));
        }
        let all = self.targets()?;
        if device.is_empty() {
            return all.into_iter().next().ok_or_else(|| ScanError::Failed("no SANE device has been added or listed".into()));
        }
        let needle = device.to_lowercase();
        let mut found = all.into_iter().filter(|t| t.id().to_lowercase().contains(&needle) || t.name.to_lowercase().contains(&needle));
        match (found.next(), found.next()) {
            (Some(t), None) => Ok(t),
            (Some(_), Some(_)) => Err(ScanError::Failed(format!("{device:?} names more than one SANE device"))),
            (None, _) => Err(ScanError::Failed(format!("no SANE device {device:?} is listed"))),
        }
    }

    fn open(&self, target: &Target) -> Result<Session, ScanError> {
        let mut client = self.connect(target.host)?;
        match client.open(&target.device) {
            Ok(handle) => Ok(Session { client: Some(client), handle }),
            Err(e) => {
                let error = scan_error(&format!("open {} on {}", target.device, target.host), &e);
                client.exit();
                Err(error)
            }
        }
    }

    fn details(&self, target: &Target) -> Result<Scanner, ScanError> {
        let mut session = self.open(target)?;
        let table = session.descriptors()?;
        Ok(scanner(target, &table))
    }
}

/// An open device: closed and the connection ended when dropped.
struct Session {
    client: Option<Client>,
    handle: Handle,
}

impl Session {
    fn client(&mut self) -> Result<&mut Client, ScanError> {
        self.client.as_mut().ok_or_else(|| ScanError::Failed("the SANE session has ended".into()))
    }

    fn descriptors(&mut self) -> Result<OptionTable, ScanError> {
        let handle = self.handle;
        self.client()?.option_descriptors(handle).map_err(|e| scan_error("get option descriptors", &e))
    }

    /// Set the active option `name`, reading the descriptors again when the driver says they changed.
    fn set(&mut self, table: &mut OptionTable, name: &str, value: &Value) -> Result<(), ScanError> {
        let Some((index, desc)) = table.active(name) else {
            return Err(ScanError::Failed(format!("this scanner has no {name} option now")));
        };
        if !desc.is_settable() {
            return Err(ScanError::Failed(format!("the {name} option of this scanner cannot be set")));
        }
        let desc = desc.clone();
        let handle = self.handle;
        let reply = self.client()?.set_option(handle, index, &desc, value).map_err(|e| scan_error(&format!("set {name}"), &e))?;
        if reply.reload_options() {
            *table = self.descriptors()?;
        }
        Ok(())
    }

    /// The current value of the active word option `name`, as a whole number.
    fn number(&mut self, table: &OptionTable, name: &str) -> Option<i32> {
        let (index, desc) = table.active(name)?;
        let desc = desc.clone();
        let handle = self.handle;
        let value = self.client().ok()?.get_option(handle, index, &desc).ok()?;
        value.first_word().map(|w| options::as_number(&desc, w))
    }

    /// End the scan in progress (or reset the device after the last frame). Best effort.
    fn cancel(&mut self) {
        let handle = self.handle;
        if let Ok(client) = self.client() {
            let _ = client.cancel(handle);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(mut client) = self.client.take() {
            if !client.is_broken() {
                let _ = client.close(self.handle);
            }
            client.exit();
        }
    }
}

/// A device's details from its option descriptors. SANE has one set of options whatever the source,
/// so the glass and the feeder report the same choices.
fn scanner(target: &Target, table: &OptionTable) -> Scanner {
    let found = options::sources(table, &target.kind);
    let choices = options::choices(table);
    let bed = options::bed(table);
    let mut scanner = Scanner::new(target.id(), target.name.clone());
    scanner.feeder = found.feeder;
    scanner.flatbed = found.glass;
    scanner.duplex = found.duplex;
    // SANE cannot tell whether paper is loaded before a scan.
    scanner.feeder_ready = None;
    scanner.connection = Connection::Other;
    scanner.port = Some(target.host.to_string());
    scanner.bed = bed.filter(|_| found.glass);
    scanner.sheet = bed.filter(|_| found.feeder);
    scanner.glass = found.glass.then(|| choices.clone());
    scanner.feeder_choices = found.feeder.then_some(choices);
    scanner
}

/// How a transfer ended.
enum End {
    /// The feeder had no more documents.
    Exhausted(Error),
    /// Every page asked for arrived.
    Counted,
    /// The caller cancelled.
    Cancelled,
    /// Something failed.
    Failed(ScanError),
}

fn is_cancelled(settings: &ScanSettings) -> bool {
    settings.cancel.as_ref().is_some_and(scan::CancelToken::is_requested)
}

/// Write the request's options: source, mode, resolution, levels, then the area (whose ranges may
/// depend on the others). Returns the resolution the device took, both ways.
fn apply(session: &mut Session, table: &mut OptionTable, settings: &ScanSettings, source: Source) -> Result<(u32, u32), ScanError> {
    if let Some(entry) = options::source_value(table, source, settings.duplex)? {
        session.set(table, "source", &Value::String(entry))?;
    }
    if let Some(entry) = options::mode_value(table, settings.color)? {
        session.set(table, "mode", &Value::String(entry))?;
    }
    let x_name = ["resolution", "x-resolution"].into_iter().find(|n| table.active(n).is_some());
    if let Some(name) = x_name
        && let Some((_, desc)) = table.active(name)
    {
        let value = options::resolution_value(desc, settings.dpi)?;
        session.set(table, name, &value)?;
    }
    if let Some(dpi_y) = settings.dpi_y.filter(|&y| y != settings.dpi) {
        let Some((_, desc)) = table.active("y-resolution") else {
            return Err(ScanError::Failed("this scanner takes one resolution for both directions".into()));
        };
        let value = options::resolution_value(desc, dpi_y)?;
        session.set(table, "y-resolution", &value)?;
    }
    for (name, level) in [("brightness", settings.brightness), ("contrast", settings.contrast), ("threshold", settings.threshold)] {
        if let Some(level) = level
            && let Some((_, desc)) = table.active(name)
        {
            let value = options::level_value(desc, name, level)?;
            session.set(table, name, &value)?;
        }
    }
    let dpi_x = x_name.and_then(|n| session.number(table, n)).and_then(|d| u32::try_from(d).ok()).filter(|&d| d > 0).unwrap_or(settings.dpi);
    let dpi_y = session
        .number(table, "y-resolution")
        .and_then(|d| u32::try_from(d).ok())
        .filter(|&d| d > 0)
        .unwrap_or_else(|| if table.active("y-resolution").is_some() { settings.dpi_y.unwrap_or(dpi_x) } else { dpi_x });
    if let Some(region) = settings.region {
        for (name, value) in options::area_values(table, region, (dpi_x, dpi_y))? {
            session.set(table, name, &value)?;
        }
    }
    Ok((dpi_x, dpi_y))
}

/// Read one started frame's data, shutting the connection down if the caller cancels meanwhile.
fn read_frame(client: &Client, started: &crate::client::Started, settings: &ScanSettings) -> Result<(Vec<u8>, Status), Error> {
    let mut data = client.data(started)?;
    if let (Some(token), Some(handle)) = (&settings.cancel, data.shutdown_handle()) {
        token.set_canceller(Arc::new(move || {
            let _ = handle.shutdown(Shutdown::Both);
        }));
        if token.is_requested()
            && let Some(handle) = data.shutdown_handle()
        {
            let _ = handle.shutdown(Shutdown::Both);
        }
    }
    let progress = settings.progress.clone();
    let result = data.read_frame(&mut |received| {
        if let Some(progress) = &progress {
            progress.set_transfer(None, received, None);
        }
    });
    if let Some(token) = &settings.cancel {
        token.clear_canceller();
    }
    result
}

/// The frames of one image: start and read until the last frame (three for a three-pass colour scan).
fn image(session: &mut Session, settings: &ScanSettings) -> Result<(Vec<RawFrame>, ByteOrder), End> {
    let handle = session.handle;
    let client = session.client().map_err(End::Failed)?;
    let mut frames = Vec::new();
    loop {
        let started = match client.start(handle) {
            Ok(started) => started,
            Err(e) if e.status() == Some(Status::NoDocs) => return Err(End::Exhausted(e)),
            Err(_) if is_cancelled(settings) => return Err(End::Cancelled),
            Err(e) => return Err(End::Failed(scan_error("start", &e))),
        };
        let order = started.byte_order;
        let parameters = client.parameters(handle).map_err(|e| End::Failed(scan_error("get parameters", &e)))?;
        if let Some(progress) = &settings.progress {
            progress.set_expected_bytes(parameters.expected_bytes());
        }
        match read_frame(client, &started, settings) {
            Ok((data, Status::Eof)) => frames.push(RawFrame { parameters, data }),
            _ if is_cancelled(settings) => return Err(End::Cancelled),
            Ok((_, status)) => return Err(End::Failed(scan_error("read", &Error::Status { operation: "read", status }))),
            Err(e) => return Err(End::Failed(scan_error("read", &e))),
        }
        if parameters.last_frame || frames.len() >= 3 {
            return Ok((frames, order));
        }
    }
}

/// Scan images until `wanted` arrived, the feeder is empty, the caller cancels or something fails.
fn transfer(session: &mut Session, settings: &ScanSettings, dpi: (u32, u32), wanted: usize, pages: &mut Vec<Page>) -> End {
    let rule = bw::PageRule::for_settings(settings);
    while pages.len() < wanted {
        if is_cancelled(settings) {
            return End::Cancelled;
        }
        let (frames, order) = match image(session, settings) {
            Ok(image) => image,
            Err(end) => return end,
        };
        let png = match frame::encode(&frames, order, dpi) {
            Ok(png) => png,
            Err(why) => return End::Failed(ScanError::Failed(format!("the scanner sent an image this backend cannot read: {why}"))),
        };
        let (page, _) = bw::page_for(&png, rule);
        if let Some(progress) = &settings.progress {
            progress.push_page(page.clone());
        }
        pages.push(page);
    }
    End::Counted
}

/// One pass over `source`: set the options, transfer, reset the device, and judge the result.
fn scan_source(session: &mut Session, table: &mut OptionTable, settings: &ScanSettings, source: Source) -> Result<Acquired, ScanError> {
    if let Some(progress) = &settings.progress {
        progress.set_source(source);
    }
    let dpi = apply(session, table, settings, source)?;
    let sides = if settings.duplex { 2 } else { 1 };
    let wanted = match (source, settings.pages) {
        (Source::Glass, _) => 1,
        (_, 0) => scan::MAX_PAGES * sides,
        (_, pages) => pages * sides,
    };
    let mut pages = Vec::new();
    let end = transfer(session, settings, dpi, usize::try_from(wanted).unwrap_or(usize::MAX), &mut pages);
    // A scan ends with a cancel, whether it completed or not: it returns the device to idle.
    session.cancel();
    let cancelled = matches!(end, End::Cancelled);
    let download_error = match end {
        End::Failed(error) => Some(error),
        End::Exhausted(e) if pages.is_empty() => Some(scan_error("start", &e)),
        End::Exhausted(_) | End::Counted | End::Cancelled => None,
    };
    // Two-sided scans count sides, not sheets, so the page-count check is skipped there.
    let checked = if settings.duplex { 0 } else { usize::try_from(settings.pages).unwrap_or(0) };
    if let Err(error) = scan::after_transfer_checked(checked, source == Source::Feeder, pages.len(), cancelled, download_error) {
        if pages.is_empty() {
            return Err(error);
        }
        return Err(ScanError::Incomplete { message: error.to_string(), cause: error.cause(), acquired: Acquired::new(source, pages) });
    }
    Ok(Acquired::new(source, pages))
}

impl SaneBackend {
    fn acquire_from(&self, target: &Target, settings: &ScanSettings) -> Result<Acquired, ScanError> {
        settings.checked()?;
        let mut session = self.open(target)?;
        let mut table = session.descriptors()?;
        let Sources { glass, feeder, .. } = options::sources(&table, &target.kind);
        // SANE cannot see paper before a scan: Auto tries the feeder, and an empty one sends it to the glass.
        let resolved = settings.source.resolve(None, feeder, glass)?;
        let result = scan_source(&mut session, &mut table, settings, resolved);
        match result {
            Err(error) if auto_fallback_to_glass(settings.source, resolved, glass, pages_in_error(&error), &error) => {
                let glass = settings.clone().with_source(Source::Glass).with_pages(1).with_duplex(false);
                scan_source(&mut session, &mut table, &glass, Source::Glass)
            }
            other => other,
        }
    }
}

impl Backend for SaneBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn scanner_list(&self) -> Result<Vec<ScannerRef>, ScanError> {
        Ok(self
            .targets()?
            .into_iter()
            .map(|t| {
                let mut row = ScannerRef::new(t.id(), t.name.clone(), Some(t.host.to_string()));
                row.connection = Connection::Other;
                row
            })
            .collect())
    }

    fn scanner_details(&self, native_id: &str) -> Result<Scanner, ScanError> {
        let target = self.target(native_id)?;
        self.details(&target)
    }

    fn scanners(&self) -> Result<Vec<Scanner>, ScanError> {
        Ok(self.targets()?.iter().filter_map(|t| self.details(t).ok()).collect())
    }

    fn diagnostic_dump(&self, filter: &str) -> Result<String, ScanError> {
        let needle = filter.to_lowercase();
        let mut out = String::new();
        for target in self.targets()? {
            if !needle.is_empty() && !target.id().to_lowercase().contains(&needle) && !target.name.to_lowercase().contains(&needle) {
                continue;
            }
            out.push_str(&format!(
                "== {} ({}, {})\n",
                target.id(),
                target.name,
                if target.kind.is_empty() { "type not listed" } else { &target.kind }
            ));
            let table = self.open(&target).and_then(|mut session| session.descriptors());
            match table {
                Ok(table) => {
                    for (index, desc) in table.iter().skip(1) {
                        out.push_str(&format!("  {index:3} {}\n", options::describe(desc)));
                    }
                }
                Err(error) => out.push_str(&format!("error: {error}\n")),
            }
        }
        Ok(out)
    }

    fn acquire(&self, settings: &ScanSettings) -> Result<Acquired, ScanError> {
        let target = self.target(&settings.device)?;
        self.acquire_from(&target, settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_ids() {
        let host = SaneHost::parse("192.168.7.7").unwrap();
        let id = native_id(host, "pixma:04A9_1234:extra");
        assert_eq!(id, "192.168.7.7:6566/pixma:04A9_1234:extra");
        let (back, device) = parse_id(&id).unwrap();
        assert_eq!((back, device), (host, "pixma:04A9_1234:extra"));
        assert_eq!(parse_id("[fd00::7]:6566/test:0").unwrap().1, "test:0");
        assert!(parse_id("192.168.7.7:6566/").is_none());
        assert!(parse_id("scanner.local/test:0").is_none());
        assert!(parse_id("test:0").is_none());
    }

    #[test]
    fn adding_opens_nothing_and_keeps_one_entry() {
        let backend = SaneBackend::new();
        assert_eq!(backend.add_host("192.168.7.7").unwrap(), "192.168.7.7:6566");
        backend.add_host("192.168.7.7:6566").unwrap();
        let id = backend.add_device("10.0.0.9", "test:0").unwrap();
        assert_eq!(backend.added(), vec!["192.168.7.7:6566".to_owned(), id.clone()]);
        assert!(backend.add_host("scanner.example").is_err());
        assert!(backend.add_host("203.0.113.5").is_err());
        assert!(backend.add_device("10.0.0.9", "").is_err());
        assert!(backend.remove(&id));
        assert!(!backend.remove(&id));
        assert!(backend.remove("192.168.7.7"));
        assert!(backend.added().is_empty());
    }
}
