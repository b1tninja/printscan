//! The session state machines, in plain Rust: ImageCaptureCore's delegate callbacks are
//! [`Event`]s, the calls the backend makes are [`Command`]s, and a [`Machine`] decides which
//! command follows which event. [`drive`] runs a machine against a [`Port`], which on macOS sends
//! each command to the main queue and collects the callbacks (`main_thread`), and in the tests is a
//! scripted device.
//!
//! Every session goes the same way: open it, wait (briefly) for the device to be ready, list its
//! functional units, do the work, close it. [`Probe`] reads every unit; [`Acquire`] picks one,
//! writes the plan to it and scans. Each step has a deadline, so a callback that never comes ends
//! the session with an error, not a hang.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use printscan::scan::{self, Acquired, HandlingStatus, InterruptKind, Page, ScanError, ScanSettings, Source, bw};

use super::errors::{self, IcaError};
use super::model::{UnitInfo, UnitKind};
use super::options::IcaOptions;
use super::plan::{self, ScanPlan, UnitSetup};

/// How long [`drive`] waits for events before it looks at the deadlines again.
pub(crate) const SLICE: Duration = Duration::from_millis(50);
/// How often a running scan's progress is read.
pub(crate) const PROGRESS_EVERY: Duration = Duration::from_millis(500);
/// The largest page file read back: a bigger one is refused, not loaded.
pub(crate) const MAX_PAGE_BYTES: u64 = 1 << 30;

/// Where ImageCaptureCore writes the pages of a scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Transfer {
    /// A directory the backend made for this scan and removes afterwards.
    pub(crate) dir: PathBuf,
    /// The `documentUTI`.
    pub(crate) uti: &'static str,
    /// The `documentName`.
    pub(crate) name: String,
}

/// A call into ImageCaptureCore for the session's device. Each one returns at once; its outcome
/// arrives as an [`Event`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Command {
    /// `requestOpenSession` (answered by [`Event::Opened`]).
    Open,
    /// Read `availableFunctionalUnitTypes` ([`Event::Units`]).
    Units,
    /// `requestSelectFunctionalUnit:` ([`Event::Selected`]).
    Select(UnitKind),
    /// Read the selected functional unit ([`Event::UnitRead`]).
    Read,
    /// Write the plan to the selected unit ([`Event::Configured`]).
    Configure(UnitSetup),
    /// File-based transfer into the directory, then `requestScan` ([`Event::Scanned`], [`Event::Completed`]).
    Scan(Transfer),
    /// Read `scanProgressPercentDone` ([`Event::Progress`]).
    Progress,
    /// `cancelScan` ([`Event::Completed`]).
    Cancel,
    /// `requestCloseSession` ([`Event::Closed`]).
    Close,
}

/// A delegate callback, or the answer to a [`Command`], as plain data.
#[derive(Debug, PartialEq)]
pub(crate) enum Event {
    /// `device:didOpenSessionWithError:`.
    Opened(Option<IcaError>),
    /// `deviceDidBecomeReady:` or `scannerDeviceDidBecomeAvailable:`.
    Ready,
    Units(Vec<UnitKind>),
    /// `scannerDevice:didSelectFunctionalUnit:error:`; `unit` is `None` for a type this crate does not know.
    Selected {
        unit: Option<UnitKind>,
        error: Option<IcaError>,
    },
    UnitRead(UnitInfo),
    Configured,
    /// `scannerDevice:didScanToURL:` with a file URL.
    Scanned(PathBuf),
    /// Percent done, 0 to 100.
    Progress(f64),
    /// `scannerDevice:didCompleteScanWithError:`.
    Completed(Option<IcaError>),
    /// `device:didCloseSessionWithError:`.
    Closed(Option<IcaError>),
    /// `didRemoveDevice:`, or the browser lost the device.
    Removed,
    /// `device:didEncounterError:`.
    DeviceError(IcaError),
    /// A command could not be carried out (the device is not listed any more, nothing is selected).
    Refused(ScanError),
    /// The caller's `CancelToken` was requested.
    CancelRequested,
}

/// Carries commands to a device and brings its events back.
pub(crate) trait Port {
    /// Ask for `command`. Returns at once.
    fn send(&mut self, command: Command);
    /// The events that arrived, waiting up to `slice` for the first.
    fn wait(&mut self, slice: Duration) -> Vec<Event>;
}

/// A session's logic.
pub(crate) trait Machine {
    type Output;
    /// The first commands.
    fn start(&mut self, now: Instant) -> Vec<Command>;
    /// What follows `event`.
    fn event(&mut self, event: Event, now: Instant) -> Vec<Command>;
    /// What follows the passing of time (deadlines, progress reads).
    fn tick(&mut self, now: Instant) -> Vec<Command>;
    fn done(&self) -> bool;
    fn finish(self) -> Self::Output;
}

/// Run `machine` against `port` until it is done. `now` is the clock (a test's is virtual).
pub(crate) fn drive<M: Machine>(mut machine: M, port: &mut impl Port, now: &mut impl FnMut() -> Instant) -> M::Output {
    for command in machine.start(now()) {
        port.send(command);
    }
    while !machine.done() {
        for event in port.wait(SLICE) {
            for command in machine.event(event, now()) {
                port.send(command);
            }
            if machine.done() {
                break;
            }
        }
        if !machine.done() {
            for command in machine.tick(now()) {
                port.send(command);
            }
        }
    }
    machine.finish()
}

/// The steps every session shares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Opening,
    Readying,
    Units,
    Working,
    Closing,
    Done,
}

/// The shared part of a session: opening, readiness, the unit list and closing.
#[derive(Debug)]
struct Core {
    options: IcaOptions,
    phase: Phase,
    deadline: Instant,
    /// The session opened, so it must be closed.
    open: bool,
    /// The device said it is ready before the session-open callback arrived.
    ready_early: bool,
    listed: Vec<UnitKind>,
    /// The first error that ended the session.
    error: Option<ScanError>,
}

/// What [`Core::event`] made of an event.
enum Routed {
    /// The core handled it.
    Handled,
    /// The unit list arrived: the work starts.
    Begin,
    /// The work's event.
    Work(Event),
}

impl Core {
    fn new(options: IcaOptions, now: Instant) -> Self {
        Self { options, phase: Phase::Opening, deadline: now, open: false, ready_early: false, listed: Vec::new(), error: None }
    }

    fn start(&mut self, now: Instant) -> Vec<Command> {
        self.phase = Phase::Opening;
        self.deadline = now + self.options.step_timeout;
        vec![Command::Open]
    }

    fn step(&mut self, phase: Phase, now: Instant, within: Duration) {
        self.phase = phase;
        self.deadline = now + within;
    }

    fn list_units(&mut self, now: Instant, out: &mut Vec<Command>) {
        self.step(Phase::Units, now, self.options.step_timeout);
        out.push(Command::Units);
    }

    /// End the session with `error` (the first one is kept).
    fn fail(&mut self, error: ScanError, now: Instant, out: &mut Vec<Command>) {
        self.error.get_or_insert(error);
        self.close(now, out);
    }

    /// Close the session, or end at once when it never opened.
    fn close(&mut self, now: Instant, out: &mut Vec<Command>) {
        if self.open && self.phase != Phase::Closing && self.phase != Phase::Done {
            self.step(Phase::Closing, now, self.options.step_timeout);
            out.push(Command::Close);
        } else if self.phase != Phase::Closing {
            self.phase = Phase::Done;
        }
    }

    fn event(&mut self, event: Event, now: Instant, out: &mut Vec<Command>) -> Routed {
        match (self.phase, event) {
            (Phase::Done, _) => {}
            (_, Event::Removed) => {
                self.error.get_or_insert_with(errors::removed);
                self.phase = Phase::Done;
            }
            (Phase::Closing, Event::Closed(_)) => self.phase = Phase::Done,
            (Phase::Closing, _) => {}
            (Phase::Working, event) => return Routed::Work(event),
            (Phase::Opening, Event::Ready) => self.ready_early = true,
            (Phase::Opening, Event::Opened(None)) => {
                self.open = true;
                if self.ready_early {
                    self.list_units(now, out);
                } else {
                    self.step(Phase::Readying, now, self.options.ready_grace);
                }
            }
            (Phase::Opening, Event::Opened(Some(error))) => self.fail(errors::scan_error(&error, "opening the session"), now, out),
            (_, Event::Refused(error)) => self.fail(error, now, out),
            (Phase::Readying, Event::Ready | Event::Selected { .. }) => self.list_units(now, out),
            (Phase::Units, Event::Units(listed)) => {
                if listed.is_empty() {
                    self.fail(ScanError::Failed("the scanner lists no functional unit".into()), now, out);
                } else {
                    self.listed = listed;
                    self.phase = Phase::Working;
                    return Routed::Begin;
                }
            }
            // Anything else (a late or unsolicited callback) changes nothing.
            _ => {}
        }
        Routed::Handled
    }

    /// The deadline of a shared step; `true` when the work's own deadline passed.
    fn tick(&mut self, now: Instant, out: &mut Vec<Command>) -> bool {
        if now < self.deadline {
            return false;
        }
        match self.phase {
            Phase::Opening => {
                self.error.get_or_insert_with(|| errors::timed_out("opening the session", self.options.step_timeout));
                // The open may still be under way; closing is harmless if it is not.
                out.push(Command::Close);
                self.phase = Phase::Done;
            }
            Phase::Readying => self.list_units(now, out),
            Phase::Units => self.fail(errors::timed_out("reading the functional units", self.options.step_timeout), now, out),
            Phase::Closing => self.phase = Phase::Done,
            Phase::Working => return true,
            Phase::Done => {}
        }
        false
    }
}

/// What a probe read.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Probed {
    /// Every unit type the scanner lists.
    pub(crate) listed: Vec<UnitKind>,
    /// The units that could be selected and read.
    pub(crate) units: Vec<UnitInfo>,
}

/// Open a session, select and read every functional unit, close. Feeds no paper.
#[derive(Debug)]
pub(crate) struct Probe {
    core: Core,
    pending: Vec<UnitKind>,
    current: Option<UnitKind>,
    reading: bool,
    units: Vec<UnitInfo>,
}

impl Probe {
    pub(crate) fn new(options: IcaOptions, now: Instant) -> Self {
        Self { core: Core::new(options, now), pending: Vec::new(), current: None, reading: false, units: Vec::new() }
    }

    /// Select the next unit, or close when every unit was read.
    fn next(&mut self, now: Instant, out: &mut Vec<Command>) {
        self.reading = false;
        match self.pending.first().copied() {
            Some(unit) => {
                self.pending.remove(0);
                self.current = Some(unit);
                self.core.deadline = now + self.core.options.step_timeout;
                out.push(Command::Select(unit));
            }
            None => {
                self.current = None;
                self.core.close(now, out);
            }
        }
    }
}

impl Machine for Probe {
    type Output = Result<Probed, ScanError>;

    fn start(&mut self, now: Instant) -> Vec<Command> {
        self.core.start(now)
    }

    fn event(&mut self, event: Event, now: Instant) -> Vec<Command> {
        let mut out = Vec::new();
        match self.core.event(event, now, &mut out) {
            Routed::Handled => {}
            Routed::Begin => {
                self.pending = self.core.listed.clone();
                self.next(now, &mut out);
            }
            Routed::Work(Event::Selected { unit, error }) if !self.reading => {
                if error.is_some() {
                    // That unit cannot be selected; the others may still be read.
                    self.next(now, &mut out);
                } else if unit == self.current {
                    self.reading = true;
                    self.core.deadline = now + self.core.options.step_timeout;
                    out.push(Command::Read);
                }
            }
            Routed::Work(Event::UnitRead(info)) if self.reading => {
                self.units.push(info);
                self.next(now, &mut out);
            }
            Routed::Work(Event::Refused(_)) => self.next(now, &mut out),
            Routed::Work(_) => {}
        }
        out
    }

    fn tick(&mut self, now: Instant) -> Vec<Command> {
        let mut out = Vec::new();
        if self.core.tick(now, &mut out) {
            // This unit did not answer; go on with the others.
            self.next(now, &mut out);
        }
        out
    }

    fn done(&self) -> bool {
        self.core.phase == Phase::Done
    }

    fn finish(self) -> Self::Output {
        match self.core.error {
            Some(error) => Err(error),
            None => Ok(Probed { listed: self.core.listed, units: self.units }),
        }
    }
}

/// Where an [`Acquire`] is between the unit list and the end of the scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Selecting(UnitKind),
    Reading(UnitKind),
    Configuring,
    Scanning,
}

/// Open a session, pick the unit, write the plan, scan and collect the pages, close.
#[derive(Debug)]
pub(crate) struct Acquire {
    core: Core,
    settings: ScanSettings,
    rule: bw::PageRule,
    step: Option<Step>,
    plan: Option<ScanPlan>,
    dir: Option<tempfile::TempDir>,
    pages: Vec<Page>,
    /// The caller cancelled.
    cancelled: bool,
    /// The scan was stopped because every page asked for arrived.
    counted: bool,
    /// `cancelScan` was sent.
    stopping: bool,
    /// The scan has fallen back from an empty feeder to the glass.
    fell_back: bool,
    /// What ended the scan itself.
    scan_error: Option<ScanError>,
    /// A `device:didEncounterError:` during the scan.
    device_error: Option<IcaError>,
    progress: f64,
    next_poll: Instant,
}

impl Acquire {
    pub(crate) fn new(settings: ScanSettings, options: IcaOptions, now: Instant) -> Self {
        let rule = bw::PageRule::for_settings(&settings);
        Self {
            core: Core::new(options, now),
            settings,
            rule,
            step: None,
            plan: None,
            dir: None,
            pages: Vec::new(),
            cancelled: false,
            counted: false,
            stopping: false,
            fell_back: false,
            scan_error: None,
            device_error: None,
            progress: -1.0,
            next_poll: now,
        }
    }

    fn has(&self, unit: UnitKind) -> bool {
        self.core.listed.contains(&unit)
    }

    fn select(&mut self, unit: UnitKind, now: Instant, out: &mut Vec<Command>) {
        self.step = Some(Step::Selecting(unit));
        self.core.deadline = now + self.core.options.step_timeout;
        out.push(Command::Select(unit));
    }

    /// The first unit to read: the feeder when it may be used (to learn whether paper is loaded), otherwise the glass.
    fn begin(&mut self, now: Instant, out: &mut Vec<Command>) {
        if self.cancelled {
            self.core.close(now, out);
            return;
        }
        let requested = self.settings.source;
        let first = match requested {
            Source::Glass => plan::resolve_source(Source::Glass, &self.core.listed, None).map(|_| UnitKind::Flatbed),
            Source::Feeder => plan::resolve_source(Source::Feeder, &self.core.listed, None).map(|_| UnitKind::DocumentFeeder),
            _ if self.has(UnitKind::DocumentFeeder) => Ok(UnitKind::DocumentFeeder),
            _ => plan::resolve_source(requested, &self.core.listed, None).map(plan::unit_for),
        };
        match first {
            Ok(unit) => self.select(unit, now, out),
            Err(error) => self.core.fail(error, now, out),
        }
    }

    /// A unit was read: switch to the glass, refuse an empty feeder, or plan and configure.
    fn read(&mut self, info: UnitInfo, now: Instant, out: &mut Vec<Command>) {
        let source = if info.kind == UnitKind::DocumentFeeder && !self.fell_back {
            match plan::resolve_source(self.settings.source, &self.core.listed, info.document_loaded) {
                Ok(Source::Glass) => return self.select(UnitKind::Flatbed, now, out),
                Ok(source) => source,
                Err(error) => return self.core.fail(error, now, out),
            }
        } else {
            plan::resolve_source(Source::Glass, &self.core.listed, None).unwrap_or(Source::Glass)
        };
        if source == Source::Feeder {
            let status = HandlingStatus::default().with_feeder_ready(info.document_loaded);
            if let Err(error) = scan::preflight(Source::Feeder, Some(status)) {
                return self.core.fail(error, now, out);
            }
        }
        match plan::plan(&self.settings, &info, source) {
            Ok(planned) => {
                if let Some(progress) = &self.settings.progress {
                    progress.set_source(planned.source);
                }
                self.step = Some(Step::Configuring);
                self.core.deadline = now + self.core.options.step_timeout;
                out.push(Command::Configure(planned.setup.clone()));
                self.plan = Some(planned);
            }
            Err(error) => self.core.fail(error, now, out),
        }
    }

    /// The unit is configured: make the directory and start the scan.
    fn scan(&mut self, now: Instant, out: &mut Vec<Command>) {
        let Some(uti) = self.plan.as_ref().map(|planned| planned.uti) else { return };
        if self.dir.is_none() {
            match tempfile::Builder::new().prefix("printscan-ica-").tempdir() {
                Ok(dir) => self.dir = Some(dir),
                Err(error) => return self.core.fail(ScanError::Failed(format!("no directory for the scanned pages: {error}")), now, out),
            }
        }
        let Some(dir) = self.dir.as_ref().map(|dir| dir.path().to_path_buf()) else { return };
        self.step = Some(Step::Scanning);
        self.core.deadline = now + self.core.options.scan_idle_timeout;
        self.next_poll = now + PROGRESS_EVERY;
        out.push(Command::Scan(Transfer { dir, uti, name: "page".into() }));
    }

    fn stop(&mut self, out: &mut Vec<Command>) {
        if !self.stopping {
            self.stopping = true;
            out.push(Command::Cancel);
        }
    }

    /// A page file arrived: read it into a page, remove it, and stop once every page asked for is in.
    fn page(&mut self, path: &Path, out: &mut Vec<Command>) {
        match self.dir.as_ref().map(|dir| take_page(dir.path(), path, self.rule)) {
            Some(Ok(page)) => {
                if let Some(progress) = &self.settings.progress {
                    progress.push_page(page.clone());
                }
                self.pages.push(page);
            }
            Some(Err(error)) => {
                self.scan_error.get_or_insert(error);
                self.stop(out);
                return;
            }
            None => return,
        }
        // The glass ends by itself after its page; only a feeder is stopped by count.
        let Some(wanted) = self.plan.as_ref().filter(|planned| planned.source == Source::Feeder).map(|planned| planned.images) else { return };
        let enough = if wanted == 0 { scan::MAX_PAGES * 2 } else { wanted };
        if u32::try_from(self.pages.len()).unwrap_or(u32::MAX) >= enough {
            self.counted = true;
            self.stop(out);
        }
    }

    /// The scan ended, with `error` when ImageCaptureCore gave one.
    fn completed(&mut self, error: Option<IcaError>, now: Instant, out: &mut Vec<Command>) {
        let ended = match error {
            Some(error) if error.is_cancel() && (self.cancelled || self.counted) => None,
            Some(error) if error.is_cancel() && !self.stopping => {
                Some(errors::scan_error(&error, "scanning").restated(InterruptKind::Cancelled, InterruptKind::Cancelled.sentence()))
            }
            Some(error) => Some(errors::scan_error(&error, "scanning")),
            None if self.pages.is_empty() => self.device_error.as_ref().map(|error| errors::scan_error(error, "scanning")),
            None => None,
        };
        if let Some(error) = ended {
            self.scan_error.get_or_insert(error);
        }
        let source = self.plan.as_ref().map_or(Source::Glass, |planned| planned.source);
        let fallback = self.scan_error.as_ref().is_some_and(|error| {
            !self.fell_back && scan::auto_fallback_to_glass(self.settings.source, source, self.has(UnitKind::Flatbed), self.pages.len(), error)
        });
        if fallback {
            self.fell_back = true;
            self.scan_error = None;
            self.device_error = None;
            self.stopping = false;
            self.plan = None;
            self.select(UnitKind::Flatbed, now, out);
        } else {
            self.step = None;
            self.core.close(now, out);
        }
    }

    fn cancel(&mut self, now: Instant, out: &mut Vec<Command>) {
        if self.cancelled {
            return;
        }
        self.cancelled = true;
        match (self.core.phase, self.step) {
            (Phase::Working, Some(Step::Scanning)) => {
                self.core.deadline = now + self.core.options.step_timeout;
                self.stop(out);
            }
            (Phase::Working, _) => {
                self.step = None;
                self.core.close(now, out);
            }
            // Before the work starts the session still opens; `begin` then closes it.
            _ => {}
        }
    }

    fn work(&mut self, event: Event, now: Instant, out: &mut Vec<Command>) {
        match (self.step, event) {
            (Some(Step::Selecting(want)), Event::Selected { unit, error }) => match error {
                Some(error) => self.core.fail(errors::scan_error(&error, &format!("selecting the {}", want.label())), now, out),
                None if unit == Some(want) => {
                    self.step = Some(Step::Reading(want));
                    self.core.deadline = now + self.core.options.step_timeout;
                    out.push(Command::Read);
                }
                None => {}
            },
            (Some(Step::Reading(want)), Event::UnitRead(info)) if info.kind == want => self.read(info, now, out),
            (Some(Step::Configuring), Event::Configured) => self.scan(now, out),
            (Some(Step::Scanning), Event::Scanned(path)) => {
                self.core.deadline = now + self.core.options.scan_idle_timeout;
                self.page(&path, out);
            }
            (Some(Step::Scanning), Event::Progress(percent)) => {
                if (percent - self.progress).abs() >= 0.5 {
                    self.progress = percent;
                    if !self.stopping {
                        self.core.deadline = now + self.core.options.scan_idle_timeout;
                    }
                    if let Some(progress) = &self.settings.progress {
                        progress.set_transfer(Some(percent.clamp(0.0, 100.0) as u8), 0, None);
                    }
                }
            }
            (Some(Step::Scanning), Event::Completed(error)) => self.completed(error, now, out),
            (Some(Step::Scanning), Event::DeviceError(error)) => {
                self.device_error.get_or_insert(error);
            }
            (Some(Step::Scanning), Event::Refused(error)) => {
                self.scan_error.get_or_insert(error);
                self.step = None;
                self.core.close(now, out);
            }
            (_, Event::Refused(error)) => self.core.fail(error, now, out),
            _ => {}
        }
    }
}

impl Machine for Acquire {
    type Output = Result<Acquired, ScanError>;

    fn start(&mut self, now: Instant) -> Vec<Command> {
        self.core.start(now)
    }

    fn event(&mut self, event: Event, now: Instant) -> Vec<Command> {
        let mut out = Vec::new();
        if event == Event::CancelRequested {
            self.cancel(now, &mut out);
            return out;
        }
        match self.core.event(event, now, &mut out) {
            Routed::Handled => {}
            Routed::Begin => self.begin(now, &mut out),
            Routed::Work(event) => self.work(event, now, &mut out),
        }
        out
    }

    fn tick(&mut self, now: Instant) -> Vec<Command> {
        let mut out = Vec::new();
        if self.core.tick(now, &mut out) {
            let doing = match self.step {
                Some(Step::Selecting(unit) | Step::Reading(unit)) => format!("selecting the {}", unit.label()),
                Some(Step::Configuring) => "setting up the scan".to_string(),
                Some(Step::Scanning) => "scanning".to_string(),
                None => "ending the scan".to_string(),
            };
            let within = if self.step == Some(Step::Scanning) && !self.stopping {
                self.core.options.scan_idle_timeout
            } else {
                self.core.options.step_timeout
            };
            if self.step == Some(Step::Scanning) {
                if !self.stopping {
                    self.scan_error.get_or_insert_with(|| errors::timed_out(&doing, within));
                }
                self.stop(&mut out);
                self.step = None;
                self.core.close(now, &mut out);
            } else {
                self.core.fail(errors::timed_out(&doing, within), now, &mut out);
            }
        } else if self.step == Some(Step::Scanning) && now >= self.next_poll {
            self.next_poll = now + PROGRESS_EVERY;
            out.push(Command::Progress);
        }
        out
    }

    fn done(&self) -> bool {
        self.core.phase == Phase::Done
    }

    fn finish(self) -> Self::Output {
        let Some(planned) = self.plan else {
            return match self.core.error {
                Some(error) => Err(error),
                None if self.cancelled => scan::after_transfer(0, true, None).map(|()| Acquired::new(self.settings.source, Vec::new())),
                None => Err(ScanError::Failed("the session ended before the scan started".into())),
            };
        };
        let download_error = self.scan_error.or(self.core.error);
        let feeder = planned.source == Source::Feeder;
        // Two-sided scans count sides, not sheets, so the page-count check is skipped there.
        let checked = if planned.setup.duplex == Some(true) { 0 } else { usize::try_from(planned.images).unwrap_or(0) };
        let pages = self.pages;
        let checked = if feeder { checked } else { 0 };
        match scan::after_transfer_checked(checked, feeder, pages.len(), self.cancelled, download_error) {
            Ok(()) => Ok(Acquired::new(planned.source, pages)),
            Err(error) if pages.is_empty() => Err(error),
            Err(error) => {
                Err(ScanError::Incomplete { message: error.to_string(), cause: error.cause(), acquired: Acquired::new(planned.source, pages) })
            }
        }
    }
}

/// Read the page file ImageCaptureCore wrote at `path`, which must be in `dir`, and remove it.
pub(crate) fn take_page(dir: &Path, path: &Path, rule: bw::PageRule) -> Result<Page, ScanError> {
    let unreadable = |what: &str| ScanError::Failed(format!("the scanned page {} {what}", path.display()));
    let inside = match (dir.canonicalize(), path.parent().map(Path::canonicalize)) {
        (Ok(dir), Some(Ok(parent))) => parent == dir,
        _ => false,
    };
    if !inside {
        return Err(unreadable("is not in the directory the scan was given"));
    }
    let size = std::fs::metadata(path).map_err(|error| unreadable(&format!("cannot be read: {error}")))?.len();
    if size > MAX_PAGE_BYTES {
        let _ = std::fs::remove_file(path);
        return Err(unreadable(&format!("is {size} bytes, more than the {MAX_PAGE_BYTES} a page may be")));
    }
    let bytes = std::fs::read(path).map_err(|error| unreadable(&format!("cannot be read: {error}")));
    let _ = std::fs::remove_file(path);
    let bytes = bytes?;
    if scan::image_format(&bytes) == "bin" {
        return Err(unreadable("is not an image this backend reads (TIFF, PNG, JPEG or BMP)"));
    }
    Ok(bw::page_for(&bytes, rule).0)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use printscan::scan::{Color, ProgressToken};

    use super::*;
    use crate::ica::model::MeasurementUnit;

    const ICA: Option<&str> = Some("com.apple.ImageCaptureCore");

    /// A 24-bit BMP of `width` by `height` white pixels at 300 dpi.
    fn bmp(width: u32, height: u32) -> Vec<u8> {
        let stride = (width * 3).div_ceil(4) * 4;
        let size = 54 + stride * height;
        let mut out = Vec::new();
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(stride * height).to_le_bytes());
        out.extend_from_slice(&11811u32.to_le_bytes());
        out.extend_from_slice(&11811u32.to_le_bytes());
        out.extend_from_slice(&[0; 8]);
        out.resize(usize::try_from(size).unwrap(), 0xff);
        out
    }

    /// How the scripted scanner behaves.
    #[derive(Clone)]
    struct Script {
        units: Vec<UnitKind>,
        feeder_loaded: bool,
        /// Pages the feeder holds (the glass always gives one).
        sheets: usize,
        /// The scan ends with this error after the pages.
        end: Option<IcaError>,
        /// The session never opens.
        silent: bool,
        /// Cancel the token after this many pages.
        cancel_after: Option<usize>,
        /// The device is removed after this many pages.
        removed_after: Option<usize>,
    }

    impl Default for Script {
        fn default() -> Self {
            Self {
                units: vec![UnitKind::Flatbed, UnitKind::DocumentFeeder],
                feeder_loaded: true,
                sheets: 3,
                end: None,
                silent: false,
                cancel_after: None,
                removed_after: None,
            }
        }
    }

    /// A scripted device: answers each command as ImageCaptureCore is expected to.
    struct Fake {
        script: Script,
        selected: UnitKind,
        queue: VecDeque<Event>,
        sent: Vec<Command>,
        scanning: Option<(PathBuf, usize)>,
        clock: Instant,
    }

    impl Fake {
        fn new(script: Script) -> Self {
            Self { script, selected: UnitKind::Flatbed, queue: VecDeque::new(), sent: Vec::new(), scanning: None, clock: Instant::now() }
        }

        fn info(&self) -> UnitInfo {
            let unit = UnitInfo::new(self.selected)
                .with_resolutions(vec![150, 300])
                .with_bit_depths(vec![8])
                .with_measurement_unit(MeasurementUnit::Inches)
                .with_physical_size((8.5, 11.0));
            match self.selected {
                UnitKind::DocumentFeeder => unit.with_document_loaded(self.script.feeder_loaded).with_supports_duplex(false),
                _ => unit,
            }
        }
    }

    impl Port for Fake {
        fn send(&mut self, command: Command) {
            self.sent.push(command.clone());
            let events = match command {
                Command::Open if self.script.silent => vec![],
                Command::Open => vec![Event::Opened(None), Event::Ready],
                Command::Units => vec![Event::Units(self.script.units.clone())],
                Command::Select(unit) => {
                    self.selected = unit;
                    vec![Event::Selected { unit: Some(unit), error: None }]
                }
                Command::Read => vec![Event::UnitRead(self.info())],
                Command::Configure(_) => vec![Event::Configured],
                Command::Scan(transfer) => {
                    self.scanning = Some((transfer.dir, 0));
                    vec![]
                }
                Command::Progress => vec![Event::Progress(50.0)],
                Command::Cancel => {
                    self.scanning = None;
                    vec![Event::Completed(Some(IcaError::new("com.apple.ImageCaptureCore", errors::SCAN_OPERATION_CANCELED, "", ICA)))]
                }
                Command::Close => vec![Event::Closed(None)],
            };
            self.queue.extend(events);
        }

        fn wait(&mut self, slice: Duration) -> Vec<Event> {
            self.clock += slice;
            // A running scan delivers one page per wait.
            if let Some((dir, done)) = self.scanning.take() {
                let total = if self.selected == UnitKind::DocumentFeeder { self.script.sheets } else { 1 };
                if self.script.removed_after == Some(done) {
                    self.queue.push_back(Event::Removed);
                } else if self.script.cancel_after == Some(done) {
                    self.queue.push_back(Event::CancelRequested);
                    self.scanning = Some((dir, done));
                } else if done < total {
                    let path = dir.join(format!("page-{done}.bmp"));
                    std::fs::write(&path, bmp(4, 4)).unwrap();
                    self.queue.push_back(Event::Scanned(path));
                    self.scanning = Some((dir, done + 1));
                } else {
                    self.queue.push_back(Event::Completed(self.script.end.clone()));
                }
            }
            self.queue.drain(..).collect()
        }
    }

    fn run<M: Machine>(machine: M, fake: &mut Fake) -> M::Output {
        let start = fake.clock;
        let mut ticks = 0u32;
        // The fake's clock moves with each wait; read it through a counter so `now` needs no borrow.
        let mut now = || {
            ticks += 1;
            start + SLICE * ticks
        };
        drive(machine, fake, &mut now)
    }

    fn acquire(settings: ScanSettings, script: Script) -> (Result<Acquired, ScanError>, Vec<Command>) {
        let mut fake = Fake::new(script);
        let result = run(Acquire::new(settings, IcaOptions::default(), fake.clock), &mut fake);
        (result, fake.sent)
    }

    #[test]
    fn a_probe_reads_every_unit_and_closes() {
        let mut fake = Fake::new(Script::default());
        let probed = run(Probe::new(IcaOptions::default(), fake.clock), &mut fake).unwrap();
        assert_eq!(probed.listed, vec![UnitKind::Flatbed, UnitKind::DocumentFeeder]);
        assert_eq!(probed.units.iter().map(|u| u.kind).collect::<Vec<_>>(), vec![UnitKind::Flatbed, UnitKind::DocumentFeeder]);
        assert_eq!(fake.sent.last(), Some(&Command::Close));
        assert!(!fake.sent.iter().any(|c| matches!(c, Command::Scan(_))), "a probe feeds no paper");
    }

    #[test]
    fn a_silent_device_times_out() {
        let mut fake = Fake::new(Script { silent: true, ..Script::default() });
        let options = IcaOptions::default().with_step_timeout(Duration::from_secs(1));
        let error = run(Probe::new(options, fake.clock), &mut fake).unwrap_err();
        assert_eq!(error.native(), Some(errors::TIMED_OUT));
        assert!(error.to_string().contains("main thread"));
    }

    #[test]
    fn auto_takes_the_whole_feeder() {
        let progress = ProgressToken::new();
        let settings = ScanSettings::default().with_dpi(300).with_pages(0).with_progress(progress.clone());
        let (result, sent) = acquire(settings, Script::default());
        let acquired = result.unwrap();
        assert_eq!(acquired.source, Source::Feeder);
        assert_eq!(acquired.pages.len(), 3);
        assert_eq!((acquired.pages[0].width, acquired.pages[0].format, acquired.pages[0].dpi), (4, "bmp", 300));
        assert_eq!(progress.pages_done(), 3);
        assert_eq!(progress.source(), Some(Source::Feeder));
        assert!(sent.iter().any(|c| matches!(c, Command::Scan(t) if t.uti == "public.tiff")));
        assert_eq!(sent.last(), Some(&Command::Close));
    }

    #[test]
    fn an_empty_feeder_sends_auto_to_the_glass() {
        let (result, sent) = acquire(ScanSettings::default().with_dpi(300), Script { feeder_loaded: false, ..Script::default() });
        let acquired = result.unwrap();
        assert_eq!((acquired.source, acquired.pages.len()), (Source::Glass, 1));
        let selects: Vec<_> = sent.iter().filter_map(|c| if let Command::Select(u) = c { Some(*u) } else { None }).collect();
        assert_eq!(selects, vec![UnitKind::DocumentFeeder, UnitKind::Flatbed]);
    }

    #[test]
    fn an_empty_feeder_asked_for_is_refused() {
        let settings = ScanSettings::default().with_dpi(300).with_source(Source::Feeder);
        let (result, sent) = acquire(settings, Script { feeder_loaded: false, ..Script::default() });
        assert_eq!(result.unwrap_err().interrupt_kind(), InterruptKind::FeederEmpty);
        assert_eq!(sent.last(), Some(&Command::Close));
    }

    #[test]
    fn a_fixed_count_stops_the_feeder() {
        let settings = ScanSettings::default().with_dpi(300).with_source(Source::Feeder).with_pages(2);
        let (result, sent) = acquire(settings, Script::default());
        assert_eq!(result.unwrap().pages.len(), 2);
        assert!(sent.contains(&Command::Cancel));
    }

    #[test]
    fn a_short_stack_keeps_its_pages() {
        let settings = ScanSettings::default().with_dpi(300).with_source(Source::Feeder).with_pages(5);
        let error = acquire(settings, Script::default()).0.unwrap_err();
        assert_eq!(scan::pages_in_error(&error), 3);
        assert_eq!(error.interrupt_kind(), InterruptKind::FeederEmpty);
    }

    #[test]
    fn a_resolution_the_unit_lacks_is_refused_before_scanning() {
        let (result, sent) = acquire(ScanSettings::default().with_dpi(600).with_source(Source::Glass), Script::default());
        assert!(result.unwrap_err().to_string().contains("150, 300"));
        assert!(!sent.iter().any(|c| matches!(c, Command::Scan(_))));
        assert_eq!(sent.last(), Some(&Command::Close));
    }

    #[test]
    fn cancel_mid_scan_keeps_the_pages() {
        let settings = ScanSettings::default().with_dpi(300).with_source(Source::Feeder).with_pages(0);
        let (result, sent) = acquire(settings, Script { cancel_after: Some(1), ..Script::default() });
        assert_eq!(result.unwrap().pages.len(), 1);
        assert!(sent.contains(&Command::Cancel));
    }

    #[test]
    fn cancel_before_any_page_is_a_cancel() {
        let settings = ScanSettings::default().with_dpi(300).with_source(Source::Feeder).with_pages(0);
        let error = acquire(settings, Script { cancel_after: Some(0), ..Script::default() }).0.unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    }

    #[test]
    fn a_jam_after_pages_is_incomplete() {
        let jam = IcaError::new("com.apple.ImageCaptureCore", -9931, "Paper jam", ICA);
        let settings = ScanSettings::default().with_dpi(300).with_source(Source::Feeder).with_pages(0);
        let error = acquire(settings, Script { end: Some(jam), ..Script::default() }).0.unwrap_err();
        assert!(matches!(&error, ScanError::Incomplete { acquired, .. } if acquired.pages.len() == 3));
        assert!(error.to_string().contains("Paper jam"));
    }

    #[test]
    fn a_cancel_at_the_device_is_named() {
        let cancelled = IcaError::new("com.apple.ImageCaptureCore", errors::SCAN_OPERATION_CANCELED, "", ICA);
        let settings = ScanSettings::default().with_dpi(300).with_source(Source::Glass);
        let mut script = Script { end: Some(cancelled), ..Script::default() };
        script.units = vec![UnitKind::Flatbed];
        // The glass page arrives, then the device says it cancelled: the page is kept, the cause named.
        let error = acquire(settings, script).0.unwrap_err();
        assert_eq!(scan::pages_in_error(&error), 1);
        assert_eq!(error.interrupt_kind(), InterruptKind::Cancelled);
    }

    #[test]
    fn a_removed_device_is_offline() {
        let settings = ScanSettings::default().with_dpi(300).with_source(Source::Feeder).with_pages(0);
        let error = acquire(settings, Script { removed_after: Some(2), ..Script::default() }).0.unwrap_err();
        assert_eq!(error.native(), Some(errors::DEVICE_REMOVED));
        assert_eq!(scan::pages_in_error(&error), 2);
    }

    #[test]
    fn black_and_white_from_grey_is_thresholded() {
        let settings = ScanSettings::default().with_dpi(300).with_source(Source::Glass).with_color(Color::BlackAndWhite);
        let acquired = acquire(settings, Script::default()).0.unwrap();
        assert!(acquired.pages[0].thresholded);
    }

    #[test]
    fn page_files_must_be_in_the_scan_directory() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let rule = bw::PageRule::for_settings(&ScanSettings::default());
        let outside = other.path().join("x.bmp");
        std::fs::write(&outside, bmp(2, 2)).unwrap();
        assert!(take_page(dir.path(), &outside, rule).is_err());
        assert!(outside.exists(), "a file outside the directory is not removed");
        let junk = dir.path().join("junk.bin");
        std::fs::write(&junk, b"not an image").unwrap();
        assert!(take_page(dir.path(), &junk, rule).is_err());
        assert!(!junk.exists());
        let good = dir.path().join("p.bmp");
        std::fs::write(&good, bmp(3, 2)).unwrap();
        assert_eq!(take_page(dir.path(), &good, rule).map(|p| (p.width, p.height)), Ok((3, 2)));
        assert!(!good.exists());
    }
}
