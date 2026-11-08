//! The resource-level client: one method per eSCL resource this backend uses, over any [`Transport`].

use std::time::Duration;

use printscan_wsd::{CancelToken, NetworkLimits};

use crate::error::{Error, Result};
use crate::model::{self, Capabilities, Status};
use crate::settings::ScanRequest;
use crate::transport::{Method, Request, Response, Transport};
use crate::url::ScannerUrl;

/// The limits of a `NextDocument`: a page is far larger than any other reply, and the scanner may
/// still be moving its head when it answers.
pub fn document_limits() -> NetworkLimits {
    let mut limits = NetworkLimits::default();
    limits.timeout = Duration::from_secs(180);
    limits.max_response = 256 << 20;
    limits
}

/// A job a scanner created: its URL on that scanner.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Job {
    pub url: ScannerUrl,
}

/// What `GET <job>/NextDocument` answered.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NextDocument {
    /// A document (one page for image formats), as delivered.
    Document { bytes: Vec<u8>, content_type: Option<String> },
    /// 404: the job has no more documents.
    Done,
    /// 503: not ready yet; ask again after the wait the scanner named, if any.
    NotReady { retry_after: Option<Duration> },
}

/// A client bound to one scanner's eSCL root.
#[derive(Clone, Debug)]
pub struct Client<T: Transport> {
    transport: T,
    root: ScannerUrl,
    limits: NetworkLimits,
    document_limits: NetworkLimits,
    cancel: Option<CancelToken>,
}

impl<T: Transport> Client<T> {
    /// A client for the scanner whose eSCL root is `root`.
    pub fn new(transport: T, root: ScannerUrl) -> Self {
        Self { transport, root, limits: NetworkLimits::default(), document_limits: document_limits(), cancel: None }
    }

    /// Limits for every request but `NextDocument`.
    #[must_use]
    pub fn with_limits(mut self, limits: NetworkLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Limits for `NextDocument` (default [`document_limits`]).
    #[must_use]
    pub fn with_document_limits(mut self, limits: NetworkLimits) -> Self {
        self.document_limits = limits;
        self
    }

    /// Stop sending once `token` is set (a request already on the wire runs to its timeout).
    #[must_use]
    pub fn with_cancel(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// The eSCL root.
    pub fn root(&self) -> &ScannerUrl {
        &self.root
    }

    fn send(&self, request: Request) -> Result<Response> {
        if self.cancel.as_ref().is_some_and(CancelToken::is_cancelled) {
            return Err(Error::Cancelled);
        }
        self.transport.send(request)
    }

    fn get(&self, name: &str) -> Result<Vec<u8>> {
        let request = Request::new(Method::Get, self.root.join(name)?, self.limits.timeout, self.limits.max_response);
        let response = self.send(request)?;
        if response.status != 200 {
            return Err(Error::Http { status: response.status, retry_after: response.retry_after });
        }
        Ok(response.body)
    }

    /// The raw `ScannerCapabilities` document, for diagnostics.
    pub fn capabilities_document(&self) -> Result<Vec<u8>> {
        self.get("ScannerCapabilities")
    }

    /// `GET ScannerCapabilities`.
    pub fn capabilities(&self) -> Result<Capabilities> {
        model::parse_capabilities(&self.get("ScannerCapabilities")?)
    }

    /// `GET ScannerStatus`.
    pub fn status(&self) -> Result<Status> {
        model::parse_status(&self.get("ScannerStatus")?)
    }

    /// `POST ScanJobs` with `request`: the job from the reply's `Location` (201 Created).
    pub fn create_job(&self, request: &ScanRequest) -> Result<Job> {
        let url = self.root.join("ScanJobs")?;
        let call =
            Request::new(Method::Post, url.clone(), self.limits.timeout, self.limits.max_response).with_body("text/xml", request.xml().into_bytes());
        let response = self.send(call)?;
        if !(200..300).contains(&response.status) {
            return Err(Error::Http { status: response.status, retry_after: response.retry_after });
        }
        let location = response.location.ok_or_else(|| Error::Protocol("the scanner created a job without a Location".into()))?;
        Ok(Job { url: self.root.job(&location)? })
    }

    /// `GET <job>/NextDocument`.
    pub fn next_document(&self, job: &Job) -> Result<NextDocument> {
        let request = Request::new(Method::Get, job.url.join("NextDocument")?, self.document_limits.timeout, self.document_limits.max_response);
        let response = self.send(request)?;
        match response.status {
            200 if response.body.is_empty() => Err(Error::Protocol("the scanner sent an empty document".into())),
            200 => Ok(NextDocument::Document { bytes: response.body, content_type: response.content_type }),
            404 => Ok(NextDocument::Done),
            503 => Ok(NextDocument::NotReady { retry_after: response.retry_after }),
            status => Err(Error::Http { status, retry_after: response.retry_after }),
        }
    }

    /// `DELETE <job>`: end the job. A job the scanner no longer knows (404) is already ended. Sent
    /// whatever the cancel token says, since it is what a cancel does.
    pub fn cancel(&self, job: &Job) -> Result<()> {
        let request = Request::new(Method::Delete, job.url.clone(), self.limits.timeout, self.limits.max_response);
        let response = self.transport.send(request)?;
        match response.status {
            200..=299 | 404 => Ok(()),
            status => Err(Error::Http { status, retry_after: response.retry_after }),
        }
    }
}
