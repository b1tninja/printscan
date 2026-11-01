//! The process-wide registry with nothing registered: every operation is `Unsupported`.

use printscan::scan::{ScanError, ScanSettings};

#[test]
fn without_a_backend_everything_is_unsupported() {
    assert!(!printscan::scan::supported());
    assert!(printscan::scan::backend_names().is_empty());
    assert_eq!(printscan::scan::scanner_list().err(), Some(ScanError::Unsupported));
    assert_eq!(printscan::scan::scanners().err(), Some(ScanError::Unsupported));
    assert_eq!(printscan::scan::scanner_details("any:id").err(), Some(ScanError::Unsupported));
    assert_eq!(printscan::scan::status("any:id").err(), Some(ScanError::Unsupported));
    assert_eq!(printscan::scan::diagnostic_dump("").err(), Some(ScanError::Unsupported));
    assert_eq!(printscan::scan::acquire(&ScanSettings::default()).err(), Some(ScanError::Unsupported));
}

#[test]
fn the_unsupported_text_is_unchanged() {
    assert_eq!(ScanError::Unsupported.to_string(), "scanning is only supported on Windows (WIA) in this version");
}
