//! What an app chooses about the backend: whether network scanners are browsed, and how long each
//! step may take.

use std::time::Duration;

/// How the backend browses and how long it waits. `Default` is local scanners only, with deadlines
/// that suit a USB scanner; a network scanner may need longer ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct IcaOptions {
    /// Browse Bonjour, shared and remote scanners as well as local ones. Off by default: the
    /// browse is network discovery, which the app asks for.
    pub network: bool,
    /// The longest a listing waits for the browser to report the local scanners.
    pub list_timeout: Duration,
    /// With [`Self::network`], how long a first listing lets network scanners announce themselves
    /// (Bonjour has no "that was all" signal).
    pub network_settle: Duration,
    /// The longest a session takes to open, a functional unit to be selected, or the session to close.
    pub step_timeout: Duration,
    /// After a session opened, how long to wait for the scanner to say it is ready before reading
    /// its functional units anyway.
    pub ready_grace: Duration,
    /// The longest a running scan may go without a page, a progress change or its end.
    pub scan_idle_timeout: Duration,
}

impl Default for IcaOptions {
    /// Local scanners only; 5 s for a listing, 3 s for network scanners to settle, 30 s per
    /// session step, 3 s of grace for readiness and 3 minutes of silence during a scan.
    fn default() -> Self {
        Self {
            network: false,
            list_timeout: Duration::from_secs(5),
            network_settle: Duration::from_secs(3),
            step_timeout: Duration::from_secs(30),
            ready_grace: Duration::from_secs(3),
            scan_idle_timeout: Duration::from_secs(180),
        }
    }
}

setters!(IcaOptions {
    set with_network => network: bool,
    set with_list_timeout => list_timeout: Duration,
    set with_network_settle => network_settle: Duration,
    set with_step_timeout => step_timeout: Duration,
    set with_ready_grace => ready_grace: Duration,
    set with_scan_idle_timeout => scan_idle_timeout: Duration,
});
