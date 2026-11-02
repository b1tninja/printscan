//! Printing: a [`Registry`] of print backends, the neutral types a backend speaks
//! ([`backends::Backend`]), tracked jobs ([`PrintJob`]), document conversion and job watching.
//!
//! This module holds no backend of its own. Network backends are their own crates
//! (`printscan-wsd` for WS-Print, `printscan-ipp` for IPP); an app registers the system spooler
//! or any other backend it has.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod backend;
mod clock;
pub mod convert;
mod job;
mod options;
pub mod pwg;
mod registry;
mod remote;
mod retry;
mod tag;
#[cfg(feature = "testing")]
pub mod testing;
mod tracked;
mod wake;
mod watch;

/// The [`Backend`](backends::Backend) trait and the neutral types a backend speaks.
pub mod backends {
    pub use super::backend::*;
}
/// The job vocabulary shared with scanning, under the names print code uses.
pub use crate::job::{
    BackendName, DeviceId, DocumentRef, ExpectedTotals, Fingerprint, HRESULT_DOMAIN, Jitter, JobDocuments, JobId, JobKind, JobState, JobTimes,
    JobView, NativeCode, ParseIdError, ParseIdErrorKind, Progress, RetryAdvice, RetryFacts, RetryPolicy, RetrySafety, Tag, TagError, TagPlacement,
    TagSupport, Timestamp, advise, namespaced_id, split_id,
};
pub use backend::{
    BackendClass, CancelToken, Document, DocumentFormat, ErrorKind, IccProfile, JobHint, NotFoundSubject, PrintError, PrinterDetails, PrinterRef,
    PrinterState, PrinterStatus, ProgressCallback, Receipt, SubmitRequest, UserConfirmed,
};
pub use convert::{Conversion, ConversionPolicy, ConvertContext, DocumentConverter, OwnedDocument, choose_raster_type, choose_resolution};
pub use job::{ExpectedTotalsExt, JobStatus, Submitted, WatchSupport};
pub use options::{ColourMode, JobOptions, Orientation, Quality, RenderingIntent, Resolution};
pub use pwg::RasterType;
pub use registry::{
    BackendError, BackendInfo, Listing, RegisterError, Registry, cancel_job, job_status, list_printers, printer_details, register, registry,
    set_converter, submit_job, submit_to_backend,
};
pub use remote::{FingerprintExt, JobFilter, JobQueryCaps, Reconciled, RemoteJob, filter_jobs, reconcile_jobs, title_matches};
pub use retry::{CANCEL_SLICE, Clock, JitterExt, ManualClock, Phase, SystemClock, advise_error, retry, retry_facts};
pub use tag::{TagExt, TagPolicy, TagSupportExt};
pub use tracked::{
    Advance, Attempt, AttemptError, AttemptOutcome, BytesSource, DocumentInfo, DocumentSource, FnSource, JobProgress, LifecycleState, LocalJobId,
    Outcome, PrintJob, PrintJobConfig, RequestRecord, SimpleState, Snapshot, StaleReason, Staleness, TrailEntry, TrailEvent,
};
pub use wake::{WakeReceiver, WakeSender, wake_channel};
pub use watch::{JobSource, JobWatch, WaitPolicy, WatchOutcome, WatchStep, wait_for_job};

/// One of a printer driver's own job options, such as the paper tray.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PrinterOption {
    /// The driver's keyword (`InputSlot`).
    pub key: String,
    /// What the driver calls it (`Paper tray`); the keyword when it gives no name.
    pub label: String,
    /// The driver's group (`Paper Handling`); empty outside a group.
    pub group: String,
    /// The choices: keyword and label (`Tray2`, `Tray 2`).
    pub choices: Vec<(String, String)>,
    /// The choice the printer uses when a job doesn't say.
    pub default: String,
}

/// Which sides of the sheet a job prints on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Duplex {
    /// One side.
    #[default]
    Off,
    /// Both sides, turned on the long edge.
    LongEdge,
    /// Both sides, turned on the short edge.
    ShortEdge,
}

/// How a page is fitted to the paper.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Scaling {
    /// The page's own size, even where the printer's margins clip it.
    #[default]
    Exact,
    /// Oversized pages shrink to the paper; pages that fit keep their size.
    ShrinkToFit,
}

/// A job's print settings: where it goes and how many copies, sides and colours.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Job {
    /// The printer, or `None` for the default one.
    pub printer: Option<String>,
    /// How many copies.
    pub copies: u32,
    /// Whether the copies come out collated.
    pub collate: bool,
    /// The sides to print on, or `None` to keep the printer's default.
    pub duplex: Option<Duplex>,
    /// How pages are fitted to the paper.
    pub scaling: Scaling,
    /// Print in grey.
    pub grayscale: bool,
    /// The job's name in the queue.
    pub title: String,
    /// The driver's own options, as a keyword and the chosen value (such as a tray).
    pub options: Vec<(String, String)>,
}

setters!(Job {
    some_into with_printer => printer: String,
    set with_copies => copies: u32,
    set with_collate => collate: bool,
    some with_duplex => duplex: Duplex,
    set with_scaling => scaling: Scaling,
    set with_grayscale => grayscale: bool,
    into with_title => title: String,
    set with_options => options: Vec<(String, String)>,
});

impl Job {
    /// The default job (see [`Default`]) for `printer`.
    pub fn new(printer: impl Into<String>) -> Self {
        Self::default().with_printer(printer)
    }
}

impl Default for Job {
    fn default() -> Self {
        Self {
            printer: None,
            copies: 1,
            collate: true,
            duplex: None,
            scaling: Scaling::Exact,
            grayscale: false,
            title: "printscan".into(),
            options: Vec::new(),
        }
    }
}
