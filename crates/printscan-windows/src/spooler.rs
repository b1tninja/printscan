//! The Windows print spooler as a [`Backend`], on [`winprint`].
//!
//! Printers come from the spooler (local queues and `\\server\queue` connections). A PDF job is
//! drawn by the in-box `Windows.Data.Pdf` into the XPS print API at the resolution of the job's
//! PrintTicket, in this process: no other program is started.
//!
//! Windows prints a job on one paper in one orientation; the document's pages need not agree. The
//! paper is the one [`JobOptions::media`] names, else the first page's size, taken from the
//! driver's own paper list; the orientation is the one asked for, else the first page's. Every page
//! is then drawn with its own transform (`placement`): a page the other way round is turned a
//! quarter turn clockwise, and with [`Scaling::ShrinkToFit`] a page larger than the printable area
//! shrinks onto it. With [`Scaling::Exact`] every page must be the paper's size, either way round,
//! and a job with another size is refused before anything is sent.
//!
//! The job's settings become the PrintTicket, which the driver validates. A setting the job asks for
//! explicitly (two sides, grey or colour, collated or uncollated copies, an orientation) that the
//! validated ticket does not keep refuses the job: it would otherwise print differently from what
//! was asked. Copies the driver cannot make as asked (it has no collation, or fewer copies) are
//! drawn by the job itself, a blank back keeping each two-sided copy on its own sheets. Hints the
//! driver lacks (a resolution, a rendering intent) are named in [`Submitted::warnings`].
//!
//! A job with [`JobOptions::application_managed_colour`] sets `PageColorManagement` = `None` and
//! `PageDestinationColorProfile` = `Application` where the driver offers them, Microsoft's route for
//! colours already converted to the printer's profile (`docs/colour.md`). A driver that lists colour
//! management without `None`, or does not keep it, refuses the job; one with no colour-management
//! setting at all prints it with a warning. [`JobOptions::rendering_intent`] becomes
//! `PageICMRenderingIntent` where the driver offers that intent.
//!
//! The spooler gives no handle for the job, so the backend reports it accepted once the document
//! is in the queue ([`WatchSupport::None`]). [`open_preferences`] opens the driver's own Printing
//! Preferences window.

mod checks;
mod draw;
mod placement;

use std::ffi::OsStr;
use std::ops::ControlFlow;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use printscan::print::backends::{Backend, BackendClass, DocumentFormat, ErrorKind, JobHint, PrintError, PrinterDetails, PrinterRef, SubmitRequest};
use printscan::print::{
    CancelToken, ColourMode, Job, JobOptions, Orientation, Progress, ProgressCallback, RenderingIntent, Resolution, RetrySafety, Scaling, Submitted,
    WatchSupport,
};
use windows::Win32::Graphics::Direct2D::D2D1_COLOR_SPACE_SRGB;
use winprint::printer::{DxgiPrintContextError, DxgiRenderContext, PrintOutcome, PrinterDevice};
use winprint::ticket::document::reader::ParsableXmlDocument as _;
use winprint::ticket::document::{NS_PSK, PrintCapabilitiesDocument, Property, WithProperties as _};
use winprint::ticket::{
    Collation, Copies, FeatureOptionPack as _, FeatureOptionPackWithPredefined as _, JobDuplex, PageColorManagement, PageDestinationColorProfile,
    PageIcmRenderingIntent, PageMediaSize, PageOrientation, PageOutputColor, PageResolution, PredefinedCollation, PredefinedDuplexType,
    PredefinedPageColorManagement, PredefinedPageDestinationColorProfile, PredefinedPageOrientation, PredefinedPageOutputColor, PrintCapabilities,
    PrintTicket, PrintTicketBuilder, StaticFeaturePathOptionPack as _,
};
use winprint::utils::Deadline;

use checks::{Chosen, Wanted};
use placement::{Fit, Mismatch, Size};

/// The backend's registry name.
pub const NAME: &str = "spooler";

/// How far a paper size may differ from the one asked for, in micrometres (drivers round).
const PAPER_TOLERANCE: u32 = 1_000;

/// Keeps two threads of this process out of the Windows PrintTicket provider (`PTOpenProvider` and
/// the calls on its handle) at the same time. The process's first calls create a cache in winspool
/// without a lock, and two threads making them together can leave winspool holding freed memory: a
/// later call crashes or misbehaves. Every call here that reaches the provider holds it.
static PRINT_TICKET_PROVIDER: Mutex<()> = Mutex::new(());

/// Holds [`PRINT_TICKET_PROVIDER`] until the guard is dropped. A lock a panicking thread held is
/// taken like any other.
fn print_ticket_provider() -> MutexGuard<'static, ()> {
    PRINT_TICKET_PROVIDER.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How long a job may take by default, from opening the document until the print system has
/// taken the whole job ([`WindowsSpooler::with_timeout`]).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The Windows print spooler. Register it with a [`printscan::print::Registry`].
#[derive(Clone, Copy, Debug)]
pub struct WindowsSpooler {
    timeout: Option<Duration>,
}

impl WindowsSpooler {
    /// The spooler with [`DEFAULT_TIMEOUT`].
    pub const fn new() -> Self {
        Self { timeout: Some(DEFAULT_TIMEOUT) }
    }

    /// The same spooler with jobs limited to `timeout`, or unlimited with `None`. The limit bounds
    /// [`Backend::submit`]: a driver or spooler that never reports the end of a job would otherwise
    /// hold the caller for ever. A job past it is cancelled and fails with [`ErrorKind::Timeout`]. It
    /// counts drawing the pages too, so a long document needs a longer limit.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    /// The limit on a job; `None` for none.
    pub const fn timeout(&self) -> Option<Duration> {
        self.timeout
    }
}

impl Default for WindowsSpooler {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for WindowsSpooler {
    fn name(&self) -> &'static str {
        NAME
    }

    fn class(&self) -> BackendClass {
        BackendClass::Local
    }

    fn accepted_formats(&self) -> &[DocumentFormat] {
        &[DocumentFormat::Pdf]
    }

    fn list_printers(&self) -> Result<Vec<PrinterRef>, PrintError> {
        let default = default_name();
        let devices = PrinterDevice::all().map_err(|e| failed("listing printers", &e))?;
        Ok(devices
            .iter()
            .map(|device| {
                let mut printer = PrinterRef::new(device.name(), device.name());
                printer.default = default.as_deref() == Some(device.name());
                printer
            })
            .collect())
    }

    fn printer_details(&self, native_id: &str) -> Result<PrinterDetails, PrintError> {
        let device = device(Some(native_id))?;
        let capabilities = capabilities(&device)?;
        let two_sided = JobDuplex::list(&capabilities)
            .any(|d| matches!(d.as_predefined_name(), Some(PredefinedDuplexType::TwoSidedLongEdge | PredefinedDuplexType::TwoSidedShortEdge)));
        let colour = PageOutputColor::list(&capabilities).any(|c| c.as_predefined_name() == Some(PredefinedPageOutputColor::Color));
        let paper = capabilities.page_media_sizes().filter_map(|m| m.option().name.as_ref().map(|n| n.local_name.clone())).collect();
        let resolutions =
            capabilities.page_resolutions().map(|r| r.dpi()).filter(|&(x, y)| x > 0 && y > 0).map(|(x, y)| Resolution::new(x, y)).collect();
        let mut details = PrinterDetails::default();
        details.id = device.name().to_owned();
        details.name = device.name().to_owned();
        details.default = default_name().as_deref() == Some(device.name());
        details.formats = vec![DocumentFormat::Pdf];
        details.two_sided = Some(two_sided);
        details.colour = Some(colour);
        details.paper = paper;
        details.max_copies = Some(u32::from(u16::MAX));
        details.resolutions = resolutions;
        let mut honoured =
            vec![JobHint::Copies, JobHint::Collate, JobHint::Duplex, JobHint::Colour, JobHint::Media, JobHint::Orientation, JobHint::Resolution];
        // The colour features this driver lists: an intent becomes the ticket's, and `None` turns its colour
        // management off.
        if PageIcmRenderingIntent::list(&capabilities).any(|i| i.as_predefined_name().is_some()) {
            honoured.push(JobHint::RenderingIntent);
        }
        if PageColorManagement::list(&capabilities).any(|c| c.as_predefined_name() == Some(PredefinedPageColorManagement::None)) {
            honoured.push(JobHint::ApplicationManagedColour);
        }
        details.honoured = Some(honoured);
        Ok(details)
    }

    fn submit(&self, request: SubmitRequest<'_>) -> Result<Submitted, PrintError> {
        let device = device(request.job.printer.as_deref())?;
        let capabilities = capabilities(&device)?;
        // A closed file in a private folder: Windows.Data.Pdf opens the file itself, and an open
        // temporary file would be locked against it.
        let staging = |e: std::io::Error| PrintError::new(ErrorKind::Io, format!("staging the document: {e}")).with_safety(RetrySafety::Safe);
        let folder = tempfile::Builder::new().prefix("printscan-").tempdir().map_err(staging)?;
        let path = folder.path().join("document.pdf");
        std::fs::write(&path, request.document.bytes).map_err(staging)?;

        // The job's limit counts from here: reading the document, drawing it and the print system taking it.
        let deadline = Deadline::after(self.timeout);
        // Entered before any COM object of the job is made, and left after the last is released.
        let _apartment = draw::Apartment::enter();
        let pdf = draw::Pdf::open(&path).map_err(|e| failed("opening the document", &e))?;
        let pages = pdf.page_sizes().map_err(|e| failed("reading the document's pages", &e))?;
        let first = *pages.first().ok_or_else(|| PrintError::new(ErrorKind::InvalidRequest, "the document has no pages"))?;
        let plan = plan(&device, &capabilities, &request.job, &request.options, first)?;
        let placements = placement::place_all(&pages, plan.paper, plan.fit).map_err(|m| mismatch(m, plan.paper))?;
        let order: Vec<Option<usize>> = match plan.copies_here {
            Some(copies) => placement::sequence(pages.len(), copies, request.job.collate, plan.chosen.two_sided()),
            None => (0..pages.len()).map(Some).collect(),
        };

        let render = DxgiRenderContext::new().map_err(|e| failed("starting Direct2D", &e))?;
        let context = {
            // Turning the ticket into the job's DEVMODE goes through the PrintTicket provider.
            let _provider = print_ticket_provider();
            render
                .start_job(&device, &plan.ticket, OsStr::new(&request.job.title), D2D1_COLOR_SPACE_SRGB, deadline)
                .map_err(|e| job_failed("starting the job", &e))?
        };
        let drawer = draw::Drawer::new(&context.dxgi_device).map_err(|e| failed("starting the PDF renderer", &e))?;
        let pages: Vec<(Size, placement::Placement)> = pages.into_iter().zip(placements).collect();
        let job = Sending { context: &context, drawer: &drawer, pdf: &pdf, pages: &pages, paper: plan.paper };
        let outcome = match job.send(&order, &request.cancel, request.progress)? {
            Some(outcome) => outcome,
            None => context.close_and_wait().map_err(|e| job_failed("printing", &e))?,
        };
        match outcome {
            PrintOutcome::Completed => {
                let mut message = format!("the document is in the queue of {}", device.name());
                if let Some(copies) = plan.copies_here {
                    message.push_str(&format!(" ({copies} copies, drawn into one job: the driver cannot make them as asked)"));
                }
                let mut submitted = Submitted::new(NAME, message);
                submitted.warnings = plan.warnings;
                Ok(submitted)
            }
            _ => Err(PrintError::new(ErrorKind::Cancelled, "the job was cancelled").with_safety(RetrySafety::Safe)),
        }
    }

    fn watch_support(&self) -> WatchSupport {
        WatchSupport::None
    }
}

/// A failure of the job, or its deadline passing ([`ErrorKind::Timeout`]: winprint cancelled the
/// job, but pages may have reached the printer before it did).
fn job_failed(what: &str, error: &DxgiPrintContextError) -> PrintError {
    match error {
        DxgiPrintContextError::TimedOut(elapsed) => {
            PrintError::new(ErrorKind::Timeout, format!("{what}: the job was cancelled after {} s, its time limit", elapsed.as_secs()))
        }
        other => failed(what, other),
    }
}

/// The user's default printer, when there is one.
fn default_name() -> Option<String> {
    PrinterDevice::user_default().ok().flatten().map(|d| d.name().to_owned())
}

/// The printer named `name`, or the default one.
fn device(name: Option<&str>) -> Result<PrinterDevice, PrintError> {
    match name {
        Some(name) => PrinterDevice::by_os_name(name).map_err(|e| match e {
            winprint::printer::FindDeviceError::NotFound => PrintError::new(ErrorKind::NotFound, format!("no printer is called {name}")),
            other => failed("opening the printer", &other),
        }),
        None => PrinterDevice::user_default()
            .map_err(|e| failed("finding the default printer", &e))?
            .ok_or_else(|| PrintError::new(ErrorKind::NotFound, "there is no default printer")),
    }
}

fn capabilities(device: &PrinterDevice) -> Result<PrintCapabilities, PrintError> {
    let _provider = print_ticket_provider();
    PrintCapabilities::fetch(device).map_err(|e| failed("reading the printer's capabilities", &e))
}

/// A failure of a call into Windows, with its causes. Nothing reached the printer, or winprint
/// cancelled the job it had started, so repeating is safe.
fn failed(what: &str, error: &dyn std::error::Error) -> PrintError {
    let mut message = format!("{what}: {error}");
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    PrintError::new(ErrorKind::Failed, message).with_safety(RetrySafety::Safe)
}

/// A job's pages going into its print job.
struct Sending<'a> {
    context: &'a winprint::printer::DxgiPrintContext,
    drawer: &'a draw::Drawer,
    pdf: &'a draw::Pdf,
    pages: &'a [(Size, placement::Placement)],
    paper: Size,
}

impl Sending<'_> {
    /// Add the pages of `order` (a page index, or a blank back) to the job, reporting each and stopping at a
    /// cancel. `Some` when the job ended early (cancelled here or outside this process); `None` when every page is
    /// in and the job is to be closed.
    fn send(
        &self,
        order: &[Option<usize>],
        cancel: &CancelToken,
        mut progress: Option<ProgressCallback<'_>>,
    ) -> Result<Option<PrintOutcome>, PrintError> {
        let total = u32::try_from(order.len()).unwrap_or(u32::MAX);
        let mut report = |done: usize| {
            if let Some(report) = progress.as_mut() {
                report(Progress::new(u32::try_from(done).unwrap_or(u32::MAX), Some(total)));
            }
        };
        let checkpoint = || {
            let requested = if cancel.is_cancelled() { ControlFlow::Break(()) } else { ControlFlow::Continue(()) };
            self.context.checkpoint(requested).map_err(|e| job_failed("printing", &e))
        };
        for (done, item) in order.iter().enumerate() {
            if let ControlFlow::Break(outcome) = checkpoint()? {
                return Ok(Some(outcome));
            }
            report(done);
            let added = match item {
                Some(index) => {
                    let (page, placement) = &self.pages[*index];
                    self.drawer.page(self.context, self.pdf, (*index, *page), placement, self.paper)
                }
                None => self.drawer.blank(self.context, self.paper),
            };
            let added = added.map_err(|e| failed(&format!("drawing page {} of {}", done + 1, order.len()), &e))?;
            if let ControlFlow::Break(outcome) = added {
                return Ok(Some(outcome));
            }
        }
        if let ControlFlow::Break(outcome) = checkpoint()? {
            return Ok(Some(outcome));
        }
        report(order.len());
        Ok(None)
    }
}

/// A job's ticket as the driver validated it, and how its pages meet the paper.
struct Plan {
    ticket: PrintTicket,
    /// The paper as the ticket orients it, in DIPs.
    paper: Size,
    fit: Fit,
    chosen: Chosen,
    /// Copies the job draws itself, when the driver cannot make them as asked.
    copies_here: Option<u32>,
    warnings: Vec<String>,
}

/// The ticket for `job` and `options` on `device`, validated by the driver and checked against what was asked;
/// `first` is the document's first page.
fn plan(device: &PrinterDevice, capabilities: &PrintCapabilities, job: &Job, options: &JobOptions, first: Size) -> Result<Plan, PrintError> {
    let name = device.name();
    let mut warnings = Vec::new();
    let exact = !matches!(job.scaling, Scaling::ShrinkToFit);

    // The paper: the size asked for, else the first page's, from the driver's own list.
    let asked = options.media.as_deref().and_then(|media| {
        let size = pwg_size(media);
        if size.is_none() {
            warnings.push(format!("{media} names no size; the paper is the document's first page's"));
        }
        size
    });
    let wanted_paper = asked.map_or(first, |(w, h)| Size::from_micron(w, h));
    let media = paper(capabilities, micron(wanted_paper));
    if media.is_none() {
        let inches = inches(wanted_paper);
        if exact {
            return Err(PrintError::new(
                ErrorKind::InvalidRequest,
                format!("{name} has no {inches} paper; print shrunk to fit, or add the size in the printer's driver"),
            ));
        }
        warnings.push(format!("the printer has no {inches} paper; the pages shrink onto its default paper"));
    }

    // The orientation: the one asked for, else the first page's where the driver offers it. Pages the other way
    // round are turned onto the paper either way.
    let offered = |wanted: PredefinedPageOrientation| PageOrientation::list(capabilities).find(|o| o.as_predefined_name() == Some(wanted));
    let explicit = options.orientation.map(|orientation| match orientation {
        Orientation::Landscape => PredefinedPageOrientation::Landscape,
        Orientation::ReversePortrait => PredefinedPageOrientation::ReversePortrait,
        Orientation::ReverseLandscape => PredefinedPageOrientation::ReverseLandscape,
        // `Portrait`, and any orientation a newer printscan adds.
        _ => PredefinedPageOrientation::Portrait,
    });
    let orientation = explicit.unwrap_or(if first.landscape() && offered(PredefinedPageOrientation::Landscape).is_some() {
        PredefinedPageOrientation::Landscape
    } else {
        PredefinedPageOrientation::Portrait
    });

    // Copies: the driver's when it makes them as asked, else drawn by the job.
    let copies = job.copies.max(1);
    let collation = if job.collate { PredefinedCollation::Collated } else { PredefinedCollation::Uncollated };
    let collation_option = Collation::list(capabilities).find(|c| c.as_predefined_name() == Some(collation));
    let drivers_copies = capabilities.max_copies().is_none_or(|max| u32::from(max.0) >= copies);
    let copies_here = (copies > 1 && !(drivers_copies && collation_option.is_some())).then_some(copies);
    let ticket_copies = if copies_here.is_some() { 1 } else { copies };

    let grey = job.grayscale || options.colour == ColourMode::Mono;
    let colour = if grey {
        Some(PredefinedPageOutputColor::Grayscale)
    } else if options.colour == ColourMode::Colour {
        Some(PredefinedPageOutputColor::Color)
    } else {
        None
    };
    let duplex = job.duplex.map(|duplex| (duplex, checks::duplex_option(duplex)));
    let colour_plan = colour_plan(name, capabilities, options, &mut warnings)?;

    // Declared before the builder, so the builder closes its provider handle before it is released.
    let _provider = print_ticket_provider();
    let mut builder = PrintTicketBuilder::new(device).map_err(|e| failed("building the print ticket", &e))?;
    let merge = |builder: &mut PrintTicketBuilder, ticket: PrintTicket| builder.merge(ticket).map_err(|e| failed("building the print ticket", &e));
    merge(&mut builder, Copies(u16::try_from(ticket_copies).unwrap_or(u16::MAX)).into())?;
    if let (true, Some(option)) = (ticket_copies > 1, collation_option) {
        merge(&mut builder, option.into())?;
    }
    match duplex {
        Some((_, Some(wanted))) => {
            if let Some(option) = JobDuplex::list(capabilities).find(|d| d.as_predefined_name() == Some(wanted)) {
                merge(&mut builder, option.into())?;
            }
        }
        Some((duplex, None)) => warnings.push(format!("{duplex:?} is left to the driver")),
        None => {}
    }
    if let Some(wanted) = colour {
        let option = PageOutputColor::list(capabilities).find(|c| {
            c.as_predefined_name() == Some(wanted)
                || (wanted == PredefinedPageOutputColor::Grayscale && c.as_predefined_name() == Some(PredefinedPageOutputColor::Monochrome))
        });
        if let Some(option) = option {
            merge(&mut builder, option.into())?;
        }
    }
    if let Some(media) = &media {
        merge(&mut builder, media.clone().into())?;
    }
    if let Some(option) = offered(orientation) {
        merge(&mut builder, option.into())?;
    }
    if let Some(resolution) = options.resolution {
        let wanted = (resolution.x_dpi, resolution.y_dpi);
        match capabilities.page_resolutions().find(|r: &PageResolution| r.dpi() == wanted) {
            Some(option) => merge(&mut builder, option.into())?,
            None => warnings.push(format!("the printer has no {}x{} dpi; it prints at its default resolution", wanted.0, wanted.1)),
        }
    }
    if let Some(option) = colour_plan.unmanaged.clone() {
        merge(&mut builder, option.into())?;
    }
    if let Some(option) = colour_plan.application_profile.clone() {
        merge(&mut builder, option.into())?;
    }
    if let Some(option) = colour_plan.intent.clone() {
        merge(&mut builder, option.into())?;
    }
    let ticket = builder.build().map_err(|e| failed("building the print ticket", &e))?;

    // What the driver kept of what was asked.
    let chosen = Chosen::read(&ticket);
    let wanted = Wanted {
        duplex: duplex.and_then(|(_, wanted)| wanted),
        collation: (ticket_copies > 1).then_some(collation),
        colour,
        orientation: explicit,
        copies: ticket_copies,
        colour_management: colour_plan.unmanaged.as_ref().and_then(|o| o.as_predefined_name()),
        destination_profile: colour_plan.application_profile.as_ref().and_then(|o| o.as_predefined_name()),
        intent: colour_plan.intent.as_ref().and_then(|o| o.as_predefined_name()),
    };
    let lost = checks::dropped(&wanted, &chosen);
    if !lost.is_empty() {
        return Err(PrintError::new(ErrorKind::Rejected, format!("{name} would not print {}; nothing was sent", lost.join(", ")))
            .with_safety(RetrySafety::Safe));
    }
    warnings.extend(checks::replaced_hints(&wanted, &chosen));

    // The paper as the driver has it, turned to the orientation it kept.
    let printable = imageable(device, &ticket);
    let paper = match (&media, printable) {
        (Some(media), _) => Size::from_micron(media.size().width_in_micron(), media.size().height_in_micron()),
        (None, Some((size, _))) => size,
        (None, None) => {
            return Err(PrintError::new(ErrorKind::Failed, format!("{name} did not say what paper it prints on")).with_safety(RetrySafety::Safe));
        }
    };
    let landscape =
        matches!(chosen.orientation.unwrap_or(orientation), PredefinedPageOrientation::Landscape | PredefinedPageOrientation::ReverseLandscape);
    let paper = if paper.landscape() == landscape { paper } else { paper.turned() };
    let fit = if exact { Fit::Exact } else { Fit::Shrink { margin: printable.map_or(0.0, |(_, margin)| margin) } };
    Ok(Plan { ticket, paper, fit, chosen, copies_here, warnings })
}

/// The colour options of a job's ticket, from the driver's own lists.
struct ColourPlan {
    /// `PageColorManagement` = `None`, for application-managed colour.
    unmanaged: Option<PageColorManagement>,
    /// `PageDestinationColorProfile` = `Application`, beside it.
    application_profile: Option<PageDestinationColorProfile>,
    /// `PageICMRenderingIntent`, for the job's rendering intent.
    intent: Option<PageIcmRenderingIntent>,
}

/// The colour options for `options` on the printer `name` (application-managed colour and the rendering intent),
/// what the driver lacks named in `warnings`. Refuses a job with application-managed colour whose driver manages
/// colour and has no setting to stop it.
fn colour_plan(name: &str, capabilities: &PrintCapabilities, options: &JobOptions, warnings: &mut Vec<String>) -> Result<ColourPlan, PrintError> {
    let mut plan = ColourPlan { unmanaged: None, application_profile: None, intent: None };
    if options.application_managed_colour {
        let management: Vec<PageColorManagement> = PageColorManagement::list(capabilities).collect();
        let profiles: Vec<PageDestinationColorProfile> = PageDestinationColorProfile::list(capabilities).collect();
        let listed = checks::application_colour(
            &management.iter().map(|o| o.as_predefined_name()).collect::<Vec<_>>(),
            &profiles.iter().map(|o| o.as_predefined_name()).collect::<Vec<_>>(),
            warnings,
        )
        .map_err(|why| {
            PrintError::new(ErrorKind::Rejected, format!("{name} would not print the document's colours as they are: {why}; nothing was sent"))
                .with_safety(RetrySafety::Safe)
        })?;
        if listed.unmanaged {
            plan.unmanaged = management.into_iter().find(|o| o.as_predefined_name() == Some(PredefinedPageColorManagement::None));
        }
        if listed.application_profile {
            plan.application_profile =
                profiles.into_iter().find(|o| o.as_predefined_name() == Some(PredefinedPageDestinationColorProfile::Application));
        }
    }
    match options.rendering_intent.map(|intent| (intent, checks::icm_intent(intent))) {
        Some((_, Some((wanted, note)))) => {
            warnings.extend(note.map(str::to_owned));
            plan.intent = PageIcmRenderingIntent::list(capabilities).find(|i| i.as_predefined_name() == Some(wanted));
            if plan.intent.is_none() {
                warnings.push(format!("the printer has no {} rendering intent; it is left to the driver", checks::intent_name(wanted)));
            }
        }
        Some((intent, None)) if intent != RenderingIntent::Auto => {
            warnings.push(format!("the {} rendering intent is left to the driver", intent.keyword()))
        }
        // No intent, or `Auto`: the driver's choice.
        _ => {}
    }
    Ok(plan)
}

/// The paper `ticket` prints on and its widest unprintable margin (DIPs), as the driver reports them for it.
fn imageable(device: &PrinterDevice, ticket: &PrintTicket) -> Option<(Size, f64)> {
    let xml = PrintCapabilities::fetch_xml_for_ticket(device, Some(ticket)).ok()?;
    let document = PrintCapabilitiesDocument::parse_from_bytes(&xml).ok()?;
    let number =
        |property: &Property, name: &str| property.get_property(name, Some(NS_PSK))?.value.as_ref()?.integer().and_then(|v| u32::try_from(v).ok());
    let size = document.get_property("PageImageableSize", Some(NS_PSK))?;
    let paper = (number(size, "ImageableSizeWidth")?, number(size, "ImageableSizeHeight")?);
    let area = size.get_property("ImageableArea", Some(NS_PSK))?;
    let origin = (number(area, "OriginWidth")?, number(area, "OriginHeight")?);
    let extent = (number(area, "ExtentWidth")?, number(area, "ExtentHeight")?);
    let margin = f64::from(widest_margin(paper, origin, extent)) * 96.0 / 25_400.0;
    Some((Size::from_micron(paper.0, paper.1), margin))
}

/// The widest of the four margins around the printable area `origin` + `extent` on `paper` (micrometres).
fn widest_margin(paper: (u32, u32), origin: (u32, u32), extent: (u32, u32)) -> u32 {
    let right = paper.0.saturating_sub(origin.0.saturating_add(extent.0));
    let bottom = paper.1.saturating_sub(origin.1.saturating_add(extent.1));
    origin.0.max(origin.1).max(right).max(bottom)
}

/// `size` in micrometres.
fn micron(size: Size) -> (u32, u32) {
    let micron = |dips: f64| (dips * 25_400.0 / 96.0).round().clamp(0.0, f64::from(u32::MAX)) as u32;
    (micron(size.w), micron(size.h))
}

/// `size` in inches, for people ("8.50 x 11.00 in").
fn inches(size: Size) -> String {
    format!("{:.2} x {:.2} in", size.w / 96.0, size.h / 96.0)
}

/// A page the paper does not take at its own size.
fn mismatch(m: Mismatch, paper: Size) -> PrintError {
    PrintError::new(
        ErrorKind::InvalidRequest,
        format!("page {} is {}, not the paper's {}; print it shrunk to fit, or as a job of its own", m.index + 1, inches(m.page), inches(paper)),
    )
}

/// Open the driver's Printing Preferences window for `printer` (`printui.dll`'s `PrintUIEntry /e`), and return once
/// it is open. What the person sets there is saved as that printer's defaults for this Windows user, which every
/// job's ticket starts from; a job's own settings still apply on top.
pub fn open_preferences(printer: &str) -> Result<(), PrintError> {
    use std::os::windows::process::CommandExt as _;
    // printui takes the name in quotes; a Windows printer name cannot hold one.
    if printer.is_empty() || printer.contains('"') || printer.chars().any(char::is_control) {
        return Err(PrintError::new(ErrorKind::InvalidRequest, "not a printer name"));
    }
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let rundll32 = std::path::Path::new(&root).join("System32").join("rundll32.exe");
    std::process::Command::new(rundll32)
        .raw_arg("printui.dll,PrintUIEntry")
        .raw_arg("/e")
        .raw_arg("/n")
        .raw_arg(format!("\"{printer}\""))
        .spawn()
        .map(drop)
        .map_err(|e| PrintError::new(ErrorKind::Unavailable, format!("the printer's preferences could not be opened: {e}")))
}

/// The paper the driver offers in `size` (micrometres, either way round).
fn paper(capabilities: &PrintCapabilities, (width, height): (u32, u32)) -> Option<PageMediaSize> {
    let near = |a: u32, b: u32| a.abs_diff(b) <= PAPER_TOLERANCE;
    capabilities.page_media_sizes().find(|m| {
        let size = m.size();
        let (w, h) = (size.width_in_micron(), size.height_in_micron());
        (near(w, width) && near(h, height)) || (near(w, height) && near(h, width))
    })
}

/// The size, in micrometres, a PWG self-describing media name carries (`iso_a4_210x297mm`,
/// `na_letter_8.5x11in`).
fn pwg_size(name: &str) -> Option<(u32, u32)> {
    let dims = name.rsplit('_').next()?;
    let (dims, micron_per_unit) = match dims.strip_suffix("mm") {
        Some(d) => (d, 1_000.0),
        None => (dims.strip_suffix("in")?, 25_400.0),
    };
    let (w, h) = dims.split_once('x')?;
    let micron = |v: &str| v.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0 && *v < 10_000.0).map(|v| (v * micron_per_unit).round() as u32);
    Some((micron(w)?, micron(h)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pwg_names_give_their_size() {
        assert_eq!(pwg_size("iso_a4_210x297mm"), Some((210_000, 297_000)));
        assert_eq!(pwg_size("na_letter_8.5x11in"), Some((215_900, 279_400)));
        assert_eq!(pwg_size("custom_100x150mm"), Some((100_000, 150_000)));
        assert_eq!(pwg_size("iso_a4"), None);
        assert_eq!(pwg_size("na_letter_8.5x11pt"), None);
        assert_eq!(pwg_size("x_0x10mm"), None);
        assert_eq!(pwg_size("x_NaNx10mm"), None);
    }

    #[test]
    fn one_thread_at_a_time_is_in_the_print_ticket_provider() {
        use std::sync::Barrier;
        use std::sync::atomic::{AtomicUsize, Ordering};
        const THREADS: usize = 12;
        let inside = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        let start = Barrier::new(THREADS);
        std::thread::scope(|s| {
            for _ in 0..THREADS {
                s.spawn(|| {
                    start.wait();
                    for _ in 0..20 {
                        let _provider = print_ticket_provider();
                        most.fetch_max(inside.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(1));
                        inside.fetch_sub(1, Ordering::SeqCst);
                    }
                });
            }
        });
        assert_eq!(most.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_panic_inside_the_print_ticket_provider_does_not_lock_it_out() {
        let panicked = std::thread::spawn(|| {
            let _provider = print_ticket_provider();
            panic!("a driver call panicked");
        })
        .join();
        assert!(panicked.is_err());
        drop(print_ticket_provider());
    }

    /// The backend lists this machine's printers and reads one's capabilities (needs a printer
    /// such as "Microsoft Print to PDF"; prints nothing).
    #[test]
    #[ignore]
    fn lists_printers_and_reads_details() {
        let printers = WindowsSpooler::default().list_printers().unwrap();
        assert!(!printers.is_empty());
        let pdf = printers.iter().find(|p| p.id == "Microsoft Print to PDF").expect("Microsoft Print to PDF is installed");
        let details = WindowsSpooler::default().printer_details(&pdf.id).unwrap();
        assert!(!details.paper.is_empty());
        assert!(!details.resolutions.is_empty());
    }
}
