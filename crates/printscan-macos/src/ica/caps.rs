//! What a scanner's functional units publish, as printscan's capabilities: a [`DeviceInfo`] becomes
//! a [`ScannerRef`], and the units a session read become a [`Scanner`] with one [`ImageChoices`] per
//! source.
//!
//! * **Sources.** The flatbed is [`Source::Glass`](printscan::scan::Source::Glass) and the document feeder [`Source::Feeder`](printscan::scan::Source::Feeder). The
//!   transparency units have no printscan source and are left out of the capabilities (the
//!   diagnostic dump still lists them).
//! * **Resolutions** are `supportedResolutions` (or `preferredResolutions` when that is all the unit
//!   lists); optical resolution is `nativeXResolution` and `nativeYResolution`.
//! * **Colours.** ImageCaptureCore publishes bit depths, not colour modes: a unit with 8 bits offers
//!   grey and colour, and black and white too (from a 1-bit scan, or thresholded from grey by
//!   printscan when the unit has no 1-bit mode); a unit with only 1 bit offers black and white.
//! * **Formats** are the files ImageCaptureCore writes for the backend (TIFF, PNG, JPEG, BMP): the
//!   host encodes them, whatever the scanner sends.
//! * **Not published by ImageCaptureCore** and so recorded as absent: brightness, contrast,
//!   rotation, compression and intents (vendor features are not read).

use printscan::scan::{Area, ChoiceField, Color, Connection, ImageChoices, PageSize, Reported, Scanner, ScannerRef, ValueSpan};

use super::model::{DeviceInfo, Transport, UnitInfo, UnitKind, document_type};

/// The transfer formats the backend can ask ImageCaptureCore for, in the order it prefers them.
pub const FORMATS: [&str; 4] = ["tiff", "png", "jpeg", "bmp"];

/// The options ImageCaptureCore has no property for.
pub const NOT_PUBLISHED: [ChoiceField; 5] =
    [ChoiceField::Brightness, ChoiceField::Contrast, ChoiceField::Rotation, ChoiceField::Compression, ChoiceField::Intents];

/// How the device is attached, in printscan's words: USB is [`Connection::Usb`]; a network,
/// Bluetooth or FireWire scanner is [`Connection::Other`] (ImageCaptureCore does not say whether a
/// network scanner speaks eSCL or something else, so it is never called WSD).
pub fn connection(transport: Transport) -> Connection {
    match transport {
        Transport::Usb => Connection::Usb,
        Transport::Network | Transport::Bluetooth | Transport::FireWire => Connection::Other,
        _ => Connection::Unknown,
    }
}

/// The port text shown for a device: its location description, or its transport.
fn port(device: &DeviceInfo) -> String {
    device.location.clone().unwrap_or_else(|| device.transport.label().to_string())
}

/// A listed device as printscan shows it (native id, not yet namespaced).
pub fn scanner_ref(device: &DeviceInfo) -> ScannerRef {
    let mut found = ScannerRef::new(device.id.clone(), device.name.clone(), Some(port(device)));
    found.connection = connection(device.transport);
    found
}

/// The page size for an `ICScannerDocumentType` value, for the four printscan names.
pub fn page_size(document: u64) -> Option<PageSize> {
    match document {
        document_type::A4 => Some(PageSize::A4),
        document_type::US_LETTER => Some(PageSize::Letter),
        document_type::US_LEGAL => Some(PageSize::Legal),
        document_type::BUSINESS_CARD => Some(PageSize::BusinessCard),
        _ => None,
    }
}

/// The `ICScannerDocumentType` value for a page size, when ImageCaptureCore has one.
pub fn document_for(size: PageSize) -> Option<u64> {
    match size {
        PageSize::A4 => Some(document_type::A4),
        PageSize::Letter => Some(document_type::US_LETTER),
        PageSize::Legal => Some(document_type::US_LEGAL),
        PageSize::BusinessCard => Some(document_type::BUSINESS_CARD),
        PageSize::Auto => Some(document_type::DEFAULT),
        _ => None,
    }
}

/// The colours a unit with these bit depths can deliver through this backend. Empty when the unit
/// published no bit depth.
pub fn colors(bit_depths: &[u32]) -> Vec<Color> {
    let deep = bit_depths.iter().any(|depth| *depth >= 8);
    let one_bit = bit_depths.contains(&1);
    let mut colors = Vec::new();
    if deep {
        colors.extend([Color::Gray, Color::Rgb]);
    }
    if deep || one_bit {
        colors.push(Color::BlackAndWhite);
    }
    colors
}

/// One unit's choices.
pub fn choices(unit: &UnitInfo) -> ImageChoices {
    let mut choices = ImageChoices::default();
    choices.dpi = unit.offered_resolutions().to_vec();
    choices.colors = colors(&unit.bit_depths);
    if unit.accepts_threshold {
        choices.threshold = Some(ValueSpan::new(0, 255, 1));
    }
    choices.formats = FORMATS.iter().map(|format| (*format).to_string()).collect();
    let mut sizes: Vec<PageSize> = unit.document_types.iter().copied().filter_map(page_size).collect();
    sizes.dedup();
    choices.page_sizes = sizes;
    choices
}

/// The scanner a session read: `units` are the functional units it read, `listed` every unit type
/// it lists (a unit that failed to be read is still known to exist).
pub fn scanner(device: &DeviceInfo, listed: &[UnitKind], units: &[UnitInfo]) -> Scanner {
    let mut scanner = Scanner::new(device.id.clone(), device.name.clone());
    scanner.connection = connection(device.transport);
    scanner.port = Some(port(device));
    scanner.flatbed = listed.contains(&UnitKind::Flatbed);
    scanner.feeder = listed.contains(&UnitKind::DocumentFeeder);
    let mut reported = Reported::default();
    reported.units = true;
    for unit in units {
        let area = unit.area_thousandths().map(Area::from);
        match unit.kind {
            UnitKind::Flatbed => {
                scanner.glass = Some(choices(unit));
                scanner.bed = area;
                reported.glass_absent = NOT_PUBLISHED.to_vec();
                if scanner.optical_x.is_none() {
                    scanner.optical_x = unit.native_resolution.map(|(x, _)| x);
                    scanner.optical_y = unit.native_resolution.map(|(_, y)| y);
                }
            }
            UnitKind::DocumentFeeder => {
                scanner.feeder_choices = Some(choices(unit));
                scanner.sheet = area;
                scanner.duplex = unit.supports_duplex == Some(true);
                scanner.feeder_ready = unit.document_loaded;
                scanner.handling_status.feeder_ready = unit.document_loaded;
                reported.feeder_absent = NOT_PUBLISHED.to_vec();
                scanner.optical_x = scanner.optical_x.or(unit.native_resolution.map(|(x, _)| x));
                scanner.optical_y = scanner.optical_y.or(unit.native_resolution.map(|(_, y)| y));
            }
            _ => {}
        }
    }
    scanner.handling_status.flat_ready = scanner.flatbed;
    scanner.handling_status.duplex_ready = scanner.duplex;
    scanner.reported = reported;
    scanner
}

/// The diagnostic text for one device: what the browser said, every unit type it lists, and what
/// each unit that was read publishes.
pub fn describe(device: &DeviceInfo, listed: &[UnitKind], units: &[UnitInfo]) -> String {
    let mut out = format!(
        "{}  {:?}  transport {}{}{}
",
        device.id,
        device.name,
        device.transport.label(),
        device.location.as_deref().map(|location| format!(", at {location}")).unwrap_or_default(),
        if device.remote { ", remote" } else { "" }
    );
    let kinds: Vec<&str> = listed.iter().map(|kind| kind.label()).collect();
    out.push_str(&format!(
        "  units: {}
",
        kinds.join(", ")
    ));
    for unit in units {
        out.push_str(&format!(
            "  {}:
",
            unit.kind.label()
        ));
        out.push_str(&format!(
            "    resolutions {:?} (preferred {:?}, now {}, native {:?})
",
            unit.resolutions, unit.preferred_resolutions, unit.resolution, unit.native_resolution
        ));
        out.push_str(&format!(
            "    bit depths {:?}; colours {:?}
",
            unit.bit_depths,
            colors(&unit.bit_depths)
        ));
        out.push_str(&format!(
            "    size {:?} in {:?} (units offered {:?}); area {:?} thousandths
",
            unit.physical_size,
            unit.measurement_unit,
            unit.measurement_units,
            unit.area_thousandths()
        ));
        out.push_str(&format!(
            "    document types {:?}; threshold {} (default {:?})
",
            unit.document_types, unit.accepts_threshold, unit.default_threshold
        ));
        if unit.kind == UnitKind::DocumentFeeder {
            out.push_str(&format!(
                "    duplex {:?}; paper loaded {:?}
",
                unit.supports_duplex, unit.document_loaded
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use printscan::scan::{Presence, Source};

    use super::*;
    use crate::ica::model::MeasurementUnit;

    fn flatbed() -> UnitInfo {
        UnitInfo::new(UnitKind::Flatbed)
            .with_resolutions(vec![75, 150, 300, 600])
            .with_bit_depths(vec![1, 8])
            .with_measurement_unit(MeasurementUnit::Inches)
            .with_physical_size((8.5, 11.69))
            .with_native_resolution((1200, 2400))
            .with_document_types(vec![document_type::A4, document_type::US_LETTER, 99])
            .with_accepts_threshold(true)
    }

    fn feeder() -> UnitInfo {
        UnitInfo::new(UnitKind::DocumentFeeder)
            .with_preferred_resolutions(vec![150, 300])
            .with_bit_depths(vec![8])
            .with_measurement_unit(MeasurementUnit::Points)
            .with_physical_size((612.0, 1008.0))
            .with_supports_duplex(true)
            .with_document_loaded(false)
    }

    #[test]
    fn colours_follow_bit_depths() {
        assert_eq!(colors(&[8]), vec![Color::Gray, Color::Rgb, Color::BlackAndWhite]);
        assert_eq!(colors(&[1]), vec![Color::BlackAndWhite]);
        assert_eq!(colors(&[16]), vec![Color::Gray, Color::Rgb, Color::BlackAndWhite]);
        assert!(colors(&[]).is_empty());
    }

    #[test]
    fn page_sizes_round_trip() {
        for size in [PageSize::A4, PageSize::Letter, PageSize::Legal, PageSize::BusinessCard] {
            assert_eq!(document_for(size).and_then(page_size), Some(size));
        }
        assert_eq!(document_for(PageSize::Custom), None);
        assert_eq!(page_size(99), None);
    }

    #[test]
    fn a_flatbed_and_feeder_scanner() {
        let device = DeviceInfo::new("0x1234", "Desk scanner").with_transport(Transport::Usb).with_location("USB port 1".into());
        let listed = [UnitKind::Flatbed, UnitKind::DocumentFeeder, UnitKind::PositiveTransparency];
        let scanner = scanner(&device, &listed, &[flatbed(), feeder()]);
        assert_eq!(scanner.id, "0x1234");
        assert_eq!(scanner.connection, Connection::Usb);
        assert_eq!(scanner.port.as_deref(), Some("USB port 1"));
        assert!(scanner.flatbed && scanner.feeder && scanner.duplex);
        assert_eq!(scanner.feeder_ready, Some(false));
        assert_eq!(scanner.bed, Some(Area::new(8500, 11690)));
        assert_eq!(scanner.sheet, Some(Area::new(8500, 14000)));
        assert_eq!((scanner.optical_x, scanner.optical_y), (Some(1200), Some(2400)));
        let glass = scanner.glass.as_ref().map(|g| (g.dpi.clone(), g.page_sizes.clone(), g.threshold));
        assert_eq!(glass, Some((vec![75, 150, 300, 600], vec![PageSize::A4, PageSize::Letter], Some(ValueSpan::new(0, 255, 1)))));
        assert_eq!(scanner.feeder_choices.as_ref().map(|f| f.dpi.clone()), Some(vec![150, 300]));
        assert_eq!(scanner.feeder_presence(), Presence::Present);
        assert_eq!(scanner.choice_presence(Source::Glass, ChoiceField::Dpi), Presence::Present);
        assert_eq!(scanner.choice_presence(Source::Glass, ChoiceField::Brightness), Presence::Absent);
        assert_eq!(scanner.choice_presence(Source::Feeder, ChoiceField::Threshold), Presence::Unknown);
    }

    #[test]
    fn a_feeder_only_network_scanner() {
        let device = DeviceInfo::new("uuid-1", "Office").with_transport(Transport::Network).with_remote(true);
        let scanner = scanner(&device, &[UnitKind::DocumentFeeder], &[feeder()]);
        assert_eq!(scanner.connection, Connection::Other);
        assert_eq!(scanner.port.as_deref(), Some("network"));
        assert_eq!(scanner.flatbed_presence(), Presence::Absent);
        assert!(scanner.glass.is_none());
        let found = scanner_ref(&device);
        assert_eq!((found.id.as_str(), found.connection), ("uuid-1", Connection::Other));
        let text = describe(&device, &[UnitKind::DocumentFeeder], &[feeder()]);
        assert!(text.contains("uuid-1") && text.contains("remote") && text.contains("paper loaded Some(false)"));
    }
}
