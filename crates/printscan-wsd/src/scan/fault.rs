//! WS-Scan faults and scanner conditions, as the scan core's [`InterruptKind`]s.

use printscan::scan::{HandlingStatus, InterruptKind, NativeCode, ScanError};

use super::model::Status;
use super::ns::WSCN;
use crate::Error;

/// The [`NativeCode`] domain of WS-Scan fault sub-codes.
pub const DOMAIN: &str = "wsd-scan";

/// The `wscn` fault sub-codes this backend tells apart, with the cause each one names. The first
/// thirteen are the schema's `ScanServiceFaultValuesType`; `ClientErrorDocumentFormatNotSupported`
/// is the spelling of the Format element's documentation, and the last four name paper and cover
/// faults some scanners send instead of a status change.
const CODES: &[(&str, InterruptKind)] = &[
    ("InvalidArgs", InterruptKind::Other),
    ("OperationFailed", InterruptKind::Other),
    ("ServerErrorTemporaryError", InterruptKind::Busy),
    ("ServerErrorInternalError", InterruptKind::DeviceError),
    ("ServerErrorNotAcceptingJobs", InterruptKind::Busy),
    ("ClientErrorJobCancelled", InterruptKind::Cancelled),
    ("ClientErrorJobIdNotFound", InterruptKind::Other),
    ("ClientErrorFormatNotSupported", InterruptKind::Other),
    ("ClientErrorInvalidScanIdentifier", InterruptKind::Other),
    ("ClientErrorInvalidDestinationToken", InterruptKind::Other),
    ("ClientErrorNoImagesAvailable", InterruptKind::FeederEmpty),
    ("ClientErrorInvalidJobToken", InterruptKind::Other),
    ("ClientErrorConflictingRequiredParameters", InterruptKind::Other),
    ("ClientErrorDocumentFormatNotSupported", InterruptKind::Other),
    ("ClientErrorMediaJam", InterruptKind::Jam),
    ("ClientErrorMultipleFeed", InterruptKind::MultiFeed),
    ("ClientErrorCoverOpen", InterruptKind::CoverOpen),
    ("ClientErrorInterlockOpen", InterruptKind::CoverOpen),
];

/// The first `wscn` sub-code of `error` this backend knows, with its index in [`CODES`].
fn known_code(error: &Error) -> Option<(usize, &'static str, InterruptKind)> {
    let fault = error.fault()?;
    std::iter::once(&fault.code).chain(&fault.nested).find_map(|code| {
        if code.ns.as_deref() != Some(WSCN) {
            return None;
        }
        CODES.iter().enumerate().find(|(_, (name, _))| *name == code.local).map(|(i, (name, kind))| (i, *name, *kind))
    })
}

/// Is `error` the `wscn:<local>` fault?
pub fn is(error: &Error, local: &str) -> bool {
    error.is_fault(WSCN, local)
}

/// Did the scanner refuse the format (either spelling)?
pub fn is_format_refused(error: &Error) -> bool {
    is(error, "ClientErrorFormatNotSupported") || is(error, "ClientErrorDocumentFormatNotSupported")
}

/// The [`NativeCode`] for one of [`CODES`].
fn native(index: usize, name: &'static str) -> NativeCode {
    NativeCode::new(DOMAIN, i64::try_from(index).unwrap_or(i64::MAX) + 1, Some(name))
}

/// `error` (from the operation `doing`, such as "CreateScanJob") as a [`ScanError`]: a known fault
/// keeps its sub-code as the native code and names its cause; the network failing is
/// [`InterruptKind::Offline`]; an HTTP 503 or 429 is [`InterruptKind::Busy`].
pub fn scan_error(doing: &str, error: &Error) -> ScanError {
    let message = format!("{doing}: {error}");
    if let Some((index, name, kind)) = known_code(error) {
        return ScanError::native_error(kind, native(index, name), message);
    }
    let kind = match error {
        Error::Transport { .. } | Error::Timeout => InterruptKind::Offline,
        Error::Http { status: 503 | 429, .. } => InterruptKind::Busy,
        Error::Cancelled => InterruptKind::Cancelled,
        _ => InterruptKind::Other,
    };
    ScanError::Native { kind, native: None, message }
}

/// What the scanner's status says about the paper path. The feeder is only known empty when the
/// scanner names `InputTrayEmpty`; nothing in WS-Scan says that paper is loaded.
pub fn handling_status(status: &Status) -> HandlingStatus {
    HandlingStatus {
        feeder_ready: status.names("InputTrayEmpty").then_some(false),
        jam: status.names("MediaJam"),
        cover_open: status.names("CoverOpen"),
        path_cover_open: status.names("InterlockOpen"),
        multiple_feed: status.names("MultipleFeedError"),
        lamp_error: status.names("LampError"),
        attention: status.names("AttentionRequired"),
        ..HandlingStatus::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Fault, Role, SubCode};

    fn fault(local: &str) -> Error {
        Error::from_fault(Fault::new(Role::Sender, SubCode::new(Some(WSCN), local), "no", ""))
    }

    #[test]
    fn known_faults_keep_their_code_and_cause() {
        let e = scan_error("RetrieveImage", &fault("ClientErrorNoImagesAvailable"));
        assert_eq!(e.interrupt_kind(), InterruptKind::FeederEmpty);
        assert_eq!(e.native().unwrap().name, Some("ClientErrorNoImagesAvailable"));
        assert_eq!(scan_error("x", &fault("ServerErrorNotAcceptingJobs")).interrupt_kind(), InterruptKind::Busy);
        assert_eq!(scan_error("x", &fault("ClientErrorMediaJam")).interrupt_kind(), InterruptKind::Jam);
        assert!(is_format_refused(&fault("ClientErrorDocumentFormatNotSupported")));
        assert!(!is_format_refused(&fault("InvalidArgs")));
        // Another namespace's code with the same local name is not ours.
        let other = Error::from_fault(Fault::new(Role::Sender, SubCode::new(Some("urn:x"), "ClientErrorMediaJam"), "", ""));
        assert_eq!(scan_error("x", &other).interrupt_kind(), InterruptKind::Other);
        assert_eq!(scan_error("x", &Error::transport(crate::TransportKind::Refused, "refused")).interrupt_kind(), InterruptKind::Offline);
    }
}
