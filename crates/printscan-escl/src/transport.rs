//! The injectable HTTP layer.
//!
//! The client never opens a socket itself: it hands a [`Request`] to a [`Transport`]. A transport
//! only ever sees a validated [`ScannerUrl`] and must honour the policy in the request: no proxy, no
//! redirects (refused as [`crate::TransportKind::Redirect`]), the timeout, and a body no larger than
//! `max_response`. A non-success status is a [`Response`], not an error.

use std::time::Duration;

use crate::error::Error;
use crate::url::ScannerUrl;

/// The HTTP methods eSCL uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Method {
    Get,
    Post,
    Delete,
}

impl Method {
    /// The method's name on the wire.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Delete => "DELETE",
        }
    }
}

/// One HTTP exchange.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Request {
    pub method: Method,
    pub url: ScannerUrl,
    /// The `Content-Type` of `body`; `None` for a request without one.
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub timeout: Duration,
    /// Read at most this many body bytes; more is [`Error::TooLarge`].
    pub max_response: usize,
}

impl Request {
    /// A `method` request to `url` with no body, answered within `timeout` with at most
    /// `max_response` body bytes.
    pub fn new(method: Method, url: ScannerUrl, timeout: Duration, max_response: usize) -> Self {
        Self { method, url, content_type: None, body: Vec::new(), timeout, max_response }
    }

    /// The same request carrying `body` as `content_type`.
    #[must_use]
    pub fn with_body(mut self, content_type: impl Into<String>, body: Vec<u8>) -> Self {
        self.content_type = Some(content_type.into());
        self.body = body;
        self
    }
}

/// The reply: status, the headers eSCL needs and the (capped) body.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct Response {
    pub status: u16,
    pub content_type: Option<String>,
    /// The `Location` header (a created job).
    pub location: Option<String>,
    /// The wait the scanner asked for with `Retry-After`, parsed and capped.
    pub retry_after: Option<Duration>,
    pub body: Vec<u8>,
}

impl Response {
    /// A reply with `status` and `body` and no headers.
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self { status, body, ..Self::default() }
    }

    /// Sets [`Self::content_type`].
    #[must_use]
    pub fn with_content_type(mut self, value: impl Into<String>) -> Self {
        self.content_type = Some(value.into());
        self
    }

    /// Sets [`Self::location`].
    #[must_use]
    pub fn with_location(mut self, value: impl Into<String>) -> Self {
        self.location = Some(value.into());
        self
    }

    /// Sets [`Self::retry_after`].
    #[must_use]
    pub fn with_retry_after(mut self, value: Option<Duration>) -> Self {
        self.retry_after = value;
        self
    }
}

/// Something that can send a request to a scanner.
pub trait Transport {
    /// Send `request`. Redirects must be refused, proxies must not be used, and the exchange must
    /// time out. A non-success status is a reply, not an error.
    fn send(&self, request: Request) -> Result<Response, Error>;
}

impl<T: Transport + ?Sized> Transport for &T {
    fn send(&self, request: Request) -> Result<Response, Error> {
        (**self).send(request)
    }
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn send(&self, request: Request) -> Result<Response, Error> {
        (**self).send(request)
    }
}

impl<T: Transport + ?Sized> Transport for std::sync::Arc<T> {
    fn send(&self, request: Request) -> Result<Response, Error> {
        (**self).send(request)
    }
}
