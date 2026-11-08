//! The crate's error, classified so a caller decides without reading the sentence.

use std::io;

use printscan::job::RetrySafety;

use crate::protocol::Status;

/// What went wrong on the connection itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TransportKind {
    /// The connection could not be made (nothing listens, no route); nothing was sent.
    Refused,
    /// The connection could not be made in time.
    ConnectTimeout,
    /// A reply or data did not arrive in time.
    Timeout,
    /// The daemon closed the connection before the reply or the data ended.
    Closed,
    /// Any other I/O failure.
    Io,
}

impl TransportKind {
    /// Whether the failure happened while connecting, so nothing was sent.
    pub const fn before_sending(self) -> bool {
        matches!(self, Self::Refused | Self::ConnectTimeout)
    }
}

/// An error of the SANE client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The address is not one this crate dials (see [`crate::policy`]). Nothing was sent.
    #[error("{0}")]
    Policy(String),
    /// The connection failed.
    #[error("{message}")]
    Transport { kind: TransportKind, message: String },
    /// The daemon sent something this client cannot read.
    #[error("the SANE daemon sent a reply this client cannot read: {0}")]
    Protocol(String),
    /// A string, array or image was larger than the configured [`crate::Limits`].
    #[error("{what} is larger than the limit of {limit}")]
    TooLarge { what: &'static str, limit: u64 },
    /// The daemon answered a procedure with a status other than success.
    #[error("{operation}: {status}")]
    Status { operation: &'static str, status: Status },
    /// The daemon asks for a user name and password for `resource`, which this client does not send.
    /// The connection is closed.
    #[error("the SANE daemon asks for a user name and password for {resource:?}; this client does not send credentials")]
    AuthorizationRequired { resource: String },
    /// The caller asked for something the protocol or the device cannot carry (a string with a NUL,
    /// a value of the wrong type). Nothing was sent.
    #[error("{0}")]
    Invalid(String),
    /// The caller cancelled.
    #[error("the operation was cancelled")]
    Cancelled,
}

/// The crate's result.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// A transport error of `kind`.
    pub fn transport(kind: TransportKind, message: impl Into<String>) -> Self {
        Self::Transport { kind, message: message.into() }
    }

    /// An I/O error while `doing` something, classified by its kind.
    pub(crate) fn io(doing: &str, error: &io::Error) -> Self {
        let kind = match error.kind() {
            io::ErrorKind::ConnectionRefused => TransportKind::Refused,
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => TransportKind::Timeout,
            io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted | io::ErrorKind::BrokenPipe => {
                TransportKind::Closed
            }
            _ => TransportKind::Io,
        };
        Self::transport(kind, format!("{doing}: {error}"))
    }

    /// The SANE status, when the daemon answered with one.
    pub fn status(&self) -> Option<Status> {
        match self {
            Self::Status { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// The transport failure, when the connection failed.
    pub fn transport_kind(&self) -> Option<TransportKind> {
        match self {
            Self::Transport { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    /// Whether a later attempt could plausibly succeed without anyone changing anything: a busy or
    /// warming device, a dropped connection, a timeout.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Transport { .. } => true,
            Self::Status { status, .. } => status.is_transient(),
            _ => false,
        }
    }

    /// What repeating the request would do. Nothing was sent for a policy, invalid or connect
    /// failure, and a device that answered busy did nothing. After any other failure the request
    /// may have been acted on: `idempotent` says whether the caller's request is harmless to repeat
    /// (a query, a cancel); a start is not.
    pub fn retry_safety(&self, idempotent: bool) -> RetrySafety {
        let after_sending = if idempotent { RetrySafety::Idempotent } else { RetrySafety::OutcomeUnknown };
        match self {
            Self::Policy(_) | Self::Invalid(_) => RetrySafety::Safe,
            Self::Transport { kind, .. } if kind.before_sending() => RetrySafety::Safe,
            Self::Status { status, .. } if status.did_nothing() => RetrySafety::Safe,
            _ => after_sending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        let busy = Error::Status { operation: "start", status: Status::DeviceBusy };
        assert!(busy.is_retryable());
        assert_eq!(busy.retry_safety(false), RetrySafety::Safe);
        let jam = Error::Status { operation: "start", status: Status::Jammed };
        assert!(!jam.is_retryable());
        assert_eq!(jam.retry_safety(false), RetrySafety::OutcomeUnknown);
        let refused = Error::transport(TransportKind::Refused, "refused");
        assert_eq!(refused.retry_safety(false), RetrySafety::Safe);
        let closed = Error::io("reading", &io::Error::from(io::ErrorKind::UnexpectedEof));
        assert_eq!(closed.transport_kind(), Some(TransportKind::Closed));
        assert_eq!(closed.retry_safety(true), RetrySafety::Idempotent);
        assert_eq!(closed.retry_safety(false), RetrySafety::OutcomeUnknown);
        assert_eq!(Error::Policy("no".into()).retry_safety(false), RetrySafety::Safe);
    }
}
