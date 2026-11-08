//! Small safe functions over ImageCaptureCore, each wrapping the calls of one read or one request.
//! The objc2 bindings mark these methods `unsafe fn` because the header translator cannot prove
//! their contracts; every block here says which contract it relies on. See docs/ica-unsafe-audit.md.
//!
//! Every function here is called on the main thread (from a main-queue block or a delegate
//! callback), with objects that are retained for the whole call. None keeps a pointer past its
//! return: values are copied out into plain Rust data.

#![allow(unsafe_code)]

use std::path::PathBuf;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, MainThreadMarker};
use objc2_foundation::{
    NSArray, NSDate, NSDefaultRunLoopMode, NSError, NSIndexSet, NSNotFound, NSNumber, NSPoint, NSRect, NSRunLoop, NSSize, NSString, NSURL,
};
use objc2_image_capture_core::{
    ICDevice, ICDeviceBrowser, ICDeviceType, ICDeviceTypeMask, ICErrorDomain, ICScannerBitDepth, ICScannerDevice, ICScannerDocumentType,
    ICScannerFunctionalUnit, ICScannerFunctionalUnitDocumentFeeder, ICScannerFunctionalUnitFlatbed, ICScannerFunctionalUnitType,
    ICScannerMeasurementUnit, ICScannerPixelDataType, ICScannerTransferMode,
};
use printscan::scan::ScanError;

use super::delegate::Delegate;
use super::errors::IcaError;
use super::model::{self, DeviceInfo, MeasurementUnit, Transport, UnitInfo, UnitKind};
use super::plan::UnitSetup;
use super::session::Transfer;

/// `ICDeviceLocationTypeMaskLocal`.
const LOCATION_LOCAL: usize = 0x0000_0100;
/// `ICDeviceLocationTypeMaskShared | Bonjour | Bluetooth | Remote`: every location that is not local.
const LOCATION_NETWORK: usize = 0x0000_0200 | 0x0000_0400 | 0x0000_0800 | 0x0000_FE00;

/// The device-type mask a browser is given: scanners, local ones and with `network` the others too.
pub(crate) fn browse_mask(network: bool) -> usize {
    ICDeviceTypeMask::Scanner.0 | LOCATION_LOCAL | if network { LOCATION_NETWORK } else { 0 }
}

/// Run the main run loop for up to `slice`, delivering callbacks and main-queue blocks. Main thread only.
pub(crate) fn run_main_loop(_mtm: MainThreadMarker, slice: Duration) {
    let until = NSDate::dateWithTimeIntervalSinceNow(slice.as_secs_f64());
    // SAFETY: `NSDefaultRunLoopMode` is an immutable `NSString` constant exported by Foundation;
    // reading the extern static is sound once Foundation is loaded, which linking it guarantees.
    let mode = unsafe { NSDefaultRunLoopMode };
    if !NSRunLoop::mainRunLoop().runMode_beforeDate(mode, &until) {
        // No input source is attached yet: the run loop returned at once, so do not spin.
        std::thread::sleep(slice.min(Duration::from_millis(10)));
    }
}

/// The runtime value of `ICErrorDomain`.
fn ica_domain() -> Option<String> {
    // SAFETY: `ICErrorDomain` is an immutable, optional `NSString` constant exported by
    // ImageCaptureCore; reading it is sound once the framework is linked.
    let domain = unsafe { ICErrorDomain };
    domain.map(NSString::to_string)
}

/// An `NSError` as plain data.
pub(crate) fn ica_error(error: &NSError) -> IcaError {
    let domain = error.domain().to_string();
    let code = i64::try_from(error.code()).unwrap_or(i64::MIN);
    IcaError::new(domain, code, error.localizedDescription().to_string(), ica_domain().as_deref())
}

/// A device's native id: its `persistentIDString`, or its `UUIDString`.
pub(crate) fn device_id(device: &ICDevice) -> Option<String> {
    // SAFETY: two property getters with no arguments on a live `ICDevice`; they return retained
    // optional strings, copied out before the call returns.
    let (persistent, uuid) = unsafe { (device.persistentIDString(), device.UUIDString()) };
    persistent.or(uuid).map(|id| id.to_string()).filter(|id| !id.is_empty())
}

/// A listed device as plain data; `None` for a device that is not a scanner or has no id.
pub(crate) fn device_info(device: &ICDevice) -> Option<DeviceInfo> {
    // SAFETY: a property getter with no arguments on a live `ICDevice`.
    let kind = unsafe { device.r#type() };
    if kind != ICDeviceType::Scanner {
        return None;
    }
    let id = device_id(device)?;
    // SAFETY: property getters with no arguments on a live `ICDevice`; the strings are retained
    // and copied out before the call returns.
    let (name, transport, location, remote) = unsafe { (device.name(), device.transportType(), device.locationDescription(), device.isRemote()) };
    let name = name.map_or_else(|| "Scanner".to_string(), |name| name.to_string());
    let mut info = DeviceInfo::new(id, name)
        .with_transport(transport.map_or(Transport::Other, |transport| Transport::from_name(&transport.to_string())))
        .with_remote(remote);
    info.location = location.map(|location| location.to_string()).filter(|location| !location.is_empty());
    Some(info)
}

/// A new device browser with `delegate`, browsing `mask`. The delegate is weak in the browser, so
/// the caller keeps it alive.
pub(crate) fn new_browser(delegate: &Delegate, mask: usize) -> Retained<ICDeviceBrowser> {
    // SAFETY: `init` on a freshly allocated browser is its designated initialiser.
    let browser = unsafe { ICDeviceBrowser::init(ICDeviceBrowser::alloc()) };
    set_browser(&browser, delegate, mask);
    browser
}

/// Set the browser's delegate and mask and make sure it runs.
pub(crate) fn set_browser(browser: &ICDeviceBrowser, delegate: &Delegate, mask: usize) {
    let delegate = ProtocolObject::from_ref(delegate);
    // SAFETY: setters and `start`/`stop` on a live browser. The delegate implements
    // `ICDeviceBrowserDelegate` (delegate.rs) and is kept alive by the main-thread state, so the
    // browser's weak reference never dangles. A browser changes its mask only while stopped.
    unsafe {
        browser.setDelegate(Some(delegate));
        if browser.browsedDeviceTypeMask().0 != mask {
            if browser.isBrowsing() {
                browser.stop();
            }
            browser.setBrowsedDeviceTypeMask(ICDeviceTypeMask(mask));
        }
        if !browser.isBrowsing() {
            browser.start();
        }
    }
}

/// The devices the browser lists now.
pub(crate) fn browser_devices(browser: &ICDeviceBrowser) -> Vec<Retained<ICDevice>> {
    // SAFETY: a property getter with no arguments on a live browser; the array is retained.
    let devices = unsafe { browser.devices() };
    devices.map(|devices| devices.to_vec()).unwrap_or_default().into_iter().take(model::MAX_LISTED).collect()
}

/// Make `delegate` the device's delegate. The delegate is weak in the device; the main-thread state keeps it.
pub(crate) fn set_device_delegate(scanner: &ICScannerDevice, delegate: &Delegate) {
    let delegate = ProtocolObject::from_ref(delegate);
    // SAFETY: a setter on a live device; the delegate implements `ICDeviceDelegate` and
    // `ICScannerDeviceDelegate` (delegate.rs) and outlives the device's use of it.
    unsafe { scanner.setDelegate(Some(delegate)) };
}

/// Whether the device has a session open.
pub(crate) fn has_open_session(scanner: &ICScannerDevice) -> bool {
    // SAFETY: a property getter with no arguments on a live device.
    unsafe { scanner.hasOpenSession() }
}

/// `requestOpenSession`; the answer is `device:didOpenSessionWithError:`.
pub(crate) fn open_session(scanner: &ICScannerDevice) {
    // SAFETY: a request with no arguments on a live device, whose delegate was set first.
    unsafe { scanner.requestOpenSession() };
}

/// `requestCloseSession`; the answer is `device:didCloseSessionWithError:`.
pub(crate) fn close_session(scanner: &ICScannerDevice) {
    // SAFETY: a request with no arguments on a live device.
    unsafe { scanner.requestCloseSession() };
}

/// The functional unit types the scanner lists, known ones only.
pub(crate) fn unit_types(scanner: &ICScannerDevice) -> Vec<UnitKind> {
    // SAFETY: a property getter with no arguments on a live device; the array is retained.
    let types: Retained<NSArray<NSNumber>> = unsafe { scanner.availableFunctionalUnitTypes() };
    let mut kinds: Vec<UnitKind> =
        types.to_vec().into_iter().take(model::MAX_LISTED).filter_map(|value| UnitKind::from_raw(value.unsignedIntegerValue() as u64)).collect();
    kinds.dedup();
    kinds
}

/// The selected functional unit.
fn selected(scanner: &ICScannerDevice) -> Retained<ICScannerFunctionalUnit> {
    // SAFETY: a property getter with no arguments on a live device with an open session.
    unsafe { scanner.selectedFunctionalUnit() }
}

/// The kind of a functional unit.
pub(crate) fn unit_kind(unit: &ICScannerFunctionalUnit) -> Option<UnitKind> {
    // SAFETY: a property getter with no arguments on a live functional unit.
    let kind = unsafe { unit.r#type() };
    UnitKind::from_raw(kind.0 as u64)
}

/// The kind of the selected unit.
pub(crate) fn selected_kind(scanner: &ICScannerDevice) -> Option<UnitKind> {
    unit_kind(&selected(scanner))
}

/// `requestSelectFunctionalUnit:`; the answer is `scannerDevice:didSelectFunctionalUnit:error:`.
pub(crate) fn select_unit(scanner: &ICScannerDevice, kind: UnitKind) {
    let kind = ICScannerFunctionalUnitType(kind.raw() as usize);
    // SAFETY: a request on a live device with an open session, with a value of the enumeration it takes.
    unsafe { scanner.requestSelectFunctionalUnit(kind) };
}

/// The values of an index set.
fn index_set(set: &NSIndexSet) -> Vec<u32> {
    let not_found = NSNotFound as u64;
    model::walk_index_set(set.firstIndex() as u64, not_found, |index| set.indexGreaterThanIndex(index as usize) as u64)
}

/// A size as `(width, height)`, when both are finite.
fn size(size: NSSize) -> Option<(f64, f64)> {
    let (width, height) = (size.width, size.height);
    (width.is_finite() && height.is_finite()).then_some((width, height))
}

/// The selected functional unit as plain data; `None` when it is of a kind this crate does not know.
pub(crate) fn read_unit(scanner: &ICScannerDevice) -> Option<UnitInfo> {
    let unit = selected(scanner);
    let kind = unit_kind(&unit)?;
    // SAFETY: property getters with no arguments on a live functional unit; the index sets are
    // retained and walked (bounded) before the call returns, and every value is copied out.
    let mut info = unsafe {
        let native = (u32::try_from(unit.nativeXResolution()).unwrap_or(0), u32::try_from(unit.nativeYResolution()).unwrap_or(0));
        let mut info = UnitInfo::new(kind)
            .with_resolutions(index_set(&unit.supportedResolutions()))
            .with_preferred_resolutions(index_set(&unit.preferredResolutions()))
            .with_resolution(u32::try_from(unit.resolution()).unwrap_or(0))
            .with_bit_depths(index_set(&unit.supportedBitDepths()))
            .with_measurement_units(
                index_set(&unit.supportedMeasurementUnits()).into_iter().filter_map(|raw| MeasurementUnit::from_raw(u64::from(raw))).collect(),
            )
            .with_accepts_threshold(unit.acceptsThresholdForBlackAndWhiteScanning());
        info.measurement_unit = MeasurementUnit::from_raw(unit.measurementUnit().0 as u64);
        info.physical_size = size(unit.physicalSize());
        if native.0 > 0 && native.1 > 0 {
            info.native_resolution = Some(native);
        }
        if info.accepts_threshold {
            info.default_threshold = Some(unit.defaultThresholdForBlackAndWhiteScanning());
        }
        info
    };
    match kind {
        UnitKind::DocumentFeeder => {
            if let Ok(feeder) = unit.downcast::<ICScannerFunctionalUnitDocumentFeeder>() {
                // SAFETY: property getters with no arguments on a live document feeder unit.
                unsafe {
                    info.document_types = index_set(&feeder.supportedDocumentTypes()).into_iter().map(u64::from).collect();
                    info.supports_duplex = Some(feeder.supportsDuplexScanning());
                    info.document_loaded = Some(feeder.documentLoaded());
                }
            }
        }
        UnitKind::Flatbed => {
            if let Ok(flatbed) = unit.downcast::<ICScannerFunctionalUnitFlatbed>() {
                // SAFETY: a property getter with no arguments on a live flatbed unit.
                info.document_types = unsafe { index_set(&flatbed.supportedDocumentTypes()) }.into_iter().map(u64::from).collect();
            }
        }
        _ => {}
    }
    Some(info)
}

/// Write `setup` to the selected unit, in the order [`UnitSetup`] documents.
pub(crate) fn configure(scanner: &ICScannerDevice, setup: &UnitSetup) {
    let unit = selected(scanner);
    // SAFETY: property setters on a live functional unit, each with a value of the type and
    // enumeration it takes (the values come from the unit's own lists, see plan.rs). The rectangle
    // is a plain value.
    unsafe {
        if let Some(measure) = setup.measurement_unit {
            unit.setMeasurementUnit(ICScannerMeasurementUnit(measure.raw() as usize));
        }
        unit.setResolution(setup.resolution as usize);
        unit.setPixelDataType(ICScannerPixelDataType(setup.pixel_type.raw() as usize));
        unit.setBitDepth(ICScannerBitDepth(setup.bit_depth as usize));
        if let Some(threshold) = setup.threshold {
            unit.setUsesThresholdForBlackAndWhiteScanning(true);
            unit.setThresholdForBlackAndWhiteScanning(threshold);
        }
        if let Some(area) = setup.scan_area {
            unit.setScanArea(NSRect::new(NSPoint::new(area.x, area.y), NSSize::new(area.width, area.height)));
        }
    }
    let document = setup.document_type.map(|document| ICScannerDocumentType(document as usize));
    match unit.downcast::<ICScannerFunctionalUnitDocumentFeeder>() {
        Ok(feeder) => {
            // SAFETY: setters on a live document feeder unit with values of the types they take;
            // the document type is one the unit listed.
            unsafe {
                if let Some(document) = document {
                    feeder.setDocumentType(document);
                }
                if let Some(duplex) = setup.duplex {
                    feeder.setDuplexScanningEnabled(duplex);
                }
            }
        }
        Err(unit) => {
            if let (Some(document), Ok(flatbed)) = (document, unit.downcast::<ICScannerFunctionalUnitFlatbed>()) {
                // SAFETY: a setter on a live flatbed unit with a document type it listed.
                unsafe { flatbed.setDocumentType(document) };
            }
        }
    }
}

/// File-based transfer of `transfer`'s UTI into its directory, then `requestScan`.
pub(crate) fn start_scan(scanner: &ICScannerDevice, transfer: &Transfer) -> Result<(), ScanError> {
    let directory = NSURL::from_directory_path(&transfer.dir)
        .ok_or_else(|| ScanError::Failed(format!("{} cannot be given to ImageCaptureCore", transfer.dir.display())))?;
    let uti = NSString::from_str(transfer.uti);
    let name = NSString::from_str(&transfer.name);
    // SAFETY: setters on a live device with an open session, with retained values that outlive
    // the calls (the device copies or retains them), then a request with no arguments.
    unsafe {
        scanner.setTransferMode(ICScannerTransferMode::FileBased);
        scanner.setDownloadsDirectory(&directory);
        scanner.setDocumentUTI(&uti);
        scanner.setDocumentName(&name);
        scanner.requestScan();
    }
    Ok(())
}

/// The selected unit's `scanProgressPercentDone`.
pub(crate) fn progress(scanner: &ICScannerDevice) -> f64 {
    // SAFETY: a property getter with no arguments on a live functional unit.
    let done = unsafe { selected(scanner).scanProgressPercentDone() };
    if done.is_finite() { done } else { 0.0 }
}

/// `cancelScan`; the scan then ends with `scannerDevice:didCompleteScanWithError:`.
pub(crate) fn cancel_scan(scanner: &ICScannerDevice) {
    // SAFETY: a request with no arguments on a live device.
    unsafe { scanner.cancelScan() };
}

/// The local path of a file URL.
pub(crate) fn file_path(url: &NSURL) -> Option<PathBuf> {
    url.to_file_path()
}
