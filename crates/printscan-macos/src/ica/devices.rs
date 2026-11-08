//! The scanners the device browser has reported, kept as plain data while the browser runs, and
//! the rules for when a listing has waited long enough and which device a request names.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::time::Instant;

use printscan::scan::ScanError;

use super::errors;
use super::model::DeviceInfo;
use super::options::IcaOptions;

/// The most devices kept: a browser that reports more is not trusted with memory.
pub(crate) const MAX_DEVICES: usize = 64;

/// What the browser has reported since it started.
#[derive(Debug, Default)]
pub(crate) struct DeviceTable {
    devices: Vec<DeviceInfo>,
    /// `deviceBrowserDidEnumerateLocalDevices:` has arrived.
    local_done: bool,
    /// When the browser was started, and whether with network scanners.
    started: Option<(Instant, bool)>,
}

impl DeviceTable {
    /// A device appeared, or one already listed changed (same id).
    pub(crate) fn add(&mut self, device: DeviceInfo) {
        if let Some(known) = self.devices.iter_mut().find(|known| known.id == device.id) {
            *known = device;
        } else if self.devices.len() < MAX_DEVICES {
            self.devices.push(device);
        }
    }

    /// A device went away.
    pub(crate) fn remove(&mut self, id: &str) {
        self.devices.retain(|device| device.id != id);
    }

    /// The browser has reported every local device.
    pub(crate) fn local_enumerated(&mut self) {
        self.local_done = true;
    }

    /// The browser was started at `now`; `network` says whether network scanners are browsed too.
    /// A later start that adds network scanners restarts the settle time.
    pub(crate) fn started(&mut self, now: Instant, network: bool) {
        match self.started {
            Some((_, true)) => {}
            Some((_, false)) if !network => {}
            _ => self.started = Some((now, network)),
        }
    }

    /// Whether the browser was started (with network scanners when `network`).
    pub(crate) fn is_browsing(&self, network: bool) -> bool {
        self.started.is_some_and(|(_, with_network)| with_network || !network)
    }

    /// The devices, in the order they were first reported.
    pub(crate) fn devices(&self) -> Vec<DeviceInfo> {
        self.devices.clone()
    }

    /// Whether a listing can answer now: the local devices are in (and, with network scanners,
    /// they had `network_settle` to announce themselves), or `list_timeout` has passed.
    pub(crate) fn settled(&self, now: Instant, options: &IcaOptions) -> bool {
        let Some((started, _)) = self.started else { return false };
        let waited = now.saturating_duration_since(started);
        if waited >= options.list_timeout {
            return true;
        }
        self.local_done && (!options.network || waited >= options.network_settle)
    }
}

/// The device a request names: an exact native id; else, when `wanted` is empty, the only or the
/// first device; else the one device whose id or name contains `wanted` (ignoring case).
pub(crate) fn pick<'a>(devices: &'a [DeviceInfo], wanted: &str) -> Result<&'a DeviceInfo, ScanError> {
    if let Some(exact) = devices.iter().find(|device| device.id == wanted) {
        return Ok(exact);
    }
    let needle = wanted.trim().to_lowercase();
    if needle.is_empty() {
        return devices.first().ok_or_else(|| ScanError::Failed("no ImageCaptureCore scanner is connected".into()));
    }
    let mut matching = devices.iter().filter(|device| device.id.to_lowercase().contains(&needle) || device.name.to_lowercase().contains(&needle));
    match (matching.next(), matching.next()) {
        (Some(only), None) => Ok(only),
        (Some(_), Some(_)) => Err(ScanError::Failed(format!("several ImageCaptureCore scanners match {wanted:?}; use the id"))),
        (None, _) => Err(errors::no_such_device(wanted)),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn table_tracks_adds_changes_and_removals() {
        let mut table = DeviceTable::default();
        table.add(DeviceInfo::new("a", "First"));
        table.add(DeviceInfo::new("b", "Second"));
        table.add(DeviceInfo::new("a", "Renamed"));
        assert_eq!(table.devices().iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Renamed", "Second"]);
        table.remove("a");
        assert_eq!(table.devices().len(), 1);
        for index in 0..100 {
            table.add(DeviceInfo::new(format!("x{index}"), "many"));
        }
        assert_eq!(table.devices().len(), MAX_DEVICES);
    }

    #[test]
    fn listing_settles_on_local_enumeration_or_timeout() {
        let options = IcaOptions::default();
        let start = Instant::now();
        let mut table = DeviceTable::default();
        assert!(!table.settled(start, &options));
        table.started(start, false);
        assert!(table.is_browsing(false) && !table.is_browsing(true));
        assert!(!table.settled(start, &options));
        assert!(table.settled(start + options.list_timeout, &options));
        table.local_enumerated();
        assert!(table.settled(start, &options));
        // With network scanners, local enumeration is not enough until they had time to settle.
        let network = options.with_network(true);
        let later = start + Duration::from_secs(1);
        table.started(later, true);
        assert!(!table.settled(later, &network));
        assert!(table.settled(later + network.network_settle, &network));
        // A local start after a network one changes nothing.
        table.started(later + Duration::from_secs(9), false);
        assert!(table.settled(later + network.network_settle, &network));
    }

    #[test]
    fn requests_name_one_device() {
        let devices = [DeviceInfo::new("usb-1", "Desk Scanner"), DeviceInfo::new("net-2", "Office Scanner")];
        assert_eq!(pick(&devices, "net-2").map(|d| d.id.as_str()), Ok("net-2"));
        assert_eq!(pick(&devices, "").map(|d| d.id.as_str()), Ok("usb-1"));
        assert_eq!(pick(&devices, "office").map(|d| d.id.as_str()), Ok("net-2"));
        assert!(pick(&devices, "scanner").is_err());
        assert!(pick(&devices, "printer").is_err_and(|e| e.native() == Some(errors::NO_SUCH_DEVICE)));
        assert!(pick(&[], "").is_err());
    }
}
