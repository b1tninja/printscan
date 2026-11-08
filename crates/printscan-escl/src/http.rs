//! The blocking HTTP transport, on `ureq`.
//!
//! Hardened to the network policy: proxies off (even from the environment), redirects refused, a
//! timeout per request, and the body read through a hard size cap. Plain `http` always works;
//! `https` needs the cargo feature `tls` and a `TlsMode` (`HttpTransport::with_tls`). Without the
//! feature an `https` URL fails as [`Error::Tls`] before any connection is made.

use std::io::Read;
use std::time::{Duration, SystemTime};

use printscan_wsd::parse_retry_after;
use ureq::Agent;

use crate::error::{Error, TransportKind};
use crate::transport::{Method, Request, Response, Transport};

/// The longest a connect may take, whatever the request's budget: a scanner that is off is reported
/// after this long, not after a budget meant for a slow page.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How an `https` connection checks the scanner (cargo feature `tls`).
///
/// A scanner on a numeric local address has no certificate a public authority vouches for: it
/// carries one it made itself. So there is no default trust store. The app either trusts
/// specific certificates (the scanner's own, or the authority that issued it, exported by a person
/// from the scanner's web page), checked the usual way including the IP address in the
/// certificate, or explicitly accepts any certificate, which encrypts the traffic but does not prove
/// which device answers.
#[cfg(feature = "tls")]
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TlsMode {
    /// Trust these DER certificates as roots. With none, every handshake fails.
    Trust(Vec<Vec<u8>>),
    /// Accept any certificate the scanner shows: encryption against eavesdroppers, no
    /// authentication of the device. An opt-in for scanners whose certificate cannot be verified.
    AcceptAnyCertificate,
}

#[cfg(feature = "tls")]
impl Default for TlsMode {
    /// Trust nothing: `https` fails until the app chooses.
    fn default() -> Self {
        Self::Trust(Vec::new())
    }
}

#[cfg(feature = "tls")]
impl TlsMode {
    /// Trust the certificates in `pem` (one or more `CERTIFICATE` blocks; other blocks are ignored).
    pub fn trust_pem(pem: &[u8]) -> Result<Self, Error> {
        let mut certs = Vec::new();
        for item in ureq::tls::parse_pem(pem) {
            match item {
                Ok(ureq::tls::PemItem::Certificate(cert)) => certs.push(cert.der().to_vec()),
                Ok(_) => {}
                Err(e) => return Err(Error::Tls(format!("unreadable PEM: {e}"))),
            }
        }
        if certs.is_empty() {
            return Err(Error::Tls("the PEM text holds no certificate".into()));
        }
        Ok(Self::Trust(certs))
    }

    fn config(&self) -> ureq::tls::TlsConfig {
        use ureq::tls::{Certificate, RootCerts, TlsConfig};
        let builder = TlsConfig::builder();
        match self {
            Self::Trust(certs) => {
                let roots: Vec<Certificate<'static>> = certs.iter().map(|der| Certificate::from_der(der).to_owned()).collect();
                builder.root_certs(RootCerts::from(roots)).build()
            }
            // Never WebPki: without its feature ureq would panic on it, and it would be useless here.
            Self::AcceptAnyCertificate => builder.root_certs(RootCerts::from(Vec::new())).disable_verification(true).build(),
        }
    }
}

/// The `ureq`-backed transport. Cheap to clone.
#[derive(Clone)]
pub struct HttpTransport {
    agent: Agent,
    tls: bool,
}

impl std::fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTransport").field("tls", &self.tls).finish_non_exhaustive()
    }
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

fn config() -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    Agent::config_builder()
        .proxy(None)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .https_only(false)
        .user_agent("printscan-escl")
}

impl HttpTransport {
    /// A plain-`http` transport: no proxy, no redirects, statuses returned as replies. An `https`
    /// URL is refused as [`Error::Tls`].
    pub fn new() -> Self {
        Self { agent: Agent::new_with_config(config().build()), tls: false }
    }

    /// A transport that also speaks `https`, checking scanners as `mode` says.
    #[cfg(feature = "tls")]
    pub fn with_tls(mode: &TlsMode) -> Self {
        Self { agent: Agent::new_with_config(config().tls_config(mode.config()).build()), tls: true }
    }
}

/// Classify an I/O error.
fn from_io(io: &std::io::Error) -> Error {
    use std::io::ErrorKind as K;
    match io.kind() {
        K::ConnectionRefused => Error::transport(TransportKind::Refused, format!("connection refused ({io})")),
        K::HostUnreachable | K::NetworkUnreachable | K::NetworkDown => Error::transport(TransportKind::Unreachable, format!("unreachable ({io})")),
        K::TimedOut | K::WouldBlock => Error::transport(TransportKind::Timeout, format!("timed out ({io})")),
        K::UnexpectedEof | K::ConnectionReset | K::ConnectionAborted | K::BrokenPipe => {
            Error::transport(TransportKind::ConnectionClosed, "the scanner closed the connection without a complete reply")
        }
        _ => Error::transport(TransportKind::Io, io.to_string()),
    }
}

/// Map a `ureq` error onto this crate's error.
fn map_error(e: ureq::Error) -> Error {
    use ureq::Error as U;
    use ureq::Timeout as T;
    let message = e.to_string();
    match e {
        U::Timeout(T::Resolve | T::Connect) => Error::transport(TransportKind::ConnectTimeout, message),
        U::Timeout(_) => Error::transport(TransportKind::Timeout, message),
        U::Io(io) => from_io(&io),
        U::HostNotFound | U::ConnectionFailed => Error::transport(TransportKind::Unreachable, message),
        U::TooManyRedirects | U::RedirectFailed => Error::transport(TransportKind::Redirect, message),
        U::Protocol(_) | U::LargeResponseHeader(..) => Error::transport(TransportKind::InvalidResponse, message),
        U::BodyExceedsLimit(limit) => Error::TooLarge("the scanner's reply", usize::try_from(limit).unwrap_or(usize::MAX)),
        U::Tls(_) | U::TlsRequired => Error::Tls(message),
        #[cfg(feature = "tls")]
        U::Rustls(_) | U::Pem(_) => Error::Tls(message),
        _ => Error::transport(TransportKind::Io, message),
    }
}

/// An error from reading the body: `ureq` wraps its own errors in `io::Error`.
fn map_body_error(e: std::io::Error) -> Error {
    match e.downcast::<ureq::Error>() {
        Ok(inner) => map_error(inner),
        Err(io) => from_io(&io),
    }
}

/// The `Retry-After` of a reply, when there is exactly one readable value.
fn retry_after(headers: &ureq::http::HeaderMap) -> Option<Duration> {
    let mut values = headers.get_all("retry-after").iter();
    let first = values.next()?;
    if values.next().is_some() {
        return None;
    }
    parse_retry_after(first.to_str().ok()?, SystemTime::now())
}

/// A header's text, at most 1 KiB.
fn header(headers: &ureq::http::HeaderMap, name: &str) -> Option<String> {
    headers.get(name).and_then(|v| v.to_str().ok()).filter(|v| v.len() <= 1024).map(str::to_owned)
}

impl Transport for HttpTransport {
    /// Send with only `Accept` and, for a body, `Content-Type` headers. A redirect is
    /// [`TransportKind::Redirect`]; a success body over `max_response` is [`Error::TooLarge`], a failure
    /// body over it keeps its status.
    fn send(&self, request: Request) -> Result<Response, Error> {
        if request.url.is_tls() && !self.tls {
            return Err(Error::Tls(if cfg!(feature = "tls") {
                "this transport was built without TLS: use HttpTransport::with_tls (EsclBackend::with_tls)".into()
            } else {
                "https needs printscan-escl's cargo feature `tls`".into()
            }));
        }
        let url = request.url.as_url();
        let connect = (request.timeout > CONNECT_TIMEOUT).then_some(CONNECT_TIMEOUT);
        let reply = match request.method {
            Method::Post => self
                .agent
                .post(&url)
                .config()
                .timeout_global(Some(request.timeout))
                .timeout_connect(connect)
                .build()
                .header("Content-Type", request.content_type.as_deref().unwrap_or("text/xml"))
                .header("Accept", "*/*")
                .send(&request.body[..]),
            Method::Get => {
                self.agent.get(&url).config().timeout_global(Some(request.timeout)).timeout_connect(connect).build().header("Accept", "*/*").call()
            }
            Method::Delete => {
                self.agent.delete(&url).config().timeout_global(Some(request.timeout)).timeout_connect(connect).build().header("Accept", "*/*").call()
            }
        };
        let mut response = reply.map_err(map_error)?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            let to = header(response.headers(), "location").map(|l| format!(" to {:?}", l.chars().take(80).collect::<String>())).unwrap_or_default();
            return Err(Error::transport(
                TransportKind::Redirect,
                format!("the scanner answered with a redirect (HTTP {status}){to}, which is refused; add the URL it names instead"),
            ));
        }
        let headers = response.headers();
        let content_type = header(headers, "content-type");
        let location = header(headers, "location");
        let wait = retry_after(headers);
        let mut body = Vec::new();
        let cap = request.max_response as u64;
        response.body_mut().as_reader().take(cap.saturating_add(1)).read_to_end(&mut body).map_err(map_body_error)?;
        if body.len() > request.max_response {
            return Err(if (200..300).contains(&status) {
                Error::TooLarge("the scanner's reply", request.max_response)
            } else {
                Error::Http { status, retry_after: wait }
            });
        }
        Ok(Response { status, content_type, location, retry_after: wait, body })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Error as IoError, ErrorKind as K};

    use super::*;

    #[test]
    fn failures_map_to_kinds() {
        let kind = |e: Error| e.transport_kind();
        assert_eq!(kind(map_error(ureq::Error::Timeout(ureq::Timeout::Connect))), Some(TransportKind::ConnectTimeout));
        assert_eq!(kind(map_error(ureq::Error::Timeout(ureq::Timeout::RecvBody))), Some(TransportKind::Timeout));
        assert_eq!(kind(map_error(ureq::Error::ConnectionFailed)), Some(TransportKind::Unreachable));
        assert_eq!(kind(map_error(ureq::Error::Io(IoError::from(K::ConnectionRefused)))), Some(TransportKind::Refused));
        assert_eq!(kind(map_body_error(IoError::from(K::UnexpectedEof))), Some(TransportKind::ConnectionClosed));
        assert!(matches!(map_error(ureq::Error::TlsRequired), Error::Tls(_)));
        assert!(matches!(map_error(ureq::Error::BodyExceedsLimit(9)), Error::TooLarge(_, 9)));
    }
}
