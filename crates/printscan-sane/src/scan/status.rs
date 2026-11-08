//! SANE statuses and client failures as the scan core's errors.

use printscan::scan::{InterruptKind, NativeCode, ScanError};

use crate::error::Error;
use crate::protocol::Status;

/// The [`NativeCode::domain`] of SANE status codes; the code is the status word and the name its
/// symbolic name (`sane 7 (SANE_STATUS_NO_DOCS)`).
pub const DOMAIN: &str = "sane";
/// The [`NativeCode::domain`] of conditions this client raises itself.
pub const NET_DOMAIN: &str = "sane-net";
/// The daemon asks for a user name and password, which this client does not send. The person can
/// allow the device without credentials on the daemon (its users and access files), or scan through
/// another route.
pub const AUTHORIZATION_REQUIRED: NativeCode = NativeCode::new(NET_DOMAIN, 1, Some("AUTHORIZATION_REQUIRED"));

/// The cause a status names. Busy, warming up and locked are [`InterruptKind::Busy`] (waiting is
/// the cure, so `ScanJob` may retry by itself); a device I/O error is [`InterruptKind::Offline`]
/// (the outcome is unknown, so the device is checked first); a cancel the client did not ask for
/// is the device's own ([`InterruptKind::Cancelled`]).
pub fn interrupt_kind(status: Status) -> InterruptKind {
    match status {
        Status::DeviceBusy | Status::WarmingUp | Status::HwLocked => InterruptKind::Busy,
        Status::Jammed => InterruptKind::Jam,
        Status::NoDocs => InterruptKind::FeederEmpty,
        Status::CoverOpen => InterruptKind::CoverOpen,
        Status::IoError => InterruptKind::Offline,
        Status::Cancelled => InterruptKind::Cancelled,
        _ => InterruptKind::Other,
    }
}

/// The native code of `status`.
pub fn native(status: Status) -> NativeCode {
    NativeCode::new(DOMAIN, i64::from(status.code()), Some(status.name()))
}

/// `error` as a scan error, its message prefixed with what was being done (`"start"`, `"open
/// pixma:..."`). A status keeps its code and cause; authorization is [`AUTHORIZATION_REQUIRED`]; a
/// connection that failed or timed out is [`InterruptKind::Offline`].
pub fn scan_error(doing: &str, error: &Error) -> ScanError {
    let message = format!("{doing}: {error}");
    match error {
        Error::Status { status, .. } => ScanError::native_error(interrupt_kind(*status), native(*status), message),
        Error::AuthorizationRequired { .. } => ScanError::native_error(InterruptKind::Other, AUTHORIZATION_REQUIRED, message),
        Error::Transport { .. } => ScanError::Native { kind: InterruptKind::Offline, native: None, message },
        Error::Cancelled => ScanError::Native { kind: InterruptKind::Cancelled, native: None, message },
        _ => ScanError::Native { kind: InterruptKind::Other, native: None, message },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TransportKind;

    #[test]
    fn statuses_name_their_cause() {
        let cases = [
            (Status::NoDocs, InterruptKind::FeederEmpty),
            (Status::Jammed, InterruptKind::Jam),
            (Status::CoverOpen, InterruptKind::CoverOpen),
            (Status::DeviceBusy, InterruptKind::Busy),
            (Status::WarmingUp, InterruptKind::Busy),
            (Status::IoError, InterruptKind::Offline),
            (Status::Inval, InterruptKind::Other),
        ];
        for (status, kind) in cases {
            let error = scan_error("start", &Error::Status { operation: "start", status });
            assert_eq!(error.interrupt_kind(), kind, "{status:?}");
            assert_eq!(error.native(), Some(native(status)));
        }
        let no_docs = scan_error("start", &Error::Status { operation: "start", status: Status::NoDocs });
        assert_eq!(no_docs.native().unwrap().to_string(), "sane 7 (SANE_STATUS_NO_DOCS)");
        let auth = scan_error("open", &Error::AuthorizationRequired { resource: "test".into() });
        assert_eq!(auth.native(), Some(AUTHORIZATION_REQUIRED));
        let offline = scan_error("connect", &Error::transport(TransportKind::Refused, "refused"));
        assert_eq!(offline.interrupt_kind(), InterruptKind::Offline);
    }
}
