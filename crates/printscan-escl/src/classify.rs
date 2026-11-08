//! eSCL failures and scanner states in the scan core's terms: [`InterruptKind`]s, a
//! [`HandlingStatus`] and [`NativeCode`]s.

use printscan::scan::{HandlingStatus, InterruptKind, NativeCode, ScanError};

use crate::error::Error;
use crate::model::{AdfState, JobInfo, JobState, ScannerState, Status};

/// The [`NativeCode`] domain of HTTP statuses; the code is the status.
pub const HTTP_DOMAIN: &str = "http";
/// The [`NativeCode`] domain of feeder states (`scan:AdfState`); the name is the state.
pub const ADF_DOMAIN: &str = "escl-adf";
/// The [`NativeCode`] domain of a job that ended on the scanner's side (`pwg:JobState`).
pub const JOB_DOMAIN: &str = "escl-job";

/// The name of an HTTP status a scanner sends.
fn status_name(status: u16) -> Option<&'static str> {
    Some(match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => return None,
    })
}

/// The native code of an HTTP status.
pub fn http_code(status: u16) -> NativeCode {
    NativeCode::new(HTTP_DOMAIN, i64::from(status), status_name(status))
}

/// The feeder states this backend names, with the cause each means and its stable code.
const ADF: &[(&str, InterruptKind)] = &[
    ("ScannerAdfEmpty", InterruptKind::FeederEmpty),
    ("ScannerAdfJam", InterruptKind::Jam),
    ("ScannerAdfHatchOpen", InterruptKind::CoverOpen),
    ("ScannerAdfMultipickDetected", InterruptKind::MultiFeed),
    ("ScannerAdfMispick", InterruptKind::PaperProblem),
    ("ScannerAdfDuplexPageTooShort", InterruptKind::PaperProblem),
    ("ScannerAdfDuplexPageTooLong", InterruptKind::PaperProblem),
    ("ScannerAdfInputTrayFailed", InterruptKind::Attention),
    ("ScannerAdfInputTrayOverloaded", InterruptKind::Attention),
];

/// The cause a feeder state names, with its native code; `None` for a state that stops nothing
/// (loaded, processing, unknown).
pub fn adf_cause(state: &AdfState) -> Option<(InterruptKind, NativeCode)> {
    let name = state.name();
    ADF.iter()
        .enumerate()
        .find(|(_, (n, _))| *n == name)
        .map(|(i, (n, kind))| (*kind, NativeCode::new(ADF_DOMAIN, i64::try_from(i).unwrap_or(0) + 1, Some(n))))
}

/// The error a feeder state names, as the scanner's state.
pub fn adf_error(state: &AdfState, doing: &str) -> Option<ScanError> {
    adf_cause(state)
        .map(|(kind, native)| ScanError::native_error(kind, native, format!("{} ({doing}: the scanner says {})", kind.sentence(), state.name())))
}

/// What the status says about the paper path. The feeder is ready when the scanner says
/// `ScannerAdfLoaded` and known empty for `ScannerAdfEmpty`; a scanner that is `Down` has a device
/// fault and one that is `Stopped` needs attention.
pub fn handling_status(status: &Status) -> HandlingStatus {
    let adf = status.adf_state.as_ref();
    let is = |s: AdfState| adf == Some(&s);
    HandlingStatus::default()
        .with_feeder_ready(match adf {
            Some(AdfState::Loaded) => Some(true),
            Some(AdfState::Empty) => Some(false),
            _ => None,
        })
        .with_jam(is(AdfState::Jam))
        .with_path_cover_open(is(AdfState::HatchOpen))
        .with_multiple_feed(is(AdfState::MultipickDetected))
        .with_attention(
            status.state == Some(ScannerState::Stopped)
                || adf.is_some_and(|a| {
                    matches!(
                        a,
                        AdfState::Mispick
                            | AdfState::DuplexPageTooShort
                            | AdfState::DuplexPageTooLong
                            | AdfState::InputTrayFailed
                            | AdfState::InputTrayOverloaded
                    )
                }),
        )
        .with_lamp_error(status.state == Some(ScannerState::Down))
}

/// `error` (from `doing`, such as "ScanJobs") as a [`ScanError`]: the network failing is
/// [`InterruptKind::Offline`]; HTTP 503, 429 and 409 are [`InterruptKind::Busy`] (the scanner is
/// working on something else; waiting is the cure); other statuses keep their code.
pub fn scan_error(doing: &str, error: &Error) -> ScanError {
    let message = format!("{doing}: {error}");
    match error {
        Error::Http { status, .. } => {
            let kind = match status {
                503 | 429 | 409 => InterruptKind::Busy,
                _ => InterruptKind::Other,
            };
            let message = if kind == InterruptKind::Busy { format!("{} ({message})", kind.sentence()) } else { message };
            ScanError::native_error(kind, http_code(*status), message)
        }
        Error::Transport { kind, .. } if !matches!(kind, crate::TransportKind::InvalidResponse | crate::TransportKind::Redirect) => {
            ScanError::Native { kind: InterruptKind::Offline, native: None, message: format!("{} ({message})", InterruptKind::Offline.sentence()) }
        }
        _ => ScanError::Native { kind: InterruptKind::Other, native: None, message },
    }
}

/// How a job the scanner ended on its own side reads, from its `JobInfo` and the feeder state:
/// `Canceled` that this backend did not ask for is [`InterruptKind::Cancelled`] (the panel's Cancel
/// key), `Aborted` takes the feeder's cause when it names one. `None` for a job that is fine.
pub fn job_error(job: &JobInfo, adf: Option<&AdfState>) -> Option<ScanError> {
    let state = job.state.as_ref()?;
    let (kind, name) = match state {
        JobState::Canceled => (InterruptKind::Cancelled, "Canceled"),
        JobState::Aborted => (adf.and_then(adf_cause).map_or(InterruptKind::Other, |(k, _)| k), "Aborted"),
        _ => return None,
    };
    let reasons = if job.reasons.is_empty() { String::new() } else { format!(": {}", job.reasons.join(", ")) };
    let code = NativeCode::new(JOB_DOMAIN, if name == "Canceled" { 1 } else { 2 }, Some(name));
    let sentence = if kind == InterruptKind::Other { "The scanner ended the scan." } else { kind.sentence() };
    Some(ScanError::native_error(kind, code, format!("{sentence} (the job is {name}{reasons})")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TransportKind;

    #[test]
    fn http_and_network_failures_have_kinds() {
        let busy = scan_error("ScanJobs", &Error::Http { status: 503, retry_after: None });
        assert_eq!(busy.interrupt_kind(), InterruptKind::Busy);
        assert_eq!(busy.native(), Some(NativeCode::new("http", 503, Some("Service Unavailable"))));
        assert_eq!(scan_error("x", &Error::Http { status: 409, retry_after: None }).interrupt_kind(), InterruptKind::Busy);
        assert_eq!(scan_error("x", &Error::Http { status: 400, retry_after: None }).interrupt_kind(), InterruptKind::Other);
        assert_eq!(scan_error("x", &Error::transport(TransportKind::Refused, "no")).interrupt_kind(), InterruptKind::Offline);
        assert_eq!(scan_error("x", &Error::transport(TransportKind::Redirect, "no")).interrupt_kind(), InterruptKind::Other);
    }

    #[test]
    fn feeder_states_name_causes() {
        let status = |adf: AdfState| Status { adf_state: Some(adf), ..Status::default() };
        assert_eq!(handling_status(&status(AdfState::Loaded)).feeder_ready, Some(true));
        assert_eq!(handling_status(&status(AdfState::Empty)).feeder_ready, Some(false));
        assert!(handling_status(&status(AdfState::Jam)).jam);
        assert_eq!(handling_status(&Status::default()).feeder_ready, None);
        let down = Status { state: Some(ScannerState::Down), ..Status::default() };
        assert_eq!(handling_status(&down).acquire_block_kind(), Some(InterruptKind::DeviceError));
        let e = adf_error(&AdfState::Empty, "ScanJobs").unwrap();
        assert_eq!(e.interrupt_kind(), InterruptKind::FeederEmpty);
        assert_eq!(e.native().and_then(|n| n.name), Some("ScannerAdfEmpty"));
        assert!(adf_error(&AdfState::Loaded, "x").is_none());
        assert!(adf_error(&AdfState::Other("New".into()), "x").is_none());
    }

    #[test]
    fn a_job_ended_at_the_scanner() {
        let job = |state| JobInfo { state: Some(state), ..JobInfo::default() };
        assert_eq!(job_error(&job(JobState::Canceled), None).unwrap().interrupt_kind(), InterruptKind::Cancelled);
        assert_eq!(job_error(&job(JobState::Aborted), Some(&AdfState::Jam)).unwrap().interrupt_kind(), InterruptKind::Jam);
        assert_eq!(job_error(&job(JobState::Aborted), None).unwrap().interrupt_kind(), InterruptKind::Other);
        assert!(job_error(&job(JobState::Completed), None).is_none());
    }
}
