//! The Objective-C object ImageCaptureCore calls back into: one class that is the delegate of the
//! device browser and of every scanner device. Each callback turns its arguments into plain data
//! (through `ffi`) and hands it to the event sink (`main_thread`); nothing else happens in a
//! callback, so none of them can block the main thread or reach the main-thread state.
//!
//! `define_class!` needs `unsafe` for the protocol conformances it declares: the methods must have
//! the selectors and argument types the protocols name. See docs/ica-unsafe-audit.md.

#![allow(unsafe_code)]

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSError, NSObjectProtocol, NSURL};
use objc2_image_capture_core::{
    ICDevice, ICDeviceBrowser, ICDeviceBrowserDelegate, ICDeviceDelegate, ICScannerDevice, ICScannerDeviceDelegate, ICScannerFunctionalUnit,
};
use printscan::scan::ScanError;

use super::ffi;
use super::main_thread::sink;
use super::session::Event;

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `Delegate` has no ivars and does not implement `Drop`.
    // - It is main-thread-only: ImageCaptureCore calls its delegates on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrintscanIcaDelegate"]
    pub(crate) struct Delegate;

    // SAFETY: `NSObjectProtocol` has no requirements beyond being an `NSObject`.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: every method has the selector and the argument types of `ICDeviceBrowserDelegate`,
    // the two required methods included.
    unsafe impl ICDeviceBrowserDelegate for Delegate {
        #[unsafe(method(deviceBrowser:didAddDevice:moreComing:))]
        fn browser_added(&self, _browser: &ICDeviceBrowser, device: &ICDevice, _more_coming: bool) {
            if let Some(info) = ffi::device_info(device) {
                sink().device_added(info);
            }
        }

        #[unsafe(method(deviceBrowser:didRemoveDevice:moreGoing:))]
        fn browser_removed(&self, _browser: &ICDeviceBrowser, device: &ICDevice, _more_going: bool) {
            if let Some(id) = ffi::device_id(device) {
                sink().device_removed(&id);
            }
        }

        #[unsafe(method(deviceBrowser:deviceDidChangeName:))]
        fn browser_renamed(&self, _browser: &ICDeviceBrowser, device: &ICDevice) {
            if let Some(info) = ffi::device_info(device) {
                sink().device_added(info);
            }
        }

        #[unsafe(method(deviceBrowserDidEnumerateLocalDevices:))]
        fn browser_enumerated(&self, _browser: &ICDeviceBrowser) {
            sink().local_enumerated();
        }
    }

    // SAFETY: every method has the selector and the argument types of `ICDeviceDelegate`, the
    // three required methods included; the errors are nullable and taken as `Option`.
    unsafe impl ICDeviceDelegate for Delegate {
        #[unsafe(method(device:didCloseSessionWithError:))]
        fn closed(&self, device: &ICDevice, error: Option<&NSError>) {
            send(device, Event::Closed(error.map(ffi::ica_error)));
        }

        #[unsafe(method(didRemoveDevice:))]
        fn removed(&self, device: &ICDevice) {
            if let Some(id) = ffi::device_id(device) {
                sink().device_removed(&id);
            }
        }

        #[unsafe(method(device:didOpenSessionWithError:))]
        fn opened(&self, device: &ICDevice, error: Option<&NSError>) {
            send(device, Event::Opened(error.map(ffi::ica_error)));
        }

        #[unsafe(method(deviceDidBecomeReady:))]
        fn ready(&self, device: &ICDevice) {
            send(device, Event::Ready);
        }

        #[unsafe(method(device:didEncounterError:))]
        fn encountered(&self, device: &ICDevice, error: Option<&NSError>) {
            if let Some(error) = error {
                send(device, Event::DeviceError(ffi::ica_error(error)));
            }
        }
    }

    // SAFETY: every method has the selector and the argument types of `ICScannerDeviceDelegate`
    // (all of its methods are optional); the errors are nullable and taken as `Option`.
    unsafe impl ICScannerDeviceDelegate for Delegate {
        #[unsafe(method(scannerDeviceDidBecomeAvailable:))]
        fn available(&self, scanner: &ICScannerDevice) {
            send(scanner, Event::Ready);
        }

        #[unsafe(method(scannerDevice:didSelectFunctionalUnit:error:))]
        fn selected(&self, scanner: &ICScannerDevice, unit: &ICScannerFunctionalUnit, error: Option<&NSError>) {
            send(scanner, Event::Selected { unit: ffi::unit_kind(unit), error: error.map(ffi::ica_error) });
        }

        #[unsafe(method(scannerDevice:didScanToURL:))]
        fn scanned(&self, scanner: &ICScannerDevice, url: &NSURL) {
            let event = match ffi::file_path(url) {
                Some(path) => Event::Scanned(path),
                None => Event::Refused(ScanError::Failed("ImageCaptureCore delivered a page that is not a local file".into())),
            };
            send(scanner, event);
        }

        #[unsafe(method(scannerDevice:didCompleteScanWithError:))]
        fn completed(&self, scanner: &ICScannerDevice, error: Option<&NSError>) {
            send(scanner, Event::Completed(error.map(ffi::ica_error)));
        }
    }
);

/// Hand `event` to whoever drives a session with `device`.
fn send(device: &ICDevice, event: Event) {
    if let Some(id) = ffi::device_id(device) {
        sink().push(&id, event);
    }
}

impl Delegate {
    /// A new delegate. Main thread only.
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(());
        // SAFETY: `init` is `NSObject`'s designated initialiser, sent once to a fresh allocation.
        unsafe { msg_send![super(this), init] }
    }
}
