//! The transport- and document-level error of an eSCL exchange. [`crate::classify`] turns it into
//! the scan core's [`printscan::scan::ScanError`].

use std::time::Duration;

use printscan::job::RetrySafety;

/// How a transport failed before a usable HTTP reply arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TransportKind {
    /// The scanner refused the connection (nothing was sent).
    Refused,
    /// No route to the scanner, or the connection could not be made (nothing was sent).
    Unreachable,
    /// The connection was not made in time (nothing was sent).
    ConnectTimeout,
    /// The exchange ran out of time after the request may have been delivered.
    Timeout,
    /// The scanner closed the connection without a complete reply.
    ConnectionClosed,
    /// The scanner answered with a redirect, which is refused.
    Redirect,
    /// The peer did not speak HTTP.
    InvalidResponse,
    /// Any other I/O failure.
    Io,
}

impl TransportKind {
    /// Whether this failure happened before anything was sent.
    pub const fn before_send(self) -> bool {
        matches!(self, Self::Refused | Self::Unreachable | Self::ConnectTimeout)
    }
}

/// Everything an exchange with a scanner can fail with.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The address policy refused a URL; nothing was sent.
    #[error("refused by policy: {0}")]
    Policy(String),
    /// TLS could not be used: the crate was built without feature `tls`, or the handshake or the
    /// scanner's certificate failed.
    #[error("TLS: {0}")]
    Tls(String),
    /// The transport failed before a usable HTTP reply arrived.
    #[error("transport: {message}")]
    Transport { kind: TransportKind, message: String },
    /// An HTTP status the operation does not expect, and the wait the scanner asked for.
    #[error("the scanner answered HTTP {status}")]
    Http { status: u16, retry_after: Option<Duration> },
    /// A size cap was hit (what, the limit in bytes).
    #[error("{0} is larger than the limit of {1} bytes")]
    TooLarge(&'static str, usize),
    /// The reply is not well-formed, bounded XML.
    #[error("unreadable XML: {0}")]
    Xml(String),
    /// A reply that is well-formed but not what the operation expects.
    #[error("unexpected reply: {0}")]
    Protocol(String),
    /// The caller cancelled; nothing further was sent.
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// A transport failure of `kind`.
    pub fn transport(kind: TransportKind, message: impl Into<String>) -> Self {
        Self::Transport { kind, message: message.into() }
    }

    /// The HTTP status, for [`Self::Http`].
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::Http { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// The wait the scanner asked for with `Retry-After`, already capped.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Http { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// The transport kind, for [`Self::Transport`].
    pub fn transport_kind(&self) -> Option<TransportKind> {
        match self {
            Self::Transport { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    /// Whether the request may have reached the scanner. A policy refusal, a cancel and a failed
    /// connect did not; TLS failures count as not sent, since the handshake comes before the request.
    pub fn may_have_been_sent(&self) -> bool {
        match self {
            Self::Policy(_) | Self::Cancelled | Self::Tls(_) => false,
            Self::Transport { kind, .. } => !kind.before_send(),
            Self::Http { .. } | Self::TooLarge(..) | Self::Xml(_) | Self::Protocol(_) => true,
        }
    }

    /// What repeating the request would do. `idempotent` is the caller's word that the request is a
    /// read or a cancel. A 503 or 429 says the scanner did not take the request.
    pub fn retry_safety(&self, idempotent: bool) -> RetrySafety {
        if !self.may_have_been_sent() || matches!(self.http_status(), Some(503 | 429)) {
            RetrySafety::Safe
        } else if idempotent {
            RetrySafety::Idempotent
        } else {
            RetrySafety::OutcomeUnknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_not_sent_is_safe_to_repeat() {
        assert_eq!(Error::transport(TransportKind::Refused, "x").retry_safety(false), RetrySafety::Safe);
        assert_eq!(Error::Policy("x".into()).retry_safety(false), RetrySafety::Safe);
        assert_eq!(Error::Http { status: 503, retry_after: None }.retry_safety(false), RetrySafety::Safe);
        assert_eq!(Error::transport(TransportKind::Timeout, "x").retry_safety(false), RetrySafety::OutcomeUnknown);
        assert_eq!(Error::transport(TransportKind::Timeout, "x").retry_safety(true), RetrySafety::Idempotent);
        assert_eq!(Error::Http { status: 500, retry_after: None }.retry_safety(false), RetrySafety::OutcomeUnknown);
    }
}
