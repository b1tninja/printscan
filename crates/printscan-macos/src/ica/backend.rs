//! [`Ica`]: the `printscan::scan::Backend` over the device browser and the session machines.
//! Everything here is safe code: Objective-C is reached through `main_thread` and `ffi` only.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use printscan::scan::{Acquired, Backend, ScanError, ScanSettings, Scanner, ScannerRef};

use super::devices;
use super::errors;
use super::main_thread::{self, MainPort};
use super::model::DeviceInfo;
use super::session::{self, Acquire, Probe, Probed};
use super::{IcaOptions, NAME, caps};

/// One session at a time in the process: ImageCaptureCore sessions are per device, but the
/// selected functional unit and the transfer settings are the device's, so two callers must not
/// interleave on one device, and serialising every session keeps the rule simple.
static SESSION: Mutex<()> = Mutex::new(());

/// ImageCaptureCore scanners as a `printscan::scan` backend, named `"ica"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Ica {
    options: IcaOptions,
}

impl Ica {
    /// The backend with default options: local scanners only.
    pub fn new() -> Self {
        Self::default()
    }

    /// The backend with `options` (network scanners, deadlines).
    pub fn with_options(options: IcaOptions) -> Self {
        Self { options }
    }

    /// The options in use.
    pub fn options(&self) -> IcaOptions {
        self.options
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, ScanError> {
        main_thread::devices(&self.options)
    }

    fn device(&self, native_id: &str) -> Result<DeviceInfo, ScanError> {
        self.devices()?.into_iter().find(|device| device.id == native_id).ok_or_else(|| errors::no_such_device(native_id))
    }

    fn probe(&self, device: &DeviceInfo) -> Result<Probed, ScanError> {
        let _session = SESSION.lock().unwrap_or_else(PoisonError::into_inner);
        let mut port = MainPort::new(&device.id);
        session::drive(Probe::new(self.options, Instant::now()), &mut port, &mut Instant::now)
    }

    fn details(&self, device: &DeviceInfo) -> Result<Scanner, ScanError> {
        let probed = self.probe(device)?;
        Ok(caps::scanner(device, &probed.listed, &probed.units))
    }
}

impl Backend for Ica {
    fn name(&self) -> &'static str {
        NAME
    }

    fn scanner_list(&self) -> Result<Vec<ScannerRef>, ScanError> {
        Ok(self.devices()?.iter().map(caps::scanner_ref).collect())
    }

    fn scanner_details(&self, native_id: &str) -> Result<Scanner, ScanError> {
        self.details(&self.device(native_id)?)
    }

    fn scanners(&self) -> Result<Vec<Scanner>, ScanError> {
        let mut found = Vec::new();
        for device in self.devices()? {
            match self.details(&device) {
                Ok(scanner) => found.push(scanner),
                Err(error) => log::debug!("scan: {} left out: {error}", device.id),
            }
        }
        Ok(found)
    }

    fn diagnostic_dump(&self, filter: &str) -> Result<String, ScanError> {
        let needle = filter.trim().to_lowercase();
        let mut out = String::new();
        for device in self.devices()? {
            if !needle.is_empty() && !device.id.to_lowercase().contains(&needle) && !device.name.to_lowercase().contains(&needle) {
                continue;
            }
            match self.probe(&device) {
                Ok(probed) => out.push_str(&caps::describe(&device, &probed.listed, &probed.units)),
                Err(error) => out.push_str(&format!("{}  {:?}  error: {error}\n", device.id, device.name)),
            }
        }
        if out.is_empty() {
            out.push_str("no ImageCaptureCore scanner matches\n");
        }
        Ok(out)
    }

    fn acquire(&self, settings: &ScanSettings) -> Result<Acquired, ScanError> {
        settings.checked()?;
        let listed = self.devices()?;
        let device = devices::pick(&listed, &settings.device)?.clone();
        let _session = SESSION.lock().unwrap_or_else(PoisonError::into_inner);
        let mut port = MainPort::new(&device.id);
        if let Some(token) = &settings.cancel {
            let id = device.id.clone();
            token.set_canceller(Arc::new(move || main_thread::cancel(&id)));
        }
        let mut routed = settings.clone();
        routed.device = device.id.clone();
        let result = session::drive(Acquire::new(routed, self.options, Instant::now()), &mut port, &mut Instant::now);
        if let Some(token) = &settings.cancel {
            token.clear_canceller();
        }
        result
    }
}
