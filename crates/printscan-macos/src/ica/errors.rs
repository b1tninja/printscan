//! ImageCaptureCore's errors in printscan's terms. An `NSError` from a delegate callback is read into
//! an [`IcaError`] (plain data) by the Objective-C layer; [`scan_error`] turns it into a
//! [`ScanError::Native`] with the [`InterruptKind`] it stands for and a [`NativeCode`] in
//! [`DOMAIN`] (`ica -9924 (ICReturnScanOperationCanceled)`). An error from any other domain keeps
//! its code in [`OTHER_DOMAIN`] and its domain in the message.
//!
//! The codes are the values of ImageCaptureCore's `ICReturnCode`, `ICLegacyReturnCode` and
//! `ICReturnConnectionErrorCode` enumerations, by their exact names. Which code a real scanner
//! sends for which condition has not been measured: the kinds below are read from the names.

use std::time::Duration;

use printscan::scan::{InterruptKind, NativeCode, ScanError};

/// The [`NativeCode::domain`] of an error ImageCaptureCore raised (`ICErrorDomain`, or an
/// `OSStatus` in ImageCaptureCore's own range).
pub const DOMAIN: &str = "ica";
/// The domain of an `NSError` from any other error domain; the message names the real one.
pub const OTHER_DOMAIN: &str = "nserror";
/// The domain of the errors this backend raises itself: a step that timed out, a device that went away.
pub const BACKEND_DOMAIN: &str = "ica-backend";

/// ImageCaptureCore did not answer a step in time.
pub const TIMED_OUT: NativeCode = NativeCode::new(BACKEND_DOMAIN, 1, Some("TIMED_OUT"));
/// The device was removed (unplugged, switched off, gone from the network) during a session.
pub const DEVICE_REMOVED: NativeCode = NativeCode::new(BACKEND_DOMAIN, 2, Some("DEVICE_REMOVED"));
/// The device id names no scanner the browser lists.
pub const NO_SUCH_DEVICE: NativeCode = NativeCode::new(BACKEND_DOMAIN, 3, Some("NO_SUCH_DEVICE"));

/// An `NSError` as plain data.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct IcaError {
    /// The error is ImageCaptureCore's own: its domain is `ICErrorDomain`, or `NSOSStatusErrorDomain`
    /// with a code in ImageCaptureCore's range.
    pub ours: bool,
    /// The error domain as the `NSError` names it.
    pub domain: String,
    pub code: i64,
    /// `localizedDescription`.
    pub description: String,
}

impl IcaError {
    /// An error in `domain` with `code` and its localized `description`. `ours` is decided from the
    /// domain: `ica_domain` is the runtime value of `ICErrorDomain`.
    pub fn new(domain: impl Into<String>, code: i64, description: impl Into<String>, ica_domain: Option<&str>) -> Self {
        let domain = domain.into();
        let ours = ica_domain.is_some_and(|ica| ica == domain) || (domain == "NSOSStatusErrorDomain" && code_name(code).is_some());
        Self { ours, domain, code, description: description.into() }
    }

    /// Whether this is ImageCaptureCore's "the scan was cancelled" (or "the download was cancelled").
    pub fn is_cancel(&self) -> bool {
        self.ours && matches!(self.code, SCAN_OPERATION_CANCELED | DOWNLOAD_CANCELED)
    }
}

/// `ICReturnScanOperationCanceled`.
pub const SCAN_OPERATION_CANCELED: i64 = -9924;
/// `ICReturnDownloadCanceled`.
pub const DOWNLOAD_CANCELED: i64 = -9937;

/// Every code this crate classifies: its name and the kind it stands for.
pub const CODES: [(i64, &str, InterruptKind); 30] = [
    (-9900, "ICReturnCommunicationErr", InterruptKind::Offline),
    (-9901, "ICReturnDeviceNotFoundErr", InterruptKind::Offline),
    (-9902, "ICReturnDeviceNotOpenErr", InterruptKind::Other),
    (-9904, "ICReturnIOPendingErr", InterruptKind::Busy),
    (-9911, "ICReturnDeviceMemoryAllocationErr", InterruptKind::DeviceError),
    (-9912, "ICReturnDeviceInternalErr", InterruptKind::DeviceError),
    (-9913, "ICReturnDeviceInvalidParamErr", InterruptKind::Other),
    (-9914, "ICReturnDeviceAlreadyOpenErr", InterruptKind::Busy),
    (-9918, "ICReturnDeviceUnsupportedErr", InterruptKind::Other),
    (-9919, "ICReturnFrameworkInternalErr", InterruptKind::Other),
    (-9921, "ICReturnInvalidSessionErr", InterruptKind::Other),
    (-9922, "ICReturnInvalidParam", InterruptKind::Other),
    (-9923, "ICReturnCommunicationTimedOut", InterruptKind::Offline),
    (SCAN_OPERATION_CANCELED, "ICReturnScanOperationCanceled", InterruptKind::Cancelled),
    (-9925, "ICReturnScannerInUseByLocalUser", InterruptKind::Busy),
    (-9926, "ICReturnScannerInUseByRemoteUser", InterruptKind::Busy),
    (-9927, "ICReturnDeviceFailedToOpenSession", InterruptKind::Offline),
    (-9928, "ICReturnDeviceFailedToCloseSession", InterruptKind::Other),
    (-9929, "ICReturnScannerFailedToSelectFunctionalUnit", InterruptKind::Other),
    (-9931, "ICReturnScannerFailedToCompleteScan", InterruptKind::Other),
    (-9932, "ICReturnReceivedUnsolicitedScannerStatusInfo", InterruptKind::Attention),
    (-9933, "ICReturnReceivedUnsolicitedScannerErrorInfo", InterruptKind::Attention),
    (-9934, "ICReturnDownloadFailed", InterruptKind::Other),
    (DOWNLOAD_CANCELED, "ICReturnDownloadCanceled", InterruptKind::Cancelled),
    (-21350, "ICReturnConnectionDriverExited", InterruptKind::DeviceError),
    (-21349, "ICReturnConnectionClosedSessionSuddenly", InterruptKind::Offline),
    (-21347, "ICReturnConnectionSessionAlreadyOpen", InterruptKind::Busy),
    (-21345, "ICReturnConnectionFailedToOpen", InterruptKind::Offline),
    (-21344, "ICReturnConnectionFailedToOpenDevice", InterruptKind::Offline),
    (-21343, "ICReturnConnectionNotAuthorizedToOpenDevice", InterruptKind::Other),
];

/// The name of a code this crate classifies.
pub fn code_name(code: i64) -> Option<&'static str> {
    CODES.iter().find(|(known, _, _)| *known == code).map(|(_, name, _)| *name)
}

/// The kind a code stands for; `None` for a code this crate does not classify.
pub fn kind_for_code(code: i64) -> Option<InterruptKind> {
    CODES.iter().find(|(known, _, _)| *known == code).map(|(_, _, kind)| *kind)
}

/// The sentence for a code that needs more than its kind's sentence.
fn special_sentence(code: i64) -> Option<&'static str> {
    match code {
        -21343 => Some(
            "macOS did not allow this app to use the scanner. Allow it in System Settings (Privacy & Security), or check the app's sandbox entitlements.",
        ),
        -9925 | -9926 => Some("The scanner is in use by someone else. Try again when they are done."),
        _ => None,
    }
}

/// The printscan error for `error`, raised while the backend was `doing` something (`"opening the
/// session"`).
pub fn scan_error(error: &IcaError, doing: &str) -> ScanError {
    if !error.ours {
        let code = NativeCode::new(OTHER_DOMAIN, error.code, None);
        return ScanError::native_error(
            InterruptKind::Other,
            code,
            format!("ImageCaptureCore failed while {doing}: {} ({} {})", error.description, error.domain, error.code),
        );
    }
    let kind = kind_for_code(error.code).unwrap_or(InterruptKind::Other);
    let code = NativeCode::new(DOMAIN, error.code, code_name(error.code));
    let message = match (special_sentence(error.code), kind) {
        (Some(sentence), _) => sentence.to_string(),
        (None, InterruptKind::Other) if error.description.is_empty() => format!("ImageCaptureCore failed while {doing} ({code})."),
        (None, InterruptKind::Other) => format!("ImageCaptureCore failed while {doing}: {}", error.description),
        (None, kind) => kind.sentence().to_string(),
    };
    ScanError::native_error(kind, code, message)
}

/// The error for a step ImageCaptureCore did not answer within `after`.
pub fn timed_out(doing: &str, after: Duration) -> ScanError {
    ScanError::native_error(
        InterruptKind::Offline,
        TIMED_OUT,
        format!(
            "The scanner did not answer within {} s while {doing}. ImageCaptureCore answers on the main thread: if this was called from another thread, the main thread must be running its run loop.",
            after.as_secs()
        ),
    )
}

/// The error for a device that went away during a session.
pub fn removed() -> ScanError {
    ScanError::native_error(InterruptKind::Offline, DEVICE_REMOVED, "The scanner was disconnected. Check that it is powered on and connected.")
}

/// The error for a device id the browser does not list.
pub fn no_such_device(id: &str) -> ScanError {
    ScanError::native_error(
        InterruptKind::Offline,
        NO_SUCH_DEVICE,
        format!("No ImageCaptureCore scanner has the id {id:?}; list the scanners again."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ICA: Option<&str> = Some("com.apple.ImageCaptureCore");

    #[test]
    fn our_codes_keep_their_kind_and_name() {
        let error = IcaError::new("com.apple.ImageCaptureCore", -9925, "In use", ICA);
        let mapped = scan_error(&error, "opening the session");
        assert_eq!(mapped.interrupt_kind(), InterruptKind::Busy);
        assert_eq!(mapped.native(), Some(NativeCode::new(DOMAIN, -9925, Some("ICReturnScannerInUseByLocalUser"))));
        assert!(mapped.to_string().contains("in use by someone else"));
        let timed = scan_error(&IcaError::new("com.apple.ImageCaptureCore", -9923, "", ICA), "scanning");
        assert_eq!(timed.to_string(), InterruptKind::Offline.sentence());
    }

    #[test]
    fn os_status_codes_in_our_range_are_ours() {
        let error = IcaError::new("NSOSStatusErrorDomain", -9912, "internal", ICA);
        assert!(error.ours);
        assert_eq!(scan_error(&error, "scanning").interrupt_kind(), InterruptKind::DeviceError);
        assert!(!IcaError::new("NSOSStatusErrorDomain", -50, "paramErr", ICA).ours);
    }

    #[test]
    fn foreign_errors_keep_domain_and_code() {
        let error = IcaError::new("NSCocoaErrorDomain", 4, "No such file", ICA);
        let mapped = scan_error(&error, "reading the page");
        assert_eq!(mapped.interrupt_kind(), InterruptKind::Other);
        assert_eq!(mapped.native(), Some(NativeCode::new(OTHER_DOMAIN, 4, None)));
        assert!(mapped.to_string().contains("NSCocoaErrorDomain 4"));
    }

    #[test]
    fn unknown_codes_and_cancels() {
        let unknown = scan_error(&IcaError::new("com.apple.ImageCaptureCore", -1, "", ICA), "scanning");
        assert_eq!(unknown.interrupt_kind(), InterruptKind::Other);
        assert!(unknown.to_string().contains("ica -1"));
        assert!(IcaError::new("com.apple.ImageCaptureCore", SCAN_OPERATION_CANCELED, "", ICA).is_cancel());
        assert!(!IcaError::new("elsewhere", SCAN_OPERATION_CANCELED, "", ICA).is_cancel());
        let denied = scan_error(&IcaError::new("com.apple.ImageCaptureCore", -21343, "", ICA), "opening the session");
        assert!(denied.to_string().contains("System Settings"));
    }

    #[test]
    fn every_code_is_listed_once() {
        for (index, (code, _, _)) in CODES.iter().enumerate() {
            assert!(CODES[index + 1..].iter().all(|(other, _, _)| other != code), "{code} twice");
        }
        assert!(timed_out("opening the session", Duration::from_secs(30)).to_string().contains("30 s"));
        assert_eq!(removed().native(), Some(DEVICE_REMOVED));
    }
}
