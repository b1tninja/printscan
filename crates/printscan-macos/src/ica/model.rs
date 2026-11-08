//! What the backend reads from ImageCaptureCore, as plain Rust data: a device as the browser lists
//! it ([`DeviceInfo`]) and a functional unit as a session reads it ([`UnitInfo`]). The numeric
//! values are those of ImageCaptureCore's public enumerations; the Objective-C layer reads them and
//! everything that decides what they mean lives here and in [`super::caps`] and [`super::plan`], so
//! it is tested on every platform.

/// A scanner's functional unit (`ICScannerFunctionalUnitType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UnitKind {
    /// The flatbed glass (0).
    Flatbed,
    /// A transparency unit for positives (1). Listed, never scanned from: printscan has no source for it.
    PositiveTransparency,
    /// A transparency unit for negatives (2). Listed, never scanned from.
    NegativeTransparency,
    /// The automatic document feeder (3).
    DocumentFeeder,
}

impl UnitKind {
    /// The `ICScannerFunctionalUnitType` value.
    pub fn raw(self) -> u64 {
        match self {
            Self::Flatbed => 0,
            Self::PositiveTransparency => 1,
            Self::NegativeTransparency => 2,
            Self::DocumentFeeder => 3,
        }
    }

    /// The unit for an `ICScannerFunctionalUnitType` value; `None` for a value this crate does not know.
    pub fn from_raw(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Flatbed),
            1 => Some(Self::PositiveTransparency),
            2 => Some(Self::NegativeTransparency),
            3 => Some(Self::DocumentFeeder),
            _ => None,
        }
    }

    /// A short name for messages and the diagnostic dump: `flatbed`, `positive transparency`,
    /// `negative transparency` or `document feeder`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Flatbed => "flatbed",
            Self::PositiveTransparency => "positive transparency",
            Self::NegativeTransparency => "negative transparency",
            Self::DocumentFeeder => "document feeder",
        }
    }
}

/// The unit a functional unit measures its sizes and scan area in (`ICScannerMeasurementUnit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MeasurementUnit {
    Inches,
    Centimeters,
    /// 1/6 inch.
    Picas,
    /// 1/72 inch.
    Points,
    /// 1/1440 inch.
    Twips,
    /// Pixels at the unit's current resolution.
    Pixels,
}

impl MeasurementUnit {
    /// The `ICScannerMeasurementUnit` value.
    pub fn raw(self) -> u64 {
        match self {
            Self::Inches => 0,
            Self::Centimeters => 1,
            Self::Picas => 2,
            Self::Points => 3,
            Self::Twips => 4,
            Self::Pixels => 5,
        }
    }

    /// The unit for an `ICScannerMeasurementUnit` value; `None` for any other value.
    pub fn from_raw(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Inches),
            1 => Some(Self::Centimeters),
            2 => Some(Self::Picas),
            3 => Some(Self::Points),
            4 => Some(Self::Twips),
            5 => Some(Self::Pixels),
            _ => None,
        }
    }

    /// How many of this unit make an inch, with `dpi` for [`Self::Pixels`]; `None` for pixels at no
    /// resolution.
    pub fn per_inch(self, dpi: u32) -> Option<f64> {
        match self {
            Self::Inches => Some(1.0),
            Self::Centimeters => Some(2.54),
            Self::Picas => Some(6.0),
            Self::Points => Some(72.0),
            Self::Twips => Some(1440.0),
            Self::Pixels => (dpi > 0).then_some(f64::from(dpi)),
        }
    }

    /// `value` of this unit in thousandths of an inch, rounded; `None` for a negative, non-finite or
    /// too large value, or for pixels at no resolution.
    pub fn to_thousandths(self, value: f64, dpi: u32) -> Option<u32> {
        let per_inch = self.per_inch(dpi)?;
        let thousandths = (value / per_inch * 1000.0).round();
        (thousandths.is_finite() && (0.0..=f64::from(u32::MAX)).contains(&thousandths)).then_some(thousandths as u32)
    }

    /// `thousandths` of an inch in this unit; `None` for pixels at no resolution.
    pub fn from_thousandths(self, thousandths: u32, dpi: u32) -> Option<f64> {
        Some(f64::from(thousandths) / 1000.0 * self.per_inch(dpi)?)
    }
}

/// The pixels a functional unit is asked for (`ICScannerPixelDataType`). Only the three printscan
/// asks for are named.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PixelType {
    /// Black and white (0), at a bit depth of 1.
    BlackAndWhite,
    /// Grey (1).
    Gray,
    /// Colour (2).
    Rgb,
}

impl PixelType {
    /// The `ICScannerPixelDataType` value.
    pub fn raw(self) -> u64 {
        match self {
            Self::BlackAndWhite => 0,
            Self::Gray => 1,
            Self::Rgb => 2,
        }
    }
}

/// `ICScannerDocumentType` values for the page sizes printscan names.
pub mod document_type {
    /// The unit's default document type.
    pub const DEFAULT: u64 = 0;
    pub const A4: u64 = 1;
    pub const US_LETTER: u64 = 3;
    pub const US_LEGAL: u64 = 4;
    pub const BUSINESS_CARD: u64 = 53;
}

/// How a device is attached, from its `transportType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Transport {
    Usb,
    FireWire,
    Bluetooth,
    /// A network scanner (`ICTransportTypeTCPIP`): Bonjour, AirScan or a shared scanner.
    Network,
    /// A transport this crate does not name, or none was reported.
    #[default]
    Other,
}

impl Transport {
    /// The transport for an `ICDeviceTransport` string (`ICTransportTypeUSB` is `"ICTransportTypeUSB"`).
    pub fn from_name(name: &str) -> Self {
        match name {
            "ICTransportTypeUSB" => Self::Usb,
            "ICTransportTypeFireWire" => Self::FireWire,
            "ICTransportTypeBluetooth" => Self::Bluetooth,
            "ICTransportTypeTCPIP" => Self::Network,
            _ => Self::Other,
        }
    }

    /// `usb`, `firewire`, `bluetooth`, `network` or `other`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Usb => "usb",
            Self::FireWire => "firewire",
            Self::Bluetooth => "bluetooth",
            Self::Network => "network",
            Self::Other => "other",
        }
    }
}

/// A scanner as the device browser lists it. Read without opening a session.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct DeviceInfo {
    /// The native id: the device's `persistentIDString`, or its `UUIDString` when it has none.
    pub id: String,
    pub name: String,
    pub transport: Transport,
    /// The device's `locationDescription`, when it has one.
    pub location: Option<String>,
    /// The device is not attached to this Mac (`isRemote`).
    pub remote: bool,
}

impl DeviceInfo {
    /// A device with native id `id` called `name`, attached by an unknown transport.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self { id: id.into(), name: name.into(), ..Self::default() }
    }
}

setters!(DeviceInfo {
    set with_transport => transport: Transport,
    some with_location => location: String,
    set with_remote => remote: bool,
});

/// One functional unit as a session reads it. Lists are empty and options `None` when the unit did
/// not publish them. Sizes are in [`Self::measurement_unit`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct UnitInfo {
    pub kind: UnitKind,
    /// `supportedResolutions`, in dots per inch.
    pub resolutions: Vec<u32>,
    /// `preferredResolutions`.
    pub preferred_resolutions: Vec<u32>,
    /// The resolution set now.
    pub resolution: u32,
    /// `nativeXResolution` and `nativeYResolution`, when positive.
    pub native_resolution: Option<(u32, u32)>,
    /// `supportedBitDepths`: 1, 8 or 16.
    pub bit_depths: Vec<u32>,
    /// `supportedMeasurementUnits`.
    pub measurement_units: Vec<MeasurementUnit>,
    /// The unit sizes are in now; `None` for a value this crate does not know.
    pub measurement_unit: Option<MeasurementUnit>,
    /// `physicalSize`: width and height of the unit's whole area.
    pub physical_size: Option<(f64, f64)>,
    /// `supportedDocumentTypes` of a flatbed or feeder, as `ICScannerDocumentType` values.
    pub document_types: Vec<u64>,
    /// A feeder's `supportsDuplexScanning`; `None` for other units.
    pub supports_duplex: Option<bool>,
    /// A feeder's `documentLoaded`; `None` for other units.
    pub document_loaded: Option<bool>,
    /// `acceptsThresholdForBlackAndWhiteScanning`.
    pub accepts_threshold: bool,
    /// `defaultThresholdForBlackAndWhiteScanning`, when the unit accepts a threshold.
    pub default_threshold: Option<u8>,
}

impl UnitInfo {
    /// A unit of `kind` that published nothing.
    pub fn new(kind: UnitKind) -> Self {
        Self {
            kind,
            resolutions: Vec::new(),
            preferred_resolutions: Vec::new(),
            resolution: 0,
            native_resolution: None,
            bit_depths: Vec::new(),
            measurement_units: Vec::new(),
            measurement_unit: None,
            physical_size: None,
            document_types: Vec::new(),
            supports_duplex: None,
            document_loaded: None,
            accepts_threshold: false,
            default_threshold: None,
        }
    }

    /// The resolutions to offer: `supportedResolutions`, or `preferredResolutions` when the unit
    /// lists only those.
    pub fn offered_resolutions(&self) -> &[u32] {
        if self.resolutions.is_empty() { &self.preferred_resolutions } else { &self.resolutions }
    }

    /// The unit's whole area in thousandths of an inch, when it published a size in a unit this crate knows.
    pub fn area_thousandths(&self) -> Option<(u32, u32)> {
        let unit = self.measurement_unit?;
        let (width, height) = self.physical_size?;
        let width = unit.to_thousandths(width, self.resolution)?;
        let height = unit.to_thousandths(height, self.resolution)?;
        (width > 0 && height > 0).then_some((width, height))
    }
}

setters!(UnitInfo {
    set with_resolutions => resolutions: Vec<u32>,
    set with_preferred_resolutions => preferred_resolutions: Vec<u32>,
    set with_resolution => resolution: u32,
    some with_native_resolution => native_resolution: (u32, u32),
    set with_bit_depths => bit_depths: Vec<u32>,
    set with_measurement_units => measurement_units: Vec<MeasurementUnit>,
    some with_measurement_unit => measurement_unit: MeasurementUnit,
    some with_physical_size => physical_size: (f64, f64),
    set with_document_types => document_types: Vec<u64>,
    some with_supports_duplex => supports_duplex: bool,
    some with_document_loaded => document_loaded: bool,
    set with_accepts_threshold => accepts_threshold: bool,
    some with_default_threshold => default_threshold: u8,
});

/// The most values read from one `NSIndexSet` or `NSArray`: a device that publishes more is cut
/// short rather than trusted.
pub const MAX_LISTED: usize = 256;

/// The values of an index set walked by `next` (`firstIndex`, then `indexGreaterThanIndex:`), which
/// answers `not_found` after the last; at most [`MAX_LISTED`] of them, and only those that fit `u32`.
pub fn walk_index_set(first: u64, not_found: u64, mut next: impl FnMut(u64) -> u64) -> Vec<u32> {
    let mut values = Vec::new();
    let mut index = first;
    while index != not_found && values.len() < MAX_LISTED {
        if let Ok(value) = u32::try_from(index) {
            values.push(value);
        }
        let following = next(index);
        if following <= index {
            // A well-behaved set only goes up; anything else would loop.
            break;
        }
        index = following;
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_round_trip_and_convert() {
        for unit in [MeasurementUnit::Inches, MeasurementUnit::Centimeters, MeasurementUnit::Points, MeasurementUnit::Pixels] {
            assert_eq!(MeasurementUnit::from_raw(unit.raw()), Some(unit));
        }
        assert_eq!(MeasurementUnit::Inches.to_thousandths(8.5, 0), Some(8500));
        assert_eq!(MeasurementUnit::Centimeters.to_thousandths(21.0, 0), Some(8268));
        assert_eq!(MeasurementUnit::Points.to_thousandths(612.0, 0), Some(8500));
        assert_eq!(MeasurementUnit::Twips.to_thousandths(1440.0, 0), Some(1000));
        assert_eq!(MeasurementUnit::Picas.to_thousandths(3.0, 0), Some(500));
        assert_eq!(MeasurementUnit::Pixels.to_thousandths(2550.0, 300), Some(8500));
        assert_eq!(MeasurementUnit::Pixels.to_thousandths(2550.0, 0), None);
        assert_eq!(MeasurementUnit::Inches.to_thousandths(-1.0, 0), None);
        assert_eq!(MeasurementUnit::Inches.to_thousandths(f64::NAN, 0), None);
        assert_eq!(MeasurementUnit::Inches.to_thousandths(1e12, 0), None);
        assert_eq!(MeasurementUnit::Points.from_thousandths(1000, 0), Some(72.0));
        assert_eq!(MeasurementUnit::from_raw(9), None);
    }

    #[test]
    fn unit_kinds_and_transports() {
        for kind in [UnitKind::Flatbed, UnitKind::PositiveTransparency, UnitKind::NegativeTransparency, UnitKind::DocumentFeeder] {
            assert_eq!(UnitKind::from_raw(kind.raw()), Some(kind));
        }
        assert_eq!(UnitKind::from_raw(4), None);
        assert_eq!(Transport::from_name("ICTransportTypeUSB"), Transport::Usb);
        assert_eq!(Transport::from_name("ICTransportTypeTCPIP"), Transport::Network);
        assert_eq!(Transport::from_name("something else"), Transport::Other);
    }

    #[test]
    fn area_needs_a_known_unit_and_size() {
        let unit = UnitInfo::new(UnitKind::Flatbed).with_measurement_unit(MeasurementUnit::Inches).with_physical_size((8.5, 11.69));
        assert_eq!(unit.area_thousandths(), Some((8500, 11690)));
        assert_eq!(UnitInfo::new(UnitKind::Flatbed).with_physical_size((8.5, 11.0)).area_thousandths(), None);
        let zero = UnitInfo::new(UnitKind::Flatbed).with_measurement_unit(MeasurementUnit::Inches).with_physical_size((0.0, 11.0));
        assert_eq!(zero.area_thousandths(), None);
    }

    #[test]
    fn index_sets_are_walked_with_bounds() {
        let set = [75u64, 150, 300, 600];
        let next = |i: u64| set.iter().copied().find(|v| *v > i).unwrap_or(u64::MAX);
        assert_eq!(walk_index_set(75, u64::MAX, next), vec![75, 150, 300, 600]);
        assert_eq!(walk_index_set(u64::MAX, u64::MAX, next), Vec::<u32>::new());
        // A set that never ends is cut at MAX_LISTED; one that goes backwards stops.
        assert_eq!(walk_index_set(1, u64::MAX, |i| i + 1).len(), MAX_LISTED);
        assert_eq!(walk_index_set(5, u64::MAX, |_| 3), vec![5]);
        // Values past u32 are skipped, not truncated.
        assert_eq!(walk_index_set(1 << 40, u64::MAX, |_| u64::MAX), Vec::<u32>::new());
    }
}
