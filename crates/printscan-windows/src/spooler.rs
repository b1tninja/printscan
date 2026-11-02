//! The Windows print spooler as a [`Backend`], on [`winprint`].
//!
//! Printers come from the spooler (local queues and `\\server\queue` connections). A PDF job is
//! drawn by the in-box `Windows.Data.Pdf` into the XPS print API at the resolution of the job's
//! PrintTicket, in this process: no other program is started. The job's settings become the
//! PrintTicket: copies, collation, two-sided, colour, paper, orientation and resolution, each
//! taken from the options the driver offers. A setting the driver does not offer is left to the
//! driver and named in [`Submitted::warnings`].
//!
//! The spooler gives no handle for the job, so the backend reports it accepted once the document
//! is in the queue ([`WatchSupport::None`]).

use std::ops::ControlFlow;
use std::sync::{Mutex, MutexGuard, PoisonError};

use printscan::print::backends::{Backend, BackendClass, DocumentFormat, ErrorKind, JobHint, PrintError, PrinterDetails, PrinterRef, SubmitRequest};
use printscan::print::{ColourMode, Duplex, Job, JobOptions, Orientation, Progress, Resolution, RetrySafety, Submitted, WatchSupport};
use winprint::printer::{FilePrinter as _, PrintOutcome, PrinterDevice, WinPdfPrinter};
use winprint::ticket::{
    Collation, Copies, FeatureOptionPack as _, FeatureOptionPackWithPredefined as _, JobDuplex, PageMediaSize, PageOrientation, PageOutputColor,
    PageResolution, PredefinedCollation, PredefinedDuplexType, PredefinedPageOrientation, PredefinedPageOutputColor, PrintCapabilities, PrintTicket,
    PrintTicketBuilder, StaticFeaturePathOptionPack as _,
};

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

/// The Windows print spooler. Register it with a [`printscan::print::Registry`].
#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsSpooler;

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
        details.honoured = Some(vec![
            JobHint::Copies,
            JobHint::Collate,
            JobHint::Duplex,
            JobHint::Colour,
            JobHint::Media,
            JobHint::Orientation,
            JobHint::Resolution,
        ]);
        Ok(details)
    }

    fn submit(&self, request: SubmitRequest<'_>) -> Result<Submitted, PrintError> {
        let device = device(request.job.printer.as_deref())?;
        let capabilities = capabilities(&device)?;
        let (ticket, warnings) = ticket(&device, &capabilities, &request.job, &request.options)?;
        // A closed file in a private folder: Windows.Data.Pdf opens the file itself, and an open
        // temporary file would be locked against it.
        let staging = |e: std::io::Error| PrintError::new(ErrorKind::Io, format!("staging the document: {e}")).with_safety(RetrySafety::Safe);
        let folder = tempfile::Builder::new().prefix("printscan-").tempdir().map_err(staging)?;
        let path = folder.path().join("document.pdf");
        std::fs::write(&path, request.document.bytes).map_err(staging)?;

        let cancel = request.cancel.clone();
        let mut report = request.progress;
        // winprint turns the ticket into a DEVMODE through the provider before the first progress
        // report, which releases the guard; a job that fails before it releases it on return.
        let mut provider = Some(print_ticket_provider());
        let outcome = WinPdfPrinter::new(device.clone())
            .print_with_progress(&path, ticket, &mut |progress| {
                drop(provider.take());
                if let Some(report) = report.as_mut() {
                    report(Progress::new(progress.current(), Some(progress.total())));
                }
                if cancel.is_cancelled() { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
            })
            .map_err(|e| failed("printing", &e))?;
        match outcome {
            PrintOutcome::Completed => {
                let mut submitted = Submitted::new(NAME, format!("the document is in the queue of {}", device.name()));
                submitted.warnings = warnings;
                Ok(submitted)
            }
            PrintOutcome::Canceled => Err(PrintError::new(ErrorKind::Cancelled, "the job was cancelled").with_safety(RetrySafety::Safe)),
        }
    }

    fn watch_support(&self) -> WatchSupport {
        WatchSupport::None
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

/// The PrintTicket for `job` and `options`, from the options the driver offers, and the settings
/// it could not carry.
fn ticket(
    device: &PrinterDevice,
    capabilities: &PrintCapabilities,
    job: &Job,
    options: &JobOptions,
) -> Result<(PrintTicket, Vec<String>), PrintError> {
    // Declared before the builder, so the builder closes its provider handle before it is released.
    let _provider = print_ticket_provider();
    let mut builder = PrintTicketBuilder::new(device).map_err(|e| failed("building the print ticket", &e))?;
    let mut warnings = Vec::new();
    let merge = |builder: &mut PrintTicketBuilder, ticket: PrintTicket| builder.merge(ticket).map_err(|e| failed("building the print ticket", &e));

    merge(&mut builder, Copies(u16::try_from(job.copies.max(1)).unwrap_or(u16::MAX)).into())?;
    if job.copies > 1 {
        let wanted = if job.collate { PredefinedCollation::Collated } else { PredefinedCollation::Uncollated };
        match Collation::list(capabilities).find(|c| c.as_predefined_name() == Some(wanted)) {
            Some(collation) => merge(&mut builder, collation.into())?,
            None => warnings.push("collation is left to the driver".to_owned()),
        }
    }
    if let Some(duplex) = job.duplex {
        let wanted = match duplex {
            Duplex::Off => Some(PredefinedDuplexType::OneSided),
            Duplex::LongEdge => Some(PredefinedDuplexType::TwoSidedLongEdge),
            Duplex::ShortEdge => Some(PredefinedDuplexType::TwoSidedShortEdge),
            _ => None,
        };
        match wanted.and_then(|wanted| JobDuplex::list(capabilities).find(|d| d.as_predefined_name() == Some(wanted))) {
            Some(option) => merge(&mut builder, option.into())?,
            None => warnings.push(format!("the printer does not offer {duplex:?}; it prints with its default")),
        }
    }
    let grey = job.grayscale || options.colour == ColourMode::Mono;
    let colour = if grey {
        Some(PredefinedPageOutputColor::Grayscale)
    } else if options.colour == ColourMode::Colour {
        Some(PredefinedPageOutputColor::Color)
    } else {
        None
    };
    if let Some(wanted) = colour {
        let offered = PageOutputColor::list(capabilities).find(|c| {
            c.as_predefined_name() == Some(wanted)
                || (wanted == PredefinedPageOutputColor::Grayscale && c.as_predefined_name() == Some(PredefinedPageOutputColor::Monochrome))
        });
        match offered {
            Some(option) => merge(&mut builder, option.into())?,
            None => warnings.push(format!("the printer does not offer {wanted:?}")),
        }
    }
    if let Some(media) = options.media.as_deref() {
        match pwg_size(media).and_then(|size| paper(capabilities, size)) {
            Some(option) => merge(&mut builder, option.into())?,
            None => warnings.push(format!("the printer has no {media}; it prints on its default paper")),
        }
    }
    if let Some(orientation) = options.orientation {
        let wanted = match orientation {
            Orientation::Portrait => PredefinedPageOrientation::Portrait,
            Orientation::Landscape => PredefinedPageOrientation::Landscape,
            Orientation::ReversePortrait => PredefinedPageOrientation::ReversePortrait,
            Orientation::ReverseLandscape => PredefinedPageOrientation::ReverseLandscape,
            _ => PredefinedPageOrientation::Portrait,
        };
        match PageOrientation::list(capabilities).find(|o| o.as_predefined_name() == Some(wanted)) {
            Some(option) => merge(&mut builder, option.into())?,
            None => warnings.push(format!("the printer does not offer {wanted:?}")),
        }
    }
    if let Some(resolution) = options.resolution {
        let wanted = (resolution.x_dpi, resolution.y_dpi);
        match capabilities.page_resolutions().find(|r: &PageResolution| r.dpi() == wanted) {
            Some(option) => merge(&mut builder, option.into())?,
            None => warnings.push(format!("the printer has no {}x{} dpi; it prints at its default resolution", wanted.0, wanted.1)),
        }
    }
    if options.rendering_intent.is_some() {
        warnings.push("the rendering intent is left to the driver".to_owned());
    }
    let ticket = builder.build().map_err(|e| failed("building the print ticket", &e))?;
    Ok((ticket, warnings))
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
        let printers = WindowsSpooler.list_printers().unwrap();
        assert!(!printers.is_empty());
        let pdf = printers.iter().find(|p| p.id == "Microsoft Print to PDF").expect("Microsoft Print to PDF is installed");
        let details = WindowsSpooler.printer_details(&pdf.id).unwrap();
        assert!(!details.paper.is_empty());
        assert!(!details.resolutions.is_empty());
    }
}
