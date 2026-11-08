//! A [`ScanSettings`] as the properties written to one functional unit before `requestScan`.
//!
//! * **Source.** [`Source::Auto`] is the feeder when the scanner has one and its `documentLoaded`
//!   does not say it is empty, otherwise the glass ([`resolve_source`]).
//! * **Resolution** is honoured or refused: a dpi the unit does not list is an error that names the
//!   ones it does. ImageCaptureCore has one resolution for both axes, so a different `dpi_y` is refused.
//! * **Colour.** Colour and grey are 8-bit scans. Black and white is a 1-bit scan when the unit has
//!   1 bit, and otherwise an 8-bit grey scan that printscan thresholds ([`ScanPlan::software_threshold`]).
//! * **Area.** Sizes are written in inches when the unit takes inches (in its own unit otherwise). A
//!   region is clipped to the unit's area; with no region the whole area is scanned, unless a page
//!   size the unit lists was asked for, which is written as the document type instead. The origin
//!   is taken to be the top left corner of the area: **unverified on hardware**.
//! * **Format** is the file ImageCaptureCore writes: TIFF unless the request names `png`, `jpeg` or
//!   `bmp`. Any other name is refused.
//! * **Pages.** The glass is one page. The feeder scans what is loaded; a fixed count is enforced
//!   by cancelling once that many pages arrived. Duplex is written only when the feeder supports it.
//! * Brightness, contrast, rotation, compression, intents and the other WIA-only settings have no
//!   ImageCaptureCore property and are ignored, as hints may be.

use printscan::scan::{self, Area, Color, ScanError, ScanSettings, Source, bw};

use super::caps;
use super::model::{MeasurementUnit, PixelType, UnitInfo, UnitKind};

/// A rectangle in a functional unit's measurement unit.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct ScanArea {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ScanArea {
    /// The rectangle at (`x`, `y`) of `width` by `height`.
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }
}

/// What is written to the selected functional unit, in this order.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct UnitSetup {
    /// Set first, so that [`Self::scan_area`] is in this unit. `None` leaves the unit's own.
    pub measurement_unit: Option<MeasurementUnit>,
    pub resolution: u32,
    pub pixel_type: PixelType,
    pub bit_depth: u32,
    /// The document type (`ICScannerDocumentType`) of a flatbed or feeder.
    pub document_type: Option<u64>,
    pub scan_area: Option<ScanArea>,
    /// A feeder's `duplexScanningEnabled`.
    pub duplex: Option<bool>,
    /// `thresholdForBlackAndWhiteScanning`, for a 1-bit scan on a unit that accepts one.
    pub threshold: Option<u8>,
}

/// A planned scan.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ScanPlan {
    /// The source used (never [`Source::Auto`]).
    pub source: Source,
    pub unit: UnitKind,
    pub setup: UnitSetup,
    /// The `documentUTI` ImageCaptureCore writes the pages as.
    pub uti: &'static str,
    /// The page format that UTI is (`tiff`, `png`, `jpeg` or `bmp`).
    pub format: &'static str,
    /// The images to take before the scan is stopped; `0` takes everything the feeder holds.
    pub images: u32,
    /// The scan is grey and printscan makes black and white of it.
    pub software_threshold: bool,
}

/// The unit a source scans from.
pub fn unit_for(source: Source) -> UnitKind {
    match source {
        Source::Feeder => UnitKind::DocumentFeeder,
        _ => UnitKind::Flatbed,
    }
}

/// The source to scan from: `listed` are the unit types the scanner lists, `feeder_loaded` the
/// feeder's `documentLoaded` when it was read.
pub fn resolve_source(requested: Source, listed: &[UnitKind], feeder_loaded: Option<bool>) -> Result<Source, ScanError> {
    let has_feeder = listed.contains(&UnitKind::DocumentFeeder);
    let has_glass = listed.contains(&UnitKind::Flatbed);
    match requested {
        Source::Feeder if !has_feeder => Err(ScanError::Failed("the scanner has no document feeder".into())),
        Source::Glass if !has_glass => Err(ScanError::Failed("the scanner has no flatbed".into())),
        other => other.resolve(feeder_loaded, has_feeder, has_glass),
    }
}

/// The UTI and page format for a requested format name; TIFF when none is named.
pub fn uti_for(format: Option<&str>) -> Result<(&'static str, &'static str), ScanError> {
    match format.map(str::to_ascii_lowercase).as_deref() {
        None | Some("tiff" | "tif") => Ok(("public.tiff", "tiff")),
        Some("png") => Ok(("public.png", "png")),
        Some("jpeg" | "jpg") => Ok(("public.jpeg", "jpeg")),
        Some("bmp") => Ok(("com.microsoft.bmp", "bmp")),
        Some(other) => Err(ScanError::Failed(format!("ImageCaptureCore pages can be {}; not {other:?}", caps::FORMATS.join(", ")))),
    }
}

/// The resolution to write, or the error that names the ones the unit offers.
fn resolution(settings: &ScanSettings, unit: &UnitInfo) -> Result<u32, ScanError> {
    if let Some(dpi_y) = settings.dpi_y
        && dpi_y != settings.dpi
    {
        return Err(ScanError::Failed(format!(
            "ImageCaptureCore scans at one resolution on both axes; {} by {dpi_y} dpi was asked for",
            settings.dpi
        )));
    }
    let offered = unit.offered_resolutions();
    if !offered.is_empty() && !offered.contains(&settings.dpi) {
        let listed = offered.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
        return Err(ScanError::Failed(format!("the {} scans at {listed} dpi, not {}", unit.kind.label(), settings.dpi)));
    }
    Ok(settings.dpi)
}

/// The pixel type and bit depth for a colour, and whether printscan thresholds the page.
fn pixels(color: Color, unit: &UnitInfo) -> Result<(PixelType, u32, bool), ScanError> {
    let depths = &unit.bit_depths;
    let deep = depths.is_empty() || depths.contains(&8);
    match color {
        Color::BlackAndWhite if depths.is_empty() || depths.contains(&1) => Ok((PixelType::BlackAndWhite, 1, false)),
        Color::BlackAndWhite if deep => Ok((PixelType::Gray, 8, true)),
        Color::Rgb if deep => Ok((PixelType::Rgb, 8, false)),
        Color::Gray if deep => Ok((PixelType::Gray, 8, false)),
        Color::BlackAndWhite => Err(ScanError::Failed(format!("the {} scans neither 1-bit nor 8-bit images", unit.kind.label()))),
        _ => Err(ScanError::Failed(format!("the {} scans only black and white", unit.kind.label()))),
    }
}

/// The measurement unit to write sizes in: inches when the unit takes them (or lists nothing), else its own.
fn sizing_unit(unit: &UnitInfo) -> Option<MeasurementUnit> {
    if unit.measurement_units.is_empty() || unit.measurement_units.contains(&MeasurementUnit::Inches) {
        Some(MeasurementUnit::Inches)
    } else {
        unit.measurement_unit
    }
}

/// The area to write, in `sizing`, and the document type, from the region or page size asked for.
fn area(settings: &ScanSettings, unit: &UnitInfo, sizing: Option<MeasurementUnit>, dpi: u32) -> Result<(Option<ScanArea>, Option<u64>), ScanError> {
    let whole = unit.area_thousandths().map(Area::from);
    let to_area = |left: u32, top: u32, width: u32, height: u32| -> Option<ScanArea> {
        let measure = sizing?;
        Some(ScanArea::new(
            measure.from_thousandths(left, dpi)?,
            measure.from_thousandths(top, dpi)?,
            measure.from_thousandths(width, dpi)?,
            measure.from_thousandths(height, dpi)?,
        ))
    };
    if let Some(region) = settings.region {
        let fitted = scan::fit_region(region, whole)?;
        let rect = to_area(fitted.left, fitted.top, fitted.width, fitted.height).ok_or_else(|| {
            ScanError::Failed(format!("the {} does not say what unit its sizes are in, so no region can be set", unit.kind.label()))
        })?;
        return Ok((Some(rect), None));
    }
    let document = settings.page_size.and_then(caps::document_for).filter(|document| unit.document_types.contains(document));
    if document.is_some() {
        return Ok((None, document));
    }
    Ok((whole.and_then(|whole| to_area(0, 0, whole.width, whole.height)), None))
}

/// Plan a scan of `settings` on `unit` (the unit for `source`, as a session read it).
pub fn plan(settings: &ScanSettings, unit: &UnitInfo, source: Source) -> Result<ScanPlan, ScanError> {
    let dpi = resolution(settings, unit)?;
    let (pixel_type, bit_depth, software_threshold) = pixels(settings.color, unit)?;
    let (uti, format) = uti_for(settings.format.as_deref())?;
    let sizing = sizing_unit(unit);
    let (scan_area, document_type) = area(settings, unit, sizing, dpi)?;
    let feeder = source == Source::Feeder;
    let duplex = (feeder && unit.supports_duplex == Some(true)).then_some(settings.duplex);
    let threshold = (pixel_type == PixelType::BlackAndWhite && unit.accepts_threshold && settings.threshold.is_some())
        .then(|| bw::threshold_byte(settings.threshold));
    let sides = if duplex == Some(true) { 2 } else { 1 };
    let images = if feeder { settings.pages.min(scan::MAX_PAGES) * sides } else { 1 };
    Ok(ScanPlan {
        source,
        unit: unit.kind,
        setup: UnitSetup {
            measurement_unit: sizing.filter(|_| scan_area.is_some()),
            resolution: dpi,
            pixel_type,
            bit_depth,
            document_type,
            scan_area,
            duplex,
            threshold,
        },
        uti,
        format,
        images,
        software_threshold,
    })
}

#[cfg(test)]
mod tests {
    use printscan::scan::{PageSize, Region};

    use super::*;
    use crate::ica::model::document_type;

    fn flatbed() -> UnitInfo {
        UnitInfo::new(UnitKind::Flatbed)
            .with_resolutions(vec![150, 300, 600])
            .with_bit_depths(vec![8])
            .with_measurement_units(vec![MeasurementUnit::Inches, MeasurementUnit::Centimeters])
            .with_measurement_unit(MeasurementUnit::Centimeters)
            .with_physical_size((21.59, 29.7))
            .with_document_types(vec![document_type::A4])
    }

    fn feeder() -> UnitInfo {
        UnitInfo::new(UnitKind::DocumentFeeder).with_bit_depths(vec![1, 8]).with_supports_duplex(true).with_accepts_threshold(true)
    }

    #[test]
    fn sources_resolve_from_what_is_listed() {
        let both = [UnitKind::Flatbed, UnitKind::DocumentFeeder];
        assert_eq!(resolve_source(Source::Auto, &both, None), Ok(Source::Feeder));
        assert_eq!(resolve_source(Source::Auto, &both, Some(false)), Ok(Source::Glass));
        assert_eq!(resolve_source(Source::Auto, &[UnitKind::DocumentFeeder], Some(false)), Ok(Source::Feeder));
        assert!(resolve_source(Source::Feeder, &[UnitKind::Flatbed], None).is_err());
        assert!(resolve_source(Source::Glass, &[UnitKind::DocumentFeeder], None).is_err());
        assert!(resolve_source(Source::Auto, &[UnitKind::PositiveTransparency], None).is_err());
    }

    #[test]
    fn resolution_is_honoured_or_refused() {
        let settings = ScanSettings::default().with_dpi(300);
        assert_eq!(plan(&settings, &flatbed(), Source::Glass).map(|p| p.setup.resolution), Ok(300));
        let refused = plan(&settings.clone().with_dpi(200), &flatbed(), Source::Glass);
        assert!(refused.is_err_and(|e| e.to_string().contains("150, 300, 600")));
        assert!(plan(&settings.clone().with_dpi_y(600), &flatbed(), Source::Glass).is_err());
        assert!(plan(&settings.with_dpi(200), &UnitInfo::new(UnitKind::Flatbed), Source::Glass).is_ok());
    }

    #[test]
    fn colours_become_pixel_types() {
        let gray = plan(&ScanSettings::default().with_dpi(300), &flatbed(), Source::Glass).map(|p| (p.setup.pixel_type, p.setup.bit_depth));
        assert_eq!(gray, Ok((PixelType::Gray, 8)));
        let bw = ScanSettings::default().with_dpi(300).with_color(Color::BlackAndWhite).with_threshold(90);
        let soft = plan(&bw, &flatbed(), Source::Glass).map(|p| (p.setup.pixel_type, p.software_threshold, p.setup.threshold));
        assert_eq!(soft, Ok((PixelType::Gray, true, None)));
        let native = plan(&bw, &feeder(), Source::Feeder).map(|p| (p.setup.pixel_type, p.setup.bit_depth, p.software_threshold, p.setup.threshold));
        assert_eq!(native, Ok((PixelType::BlackAndWhite, 1, false, Some(90))));
        let only_bw = UnitInfo::new(UnitKind::Flatbed).with_bit_depths(vec![1]);
        assert!(plan(&ScanSettings::default().with_color(Color::Rgb), &only_bw, Source::Glass).is_err());
    }

    #[test]
    fn areas_are_written_in_inches() {
        let whole = plan(&ScanSettings::default().with_dpi(300), &flatbed(), Source::Glass).map(|p| (p.setup.measurement_unit, p.setup.scan_area));
        assert_eq!(whole, Ok((Some(MeasurementUnit::Inches), Some(ScanArea::new(0.0, 0.0, 8.5, 11.693)))));
        let region = ScanSettings::default().with_dpi(300).with_region(Region::new(1000, 2000, 4000, 20000));
        let clipped = plan(&region, &flatbed(), Source::Glass).map(|p| p.setup.scan_area);
        assert_eq!(clipped, Ok(Some(ScanArea::new(1.0, 2.0, 4.0, 9.693))));
        let outside = ScanSettings::default().with_dpi(300).with_region(Region::new(9000, 0, 100, 100));
        assert!(plan(&outside, &flatbed(), Source::Glass).is_err());
        let a4 = ScanSettings::default().with_dpi(300).with_page_size(PageSize::A4);
        assert_eq!(plan(&a4, &flatbed(), Source::Glass).map(|p| (p.setup.document_type, p.setup.scan_area)), Ok((Some(document_type::A4), None)));
        // A unit with no sizes gets no area, and a region on it is refused.
        let bare = UnitInfo::new(UnitKind::Flatbed).with_measurement_units(vec![MeasurementUnit::Pixels]);
        assert_eq!(plan(&ScanSettings::default(), &bare, Source::Glass).map(|p| p.setup.scan_area), Ok(None));
        let on_bare = ScanSettings::default().with_region(Region::new(0, 0, 1000, 1000));
        assert!(plan(&on_bare, &bare, Source::Glass).is_err());
    }

    #[test]
    fn formats_pages_and_duplex() {
        assert_eq!(uti_for(None), Ok(("public.tiff", "tiff")));
        assert_eq!(uti_for(Some("PNG")), Ok(("public.png", "png")));
        assert!(uti_for(Some("pdf")).is_err());
        let glass = plan(&ScanSettings::default().with_dpi(300).with_pages(5), &flatbed(), Source::Glass);
        assert_eq!(glass.map(|p| (p.images, p.setup.duplex)), Ok((1, None)));
        let simplex = plan(&ScanSettings::default().with_pages(3), &feeder(), Source::Feeder);
        assert_eq!(simplex.map(|p| (p.images, p.setup.duplex)), Ok((3, Some(false))));
        let duplex = plan(&ScanSettings::default().with_pages(3).with_duplex(true), &feeder(), Source::Feeder);
        assert_eq!(duplex.map(|p| (p.images, p.setup.duplex)), Ok((6, Some(true))));
        let all = plan(&ScanSettings::default().with_pages(0), &feeder(), Source::Feeder);
        assert_eq!(all.map(|p| p.images), Ok(0));
    }
}
