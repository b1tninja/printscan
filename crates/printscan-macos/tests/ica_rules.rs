//! The ImageCaptureCore rules through the public API, on every platform: a scanner's functional
//! units as printscan capabilities, a request as unit settings, and the errors.

use printscan::scan::{Color, InterruptKind, PageSize, Presence, Region, ScanSettings, Source};
use printscan_macos::ica::caps;
use printscan_macos::ica::errors::{self, IcaError};
use printscan_macos::ica::model::{DeviceInfo, MeasurementUnit, PixelType, Transport, UnitInfo, UnitKind, document_type};
use printscan_macos::ica::plan;

const ICA: Option<&str> = Some("com.apple.ImageCaptureCore");

/// A letter-size flatbed that measures in points, as a scanner might publish it.
fn flatbed() -> UnitInfo {
    UnitInfo::new(UnitKind::Flatbed)
        .with_resolutions(vec![75, 100, 150, 200, 300, 600, 1200])
        .with_bit_depths(vec![1, 8, 16])
        .with_measurement_units(vec![MeasurementUnit::Inches, MeasurementUnit::Points])
        .with_measurement_unit(MeasurementUnit::Points)
        .with_physical_size((612.0, 792.0))
        .with_document_types(vec![document_type::US_LETTER, document_type::A4])
}

#[test]
fn capabilities_and_plan_agree() {
    let device = DeviceInfo::new("persistent-1", "Desk").with_transport(Transport::Usb);
    let scanner = caps::scanner(&device, &[UnitKind::Flatbed], &[flatbed()]);
    assert_eq!(scanner.flatbed_presence(), Presence::Present);
    assert_eq!(scanner.feeder_presence(), Presence::Absent);
    let glass = scanner.glass.clone().unwrap_or_default();
    // Every resolution and colour the capabilities list plans without an error.
    for dpi in &glass.dpi {
        for color in &glass.colors {
            let settings = ScanSettings::default().with_dpi(*dpi).with_color(*color).with_source(Source::Glass);
            assert!(plan::plan(&settings, &flatbed(), Source::Glass).is_ok(), "{dpi} dpi {color:?}");
        }
    }
    assert_eq!(glass.page_sizes, vec![PageSize::Letter, PageSize::A4]);
}

#[test]
fn a_region_on_a_letter_bed() {
    let settings = ScanSettings::default().with_dpi(300).with_region(Region::new(500, 500, 2000, 3000)).with_color(Color::Rgb);
    let planned = plan::plan(&settings, &flatbed(), Source::Glass).unwrap();
    assert_eq!(planned.setup.pixel_type, PixelType::Rgb);
    assert_eq!(planned.setup.measurement_unit, Some(MeasurementUnit::Inches));
    let area = planned.setup.scan_area.unwrap();
    assert_eq!((area.x, area.y, area.width, area.height), (0.5, 0.5, 2.0, 3.0));
}

#[test]
fn errors_name_their_cause() {
    let busy = errors::scan_error(&IcaError::new("com.apple.ImageCaptureCore", -9926, "", ICA), "opening the session");
    assert_eq!(busy.interrupt_kind(), InterruptKind::Busy);
    assert_eq!(busy.native().map(|n| n.to_string()), Some("ica -9926 (ICReturnScannerInUseByRemoteUser)".to_string()));
}
