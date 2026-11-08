//! [`CupsBackend`]: the queues of a CUPS scheduler as a registry backend.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use printscan::print::backends::{Backend, BackendClass, DocumentFormat, PrinterDetails, PrinterRef, SubmitRequest};
use printscan::print::{
    ErrorKind, Fingerprint, JobFilter, JobId, JobQueryCaps, JobStatus, Phase, PrintError, Reconciled, RemoteJob, Submitted, TagSupport, WatchSupport,
};
use printscan_ipp::adapter::{SharedTransport, Transport, default_transport};
use printscan_ipp::codec::Attribute;
use printscan_ipp::{Client, IppBackend, IppVersion, NetworkLimits, Op, PrinterUri, StatusCode};

use super::queue::{check_queue_name, encode_segment};
use super::scheduler::{self, CUPS_GET_DEFAULT, CUPS_GET_PRINTERS, Queue};

/// The registry name and id prefix.
const NAME: &str = "cups";

/// Where the scheduler is unless the app says otherwise: CUPS listens on `localhost:631`.
pub const DEFAULT_SERVER: &str = "ipp://127.0.0.1:631/";

/// What the last listing said about a queue that the URI and the details need.
#[derive(Clone, Debug, Default)]
struct Known {
    is_class: bool,
    info: Option<String>,
}

#[derive(Clone, Debug)]
struct Config {
    user_name: String,
    limits: NetworkLimits,
    queue_limit: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self { user_name: "printscan".into(), limits: NetworkLimits::default(), queue_limit: 1000 }
    }
}

/// A CUPS backend. Registers as `cups`; its printers are the scheduler's queues, named by queue name
/// (`cups:<queue>`), and its jobs `cups:<queue>#<job id>` (see [`CupsBackend::job_id`]). See the
/// [crate documentation](crate) for what it does and does not do.
///
/// The backend is cheap to clone and clones share what they learned of the queues (which ones are
/// classes). Configuration is set before it is cloned; a `with_*` method makes an independent variant.
#[derive(Clone)]
pub struct CupsBackend {
    server: PrinterUri,
    transport: SharedTransport,
    /// Prints, follows and cancels; its printers are the queue URIs this backend has used.
    ipp: IppBackend,
    config: Arc<Config>,
    known: Arc<RwLock<HashMap<String, Known>>>,
    counter: Arc<AtomicU32>,
}

impl fmt::Debug for CupsBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CupsBackend").field("server", &self.server.canonical()).field("config", &self.config).finish_non_exhaustive()
    }
}

impl Default for CupsBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// The queue and job number a `cups` job id names; see [`CupsBackend::parse_native`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct CupsJob {
    pub queue: String,
    pub job_number: u32,
}

/// `<queue>#<job number>`. A queue name never contains `#`.
fn job_native(queue: &str, job_number: u32) -> String {
    format!("{queue}#{job_number}")
}

/// A job number that is a valid IPP `job-id` (1 to 2147483647).
fn valid_job_number(n: u32) -> bool {
    n != 0 && i32::try_from(n).is_ok()
}

fn invalid(message: String) -> PrintError {
    PrintError::new(ErrorKind::InvalidRequest, message).with_phase(Phase::Prepare)
}

impl CupsBackend {
    /// A backend for the scheduler at [`DEFAULT_SERVER`] with the plain-HTTP transport. Opens no
    /// connection.
    pub fn new() -> Self {
        let transport = default_transport();
        Self {
            server: PrinterUri::parse(DEFAULT_SERVER).expect("the default server URI passes the printer URI policy"),
            ipp: IppBackend::new().with_transport(Arc::clone(&transport)),
            transport,
            config: Arc::default(),
            known: Arc::default(),
            counter: Arc::new(AtomicU32::new(1)),
        }
    }

    /// Talk to the scheduler at `uri` (`ipp://<numeric address>[:port]/` or `http://`, port 631 when
    /// none is named) instead of [`DEFAULT_SERVER`]. The address follows `printscan-ipp`'s printer URI
    /// policy (numeric loopback, link-local or private); the path must be `/`. Opens no connection.
    pub fn with_server(mut self, uri: &str) -> Result<Self, PrintError> {
        let server = PrinterUri::parse(uri).map_err(PrintError::from)?;
        if server.path() != "/" {
            return Err(invalid(format!("{uri:?} is not a CUPS scheduler URI: name the scheduler's root (ipp://<address>:<port>/), not a queue")));
        }
        self.server = server;
        Ok(self)
    }

    /// Use another transport (a recorder, a test double). The transport is only ever given validated
    /// URIs on the scheduler's address.
    #[must_use]
    pub fn with_transport(mut self, transport: impl Transport + Send + Sync + 'static) -> Self {
        let transport: SharedTransport = Arc::new(transport);
        self.ipp = self.ipp.with_transport(Arc::clone(&transport));
        self.transport = transport;
        self
    }

    /// The `requesting-user-name` sent with requests, and so the owner CUPS records for jobs.
    /// Defaults to a fixed `printscan`, never the account name.
    #[must_use]
    pub fn with_user_name(mut self, name: &str) -> Self {
        self.ipp = self.ipp.with_user_name(name);
        self.configured(|c| c.user_name = name.to_owned())
    }

    /// Timeout and size caps per request.
    #[must_use]
    pub fn with_limits(mut self, limits: NetworkLimits) -> Self {
        self.ipp = self.ipp.with_limits(limits);
        self.configured(|c| c.limits = limits)
    }

    /// The poll interval this backend recommends for [`Backend::job_status`] (default two seconds).
    #[must_use]
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.ipp = self.ipp.with_poll_interval(interval);
        self
    }

    /// The most queues one listing returns (default 1000).
    #[must_use]
    pub fn with_queue_limit(self, limit: u32) -> Self {
        self.configured(|c| c.queue_limit = limit.max(1))
    }

    /// Configure the IPP backend that prints, follows and cancels jobs on the queues: strict mode,
    /// the job flow, `Validate-Job` first, the IPP version and so on (see [`IppBackend`]). Call it
    /// after [`Self::with_transport`], [`Self::with_user_name`] and [`Self::with_limits`] only to
    /// override them for jobs.
    #[must_use]
    pub fn with_ipp(mut self, configure: impl FnOnce(IppBackend) -> IppBackend) -> Self {
        self.ipp = configure(self.ipp);
        self
    }

    /// The scheduler's canonical URI (`ipp://127.0.0.1:631/` by default).
    pub fn server(&self) -> String {
        self.server.canonical()
    }

    fn configured(mut self, change: impl FnOnce(&mut Config)) -> Self {
        let mut config = (*self.config).clone();
        change(&mut config);
        self.config = Arc::new(config);
        self
    }

    /// An IPP client for the scheduler's root: IPP 2.0, falling back to 1.1 once when refused.
    fn scheduler(&self) -> Client<SharedTransport> {
        Client::new(Arc::clone(&self.transport), self.server.clone())
            .with_limits(self.config.limits)
            .with_user(&self.config.user_name)
            .with_version(IppVersion::V2_0)
            .with_fallback(true)
    }

    /// The next request id; 0 is skipped, since the protocol reserves it.
    fn next_request_id(&self) -> u32 {
        loop {
            let id = self.counter.fetch_add(1, Ordering::Relaxed);
            if id != 0 {
                return id;
            }
        }
    }

    /// Send one scheduler operation and read the queues of its reply.
    fn scheduler_queues(&self, code: u16, extra: Vec<Attribute>, limit: usize) -> Result<Vec<Queue>, printscan_ipp::IppError> {
        let client = self.scheduler();
        let message = scheduler::request(client.version().parts(), code, self.next_request_id(), &self.config.user_name, extra);
        let reply = client.exchange(&message)?;
        let queues = scheduler::queues(&reply.message, limit);
        let mut known = self.known.write().unwrap_or_else(PoisonError::into_inner);
        for queue in &queues {
            known.insert(queue.name.clone(), Known { is_class: queue.is_class, info: queue.info.clone() });
        }
        Ok(queues)
    }

    /// `CUPS-Get-Printers`: every printer and class of the scheduler, in its order, at most
    /// [`Self::with_queue_limit`]. The default queue is not marked here (see [`Self::default_queue`]).
    /// A queue whose name this backend could not use as an id is left out.
    pub fn queues(&self) -> Result<Vec<Queue>, PrintError> {
        let limit = self.config.queue_limit;
        self.scheduler_queues(
            CUPS_GET_PRINTERS,
            vec![Attribute::integer("limit", i32::try_from(limit).unwrap_or(i32::MAX))],
            usize::try_from(limit).unwrap_or(usize::MAX),
        )
        .map_err(|e| e.print_error(Op::Query))
    }

    /// `CUPS-Get-Default`: the scheduler's default queue, or `None` when it has none
    /// (`client-error-not-found`).
    pub fn default_queue(&self) -> Result<Option<Queue>, PrintError> {
        match self.scheduler_queues(CUPS_GET_DEFAULT, Vec::new(), 1) {
            Ok(queues) => Ok(queues.into_iter().next().map(|mut q| {
                q.default = true;
                q
            })),
            Err(e) if e.status() == Some(StatusCode::NOT_FOUND) => Ok(None),
            Err(e) => Err(e.print_error(Op::Query)),
        }
    }

    /// The IPP URI of a queue on this scheduler: `ipp://<server>/printers/<name>`, or
    /// `/classes/<name>` for a queue the last listing named as a class, with the name
    /// percent-encoded. [`ErrorKind::InvalidRequest`] for a name that is not a queue name.
    pub fn queue_uri(&self, queue: &str) -> Result<String, PrintError> {
        self.queue_printer_uri(queue).map(|u| u.canonical())
    }

    fn queue_printer_uri(&self, queue: &str) -> Result<PrinterUri, PrintError> {
        check_queue_name(queue)?;
        let is_class = self.known.read().unwrap_or_else(PoisonError::into_inner).get(queue).is_some_and(|k| k.is_class);
        let root = self.server.canonical();
        let collection = if is_class { "classes" } else { "printers" };
        PrinterUri::parse(&format!("{}/{collection}/{}", root.trim_end_matches('/'), encode_segment(queue))).map_err(PrintError::from)
    }

    /// The queue's URI, added to the IPP backend (which only talks to printers it was given).
    fn target(&self, queue: &str) -> Result<String, PrintError> {
        let uri = self.queue_printer_uri(queue)?.canonical();
        self.ipp.add(&uri)
    }

    /// The job id of job number `job_number` on `queue`: the typed way to name a job this backend
    /// made, so nothing concatenates strings. The queue name must be one ([`ErrorKind::InvalidRequest`]
    /// otherwise) and the job number a valid IPP `job-id` (1 to 2147483647). The inverse is
    /// [`Self::parse_native`] (or [`Self::parse_job_id`]).
    pub fn job_id(queue: &str, job_number: u32) -> Result<JobId, PrintError> {
        check_queue_name(queue)?;
        if !valid_job_number(job_number) {
            return Err(invalid(format!("{job_number} is not a valid IPP job id (1 to 2147483647)")));
        }
        Ok(JobId::new(NAME, job_native(queue, job_number)))
    }

    /// Split the native part of a `cups` [`JobId`] (`<queue>#<job number>`). A malformed handle is
    /// [`ErrorKind::InvalidRequest`].
    pub fn parse_native(native: &str) -> Result<CupsJob, PrintError> {
        let bad = |why: &str| invalid(format!("{native:?} is not a cups job handle: {why}"));
        let (queue, number) = native.rsplit_once('#').ok_or_else(|| bad("no job number"))?;
        let number: u32 = number.parse().map_err(|_| bad("the job number is not a number"))?;
        if !valid_job_number(number) {
            return Err(bad("the job number is out of range"));
        }
        check_queue_name(queue).map_err(|e| bad(e.message()))?;
        Ok(CupsJob { queue: queue.to_owned(), job_number: number })
    }

    /// [`Self::parse_native`] for a whole [`JobId`]; one of another backend is [`ErrorKind::InvalidRequest`].
    pub fn parse_job_id(job: &JobId) -> Result<CupsJob, PrintError> {
        if job.backend() != NAME {
            return Err(invalid(format!("{job} is not a job of the cups backend")));
        }
        Self::parse_native(job.native())
    }

    /// The IPP backend's id of a `cups` job.
    fn ipp_job(&self, job: &JobId) -> Result<(CupsJob, JobId), PrintError> {
        let parsed = Self::parse_job_id(job)?;
        let uri = self.target(&parsed.queue)?;
        let ipp = IppBackend::job_id(&uri, parsed.job_number)?;
        Ok((parsed, ipp))
    }

    /// The `cups` id of a job the IPP backend reported on `queue`.
    fn cups_job(queue: &str, ipp: &JobId) -> Result<JobId, PrintError> {
        let job = IppBackend::parse_job_id(ipp)?;
        Self::job_id(queue, job.job_number)
    }

    fn remap(queue: &str, mut job: RemoteJob) -> Result<RemoteJob, PrintError> {
        job.id = Self::cups_job(queue, &job.id)?;
        Ok(job)
    }
}

impl Backend for CupsBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    /// Jobs go to the scheduler over IPP and may leave the machine (a queue can forward to a network
    /// printer), so the registry asks for a [`printscan::print::UserConfirmed`] for every job.
    fn class(&self) -> BackendClass {
        BackendClass::Network
    }

    fn accepted_formats(&self) -> &[DocumentFormat] {
        self.ipp.accepted_formats()
    }

    /// Each queue lists its own formats (`document-format-supported`, which includes what the
    /// scheduler's filters convert); the registry reads them with one `Get-Printer-Attributes`.
    fn formats_per_printer(&self) -> bool {
        true
    }

    /// The scheduler's queues: one `CUPS-Get-Printers`, and one `CUPS-Get-Default` to mark the
    /// default (no queue is marked when that read fails). The name is `printer-info`, else the
    /// queue name.
    fn list_printers(&self) -> Result<Vec<PrinterRef>, PrintError> {
        let queues = self.queues()?;
        let default = self.default_queue().ok().flatten().map(|q| q.name);
        Ok(queues
            .into_iter()
            .map(|q| {
                let mut printer = PrinterRef::new(q.name.clone(), q.display_name());
                printer.default = default.as_deref() == Some(q.name.as_str());
                printer.two_sided = q.two_sided;
                printer
            })
            .collect())
    }

    /// One `Get-Printer-Attributes` on the queue, read as `printscan-ipp` reads a printer, and one
    /// `CUPS-Get-Default` for [`PrinterDetails::default`] (left `false` when that read fails). The
    /// name is the `printer-info` of the last listing when there was one.
    fn printer_details(&self, native_id: &str) -> Result<PrinterDetails, PrintError> {
        let uri = self.target(native_id)?;
        let mut details = self.ipp.printer_details(&uri)?;
        details.id = native_id.to_owned();
        let info = self.known.read().unwrap_or_else(PoisonError::into_inner).get(native_id).and_then(|k| k.info.clone());
        if let Some(info) = info.filter(|i| !i.trim().is_empty()) {
            details.name = info;
        }
        details.default = self.default_queue().ok().flatten().is_some_and(|q| q.name == native_id);
        Ok(details)
    }

    /// Print to the queue named by `request.job.printer`, or to the scheduler's default queue
    /// (one `CUPS-Get-Default`) when it names none. The job itself is `printscan-ipp`'s submit on
    /// the queue's URI: attributes, an optional `Validate-Job`, then `Print-Job` (or `Create-Job`
    /// and `Send-Document`); it returns as soon as the scheduler has accepted the job.
    fn submit(&self, request: SubmitRequest<'_>) -> Result<Submitted, PrintError> {
        let mut request = request;
        if request.consent.is_none() {
            return Err(PrintError::new(
                ErrorKind::ConsentRequired,
                "the cups backend hands jobs to the print scheduler, which may send them over a network, and needs the person's confirmation (UserConfirmed)",
            ));
        }
        let queue = match request.job.printer.take() {
            Some(queue) => queue,
            None => self
                .default_queue()?
                .map(|q| q.name)
                .ok_or_else(|| PrintError::printer_not_found("the CUPS scheduler has no default queue: name one (cups:<queue>)"))?,
        };
        request.job.printer = Some(self.target(&queue)?);
        let mut submitted = self.ipp.submit(request)?;
        submitted.backend = NAME;
        if let Some(job) = submitted.job.take() {
            let number = IppBackend::parse_job_id(&job)?.job_number;
            let job = Self::job_id(&queue, number)?;
            let mut message = format!("job {number} on the CUPS queue {queue} was accepted");
            if !submitted.warnings.is_empty() {
                message.push_str(&format!(" (warnings: {})", submitted.warnings.join("; ")));
            }
            submitted.message = message;
            submitted.job = Some(job);
        }
        Ok(submitted)
    }

    /// Callers poll; the scheduler's event subscriptions are not used.
    fn watch_support(&self) -> WatchSupport {
        self.ipp.watch_support()
    }

    fn recommended_poll_interval(&self) -> Option<Duration> {
        self.ipp.recommended_poll_interval()
    }

    fn open_job_timeout(&self) -> Option<Duration> {
        self.ipp.open_job_timeout()
    }

    /// One `Get-Job-Attributes`. A job the scheduler no longer lists (CUPS forgets finished jobs
    /// after its history limit) is [`ErrorKind::NotFound`].
    fn job_status(&self, job: &JobId) -> Result<JobStatus, PrintError> {
        let (_, ipp) = self.ipp_job(job)?;
        self.ipp.job_status(&ipp)
    }

    /// `Cancel-Job`. A job that already ended is not an error. CUPS may answer that it needs
    /// authentication (its default policy lets only the job's owner, or an administrator, cancel).
    fn cancel_job(&self, job: &JobId) -> Result<(), PrintError> {
        let (_, ipp) = self.ipp_job(job)?;
        self.ipp.cancel_job(&ipp)
    }

    /// The tag is a suffix of `job-name`, which CUPS lists back.
    fn tag_support(&self) -> TagSupport {
        self.ipp.tag_support()
    }

    fn job_query_caps(&self) -> JobQueryCaps {
        self.ipp.job_query_caps()
    }

    /// `Get-Jobs` on the queue, active and completed (see `printscan-ipp`), with `cups` job ids.
    fn list_jobs(&self, printer_native: &str, filter: &JobFilter) -> Result<Vec<RemoteJob>, PrintError> {
        let uri = self.target(printer_native)?;
        self.ipp.list_jobs(&uri, filter)?.into_iter().map(|job| Self::remap(printer_native, job)).collect()
    }

    /// Reconcile by tag, else by title, this backend's user name and time (see `printscan-ipp`).
    fn find_job(&self, printer_native: &str, fingerprint: &Fingerprint) -> Result<Reconciled, PrintError> {
        let uri = self.target(printer_native)?;
        Ok(match self.ipp.find_job(&uri, fingerprint)? {
            Reconciled::Found(job) => Reconciled::Found(Self::remap(printer_native, job)?),
            Reconciled::Ambiguous(jobs) => {
                Reconciled::Ambiguous(jobs.into_iter().map(|job| Self::remap(printer_native, job)).collect::<Result<_, _>>()?)
            }
            // NotFound, and any outcome added later, carries no job to rename.
            other => other,
        })
    }
}
