//! The `ScanSettings` document of a `POST ScanJobs`, planned from the scan core's
//! [`printscan::scan::ScanSettings`] against what the scanner published.

use printscan::scan::{self, Color, Intent, PageSize, ScanError, ScanSettings, Source, ValueSpan};
use printscan_wsd::xml::esc;

use crate::model::{Capabilities, ColorMode, InputCaps, PWG_NS, Range, SCAN_NS};

/// The formats asked for when the request names none, best first: lossless single images, then
/// JPEG, then PDF.
const PREFERRED: &[&str] = &["image/png", "image/tiff", "image/jpeg", "application/pdf"];

/// Where the paper is (`pwg:InputSource`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum InputSource {
    Platen,
    Feeder,
}

impl InputSource {
    /// The value on the wire.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Platen => "Platen",
            Self::Feeder => "Feeder",
        }
    }
}

/// One scan job's settings. Lengths are in three hundredths of an inch.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScanRequest {
    /// `pwg:Version` to write: the scanner's own, else `2.0`.
    pub version: String,
    pub input: InputSource,
    pub duplex: bool,
    pub color: ColorMode,
    pub resolution: (u32, u32),
    /// A MIME type.
    pub format: String,
    /// `(x, y, width, height)`.
    pub region: (u32, u32, u32, u32),
    pub intent: Option<&'static str>,
    pub brightness: Option<i32>,
    pub contrast: Option<i32>,
    pub threshold: Option<i32>,
}

impl ScanRequest {
    /// The `scan:ScanSettings` document.
    pub fn xml(&self) -> String {
        let (x, y, w, h) = self.region;
        let mut s = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<scan:ScanSettings xmlns:scan=\"{SCAN_NS}\" xmlns:pwg=\"{PWG_NS}\">\
             <pwg:Version>{}</pwg:Version>",
            esc(&self.version)
        );
        if let Some(intent) = self.intent {
            s.push_str(&format!("<scan:Intent>{intent}</scan:Intent>"));
        }
        s.push_str(&format!(
            "<pwg:ScanRegions><pwg:ScanRegion><pwg:Height>{h}</pwg:Height><pwg:ContentRegionUnits>escl:ThreeHundredthsOfInches</pwg:ContentRegionUnits>\
             <pwg:Width>{w}</pwg:Width><pwg:XOffset>{x}</pwg:XOffset><pwg:YOffset>{y}</pwg:YOffset></pwg:ScanRegion></pwg:ScanRegions>"
        ));
        let format = esc(&self.format);
        s.push_str(&format!("<pwg:DocumentFormat>{format}</pwg:DocumentFormat><scan:DocumentFormatExt>{format}</scan:DocumentFormatExt>"));
        s.push_str(&format!("<pwg:InputSource>{}</pwg:InputSource>", self.input.name()));
        let (rx, ry) = self.resolution;
        s.push_str(&format!("<scan:XResolution>{rx}</scan:XResolution><scan:YResolution>{ry}</scan:YResolution>"));
        s.push_str(&format!("<scan:ColorMode>{}</scan:ColorMode>", esc(self.color.name())));
        if self.input == InputSource::Feeder {
            s.push_str(&format!("<scan:Duplex>{}</scan:Duplex>", self.duplex));
        }
        for (name, value) in [("Brightness", self.brightness), ("Contrast", self.contrast), ("Threshold", self.threshold)] {
            if let Some(v) = value {
                s.push_str(&format!("<scan:{name}>{v}</scan:{name}>"));
            }
        }
        s.push_str("</scan:ScanSettings>");
        s
    }
}

/// The MIME type for a format name the scan core uses (`png`, `jpeg`, `tiff`, `pdf`), or the
/// name itself, lowercased, when it already is one.
pub fn mime_for(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "png" => "image/png".into(),
        "jpeg" | "jpg" | "jfif" => "image/jpeg".into(),
        "tiff" | "tif" => "image/tiff".into(),
        "pdf" => "application/pdf".into(),
        "bmp" => "image/bmp".into(),
        other => other.into(),
    }
}

/// The scan core's name for a MIME type (`image/png` is `png`), or the MIME type itself.
pub fn format_name(mime: &str) -> String {
    match mime {
        "image/png" => "png".into(),
        "image/jpeg" => "jpeg".into(),
        "image/tiff" => "tiff".into(),
        "application/pdf" => "pdf".into(),
        "image/bmp" => "bmp".into(),
        other => other.into(),
    }
}

/// The eSCL intent for a scan core intent, when there is a close one.
pub fn intent_for(intent: Intent) -> Option<&'static str> {
    match intent {
        Intent::Text => Some("Document"),
        Intent::Color => Some("Photo"),
        Intent::BestPreview => Some("Preview"),
        _ => None,
    }
}

/// The scan core intent for an eSCL one.
pub fn intent_from(name: &str) -> Option<Intent> {
    match name {
        "Document" => Some(Intent::Text),
        "Photo" => Some(Intent::Color),
        "Preview" => Some(Intent::BestPreview),
        _ => None,
    }
}

/// Thousandths of an inch as three hundredths, rounded.
pub fn to_300ths(mils: u32) -> u32 {
    u32::try_from((u64::from(mils) * 3 + 5) / 10).unwrap_or(u32::MAX)
}

/// Three hundredths of an inch as thousandths, rounded.
pub fn to_mils(v: u32) -> u32 {
    u32::try_from((u64::from(v) * 10 + 1) / 3).unwrap_or(u32::MAX)
}

/// The resolutions to offer for one side: the discrete list, or the common values a range admits
/// plus its ends.
pub fn listed_resolutions(caps: &InputCaps) -> Vec<u32> {
    let mut out: Vec<u32> = caps.resolutions.iter().filter(|(x, y)| x == y).map(|(x, _)| *x).collect();
    if let Some(r) = caps.x_range {
        for d in [75, 100, 150, 200, 240, 300, 400, 600, 1200, 2400] {
            if (r.min..=r.max).contains(&d) && (d - r.min) % r.step == 0 {
                out.push(u32::try_from(d).unwrap_or(0));
            }
        }
        out.extend([r.min, r.max].iter().filter_map(|v| u32::try_from(*v).ok()));
    }
    out.retain(|d| *d > 0);
    out.sort_unstable();
    out.dedup();
    out
}

/// The resolution for `want`: the nearest discrete one at or above it (else the highest), or `want`
/// aligned into the range; `want` when the scanner publishes neither.
fn resolve_dpi(want: u32, discrete: &[u32], range: Option<Range>) -> Result<u32, ScanError> {
    if !discrete.is_empty() && range.is_none() {
        return Ok(discrete.iter().copied().find(|&d| d >= want).or_else(|| discrete.last().copied()).unwrap_or(want));
    }
    match range {
        Some(r) => {
            let value = scan::align_to_span(i32::try_from(want).unwrap_or(i32::MAX), ValueSpan::new(r.min, r.max, r.step), true)?;
            Ok(u32::try_from(value).unwrap_or(want))
        }
        None => Ok(want),
    }
}

/// A requested adjustment in the scanner's range, or nothing when it publishes none.
fn adjustment(value: Option<i32>, range: Option<Range>) -> Result<Option<i32>, ScanError> {
    match (value, range) {
        (Some(v), Some(r)) => scan::align_to_span(v, ValueSpan::new(r.min, r.max, r.step), false).map(Some),
        _ => Ok(None),
    }
}

/// The full size of a preset page, in thousandths of an inch.
fn page_size(size: PageSize) -> Option<(u32, u32)> {
    match size {
        PageSize::A4 => Some((8268, 11693)),
        PageSize::Letter => Some((8500, 11000)),
        PageSize::Legal => Some((8500, 14000)),
        PageSize::BusinessCard => Some((3500, 2000)),
        _ => None,
    }
}

/// The colour mode to ask for. Black and white falls back to grey (thresholded afterwards) when the
/// scanner does not list `BlackAndWhite1`.
fn color_mode(color: Color, listed: &[ColorMode]) -> Result<ColorMode, ScanError> {
    let want = match color {
        Color::Rgb => ColorMode::Rgb24,
        Color::BlackAndWhite => ColorMode::BlackAndWhite1,
        _ => ColorMode::Grayscale8,
    };
    if listed.is_empty() || listed.contains(&want) {
        return Ok(want);
    }
    if want == ColorMode::BlackAndWhite1 && listed.contains(&ColorMode::Grayscale8) {
        return Ok(ColorMode::Grayscale8);
    }
    Err(ScanError::Failed(format!("the scanner does not offer {} on this source", want.name())))
}

/// Plan the request for `settings` on `source` (already resolved, never [`Source::Auto`]).
pub fn plan(settings: &ScanSettings, caps: &Capabilities, source: Source) -> Result<ScanRequest, ScanError> {
    settings.checked()?;
    let (input, side) = match source {
        Source::Glass => (InputSource::Platen, caps.platen.as_ref().ok_or_else(|| ScanError::Failed("this scanner has no glass".into()))?),
        _ => {
            if settings.duplex && !caps.duplex() {
                return Err(ScanError::Failed("this scanner's feeder cannot scan both sides".into()));
            }
            (InputSource::Feeder, caps.feeder(settings.duplex).ok_or_else(|| ScanError::Failed("this scanner has no document feeder".into()))?)
        }
    };
    let color = color_mode(settings.color, &side.color_modes)?;
    let format = match &settings.format {
        Some(name) => {
            let mime = mime_for(name);
            if !side.formats.is_empty() && !side.formats.contains(&mime) {
                return Err(ScanError::Failed(format!("the scanner does not offer the {name} format on this source")));
            }
            mime
        }
        None => PREFERRED.iter().find(|f| side.formats.iter().any(|g| g == *f)).copied().unwrap_or("image/jpeg").to_owned(),
    };
    let discrete_x: Vec<u32> = side.resolutions.iter().map(|r| r.0).collect();
    let discrete_y: Vec<u32> = side.resolutions.iter().map(|r| r.1).collect();
    let resolution = (
        resolve_dpi(settings.dpi, &discrete_x, side.x_range)?,
        resolve_dpi(settings.dpi_y.unwrap_or(settings.dpi), &discrete_y, side.y_range.or(side.x_range))?,
    );
    let (max_w, max_h) = side.max_area().unwrap_or((to_300ths(8500), to_300ths(11000)));
    let region = match settings.region {
        Some(r) => {
            let (x, y, w, h) = (to_300ths(r.left), to_300ths(r.top), to_300ths(r.width), to_300ths(r.height));
            if w == 0 || h == 0 || x.saturating_add(w) > max_w || y.saturating_add(h) > max_h {
                return Err(ScanError::Failed(format!(
                    "the region does not fit the scanner's {} x {} area (thousandths of an inch)",
                    to_mils(max_w),
                    to_mils(max_h)
                )));
            }
            (x, y, w, h)
        }
        None => match settings.page_size.and_then(page_size) {
            Some((w, h)) => (0, 0, to_300ths(w).min(max_w), to_300ths(h).min(max_h)),
            None => (0, 0, max_w, max_h),
        },
    };
    if side.min_width.is_some_and(|m| region.2 < m) || side.min_height.is_some_and(|m| region.3 < m) {
        return Err(ScanError::Failed("the region is smaller than the scanner's smallest scan area".into()));
    }
    let intent = settings.intent.and_then(intent_for).filter(|i| side.intents.iter().any(|s| s == i));
    Ok(ScanRequest {
        version: caps.version.clone().filter(|v| v.len() <= 8 && v.bytes().all(|b| b.is_ascii_digit() || b == b'.')).unwrap_or_else(|| "2.0".into()),
        input,
        duplex: input == InputSource::Feeder && settings.duplex,
        color,
        resolution,
        format,
        region,
        intent,
        brightness: adjustment(settings.brightness, caps.brightness)?,
        contrast: adjustment(settings.contrast, caps.contrast)?,
        threshold: adjustment(settings.threshold, caps.threshold)?,
    })
}

#[cfg(test)]
mod tests {
    use printscan::scan::Region;

    use super::*;

    fn caps() -> Capabilities {
        let platen = InputCaps {
            max_width: Some(2550),
            max_height: Some(3300),
            min_width: Some(16),
            min_height: Some(16),
            color_modes: vec![ColorMode::Rgb24, ColorMode::Grayscale8],
            formats: vec!["image/jpeg".into(), "image/png".into(), "application/pdf".into()],
            resolutions: vec![(100, 100), (200, 200), (300, 300), (600, 600)],
            intents: vec!["Document".into()],
            ..InputCaps::default()
        };
        let adf = InputCaps {
            max_height: Some(4200),
            resolutions: Vec::new(),
            x_range: Some(Range::new(75, 600, 25)),
            formats: vec!["image/jpeg".into()],
            ..platen.clone()
        };
        Capabilities {
            version: Some("2.6".into()),
            platen: Some(platen),
            adf_simplex: Some(adf.clone()),
            adf_duplex: Some(adf),
            brightness: Some(Range::new(0, 100, 1)),
            ..Capabilities::default()
        }
    }

    #[test]
    fn a_glass_scan_takes_the_whole_bed_in_a_lossless_format() {
        let s = ScanSettings::default().with_dpi(250).with_color(Color::Rgb).with_brightness(500).with_contrast(3).with_intent(Intent::Text);
        let r = plan(&s, &caps(), Source::Glass).unwrap();
        assert_eq!((r.input, r.format.as_str(), r.resolution, r.region), (InputSource::Platen, "image/png", (300, 300), (0, 0, 2550, 3300)));
        assert_eq!((r.brightness, r.contrast, r.intent), (Some(100), None, Some("Document")), "clamped; contrast is not published");
        let xml = r.xml();
        for part in [
            "<pwg:Version>2.6</pwg:Version>",
            "<pwg:InputSource>Platen</pwg:InputSource>",
            "<scan:ColorMode>RGB24</scan:ColorMode>",
            "<pwg:Width>2550</pwg:Width>",
            "<scan:XResolution>300</scan:XResolution>",
            "<pwg:DocumentFormat>image/png</pwg:DocumentFormat>",
            "escl:ThreeHundredthsOfInches",
        ] {
            assert!(xml.contains(part), "{part} in {xml}");
        }
        assert!(!xml.contains("Duplex"));
        assert!(roxmltree::Document::parse(&xml).is_ok());
    }

    #[test]
    fn a_feeder_scan_aligns_into_the_range_and_says_duplex() {
        let s = ScanSettings::default().with_dpi(310).with_duplex(true).with_page_size(PageSize::Legal).with_color(Color::BlackAndWhite);
        let r = plan(&s, &caps(), Source::Feeder).unwrap();
        assert_eq!((r.input, r.duplex, r.resolution, r.format.as_str()), (InputSource::Feeder, true, (300, 300), "image/jpeg"));
        assert_eq!(r.region, (0, 0, 2550, 4200));
        assert_eq!(r.color, ColorMode::Grayscale8, "black and white is thresholded from grey");
        assert!(r.xml().contains("<scan:Duplex>true</scan:Duplex>"));
    }

    #[test]
    fn regions_convert_to_three_hundredths() {
        let s = ScanSettings::default().with_region(Region::new(1000, 2000, 4000, 5000));
        assert_eq!(plan(&s, &caps(), Source::Glass).unwrap().region, (300, 600, 1200, 1500));
        assert_eq!((to_300ths(8500), to_mils(2550)), (2550, 8500));
    }

    #[test]
    fn what_the_scanner_does_not_offer_is_refused() {
        let c = caps();
        assert!(plan(&ScanSettings::default().with_format("tiff"), &c, Source::Glass).is_err());
        assert!(plan(&ScanSettings::default().with_region(Region::new(8000, 0, 1000, 1000)), &c, Source::Glass).is_err());
        assert!(plan(&ScanSettings::default().with_region(Region::new(0, 0, 10, 10)), &c, Source::Glass).is_err());
        let mut no_adf = c.clone();
        no_adf.adf_simplex = None;
        no_adf.adf_duplex = None;
        assert!(plan(&ScanSettings::default(), &no_adf, Source::Feeder).is_err());
        let mut rgb_only = c;
        rgb_only.platen.as_mut().unwrap().color_modes = vec![ColorMode::Rgb24];
        assert!(plan(&ScanSettings::default().with_color(Color::Gray), &rgb_only, Source::Glass).is_err());
        assert_eq!(plan(&ScanSettings::default().with_format("pdf"), &caps(), Source::Glass).unwrap().format, "application/pdf");
    }

    #[test]
    fn resolutions_offered_from_a_range() {
        let adf = caps().adf_simplex.unwrap();
        assert_eq!(listed_resolutions(&adf), [75, 100, 150, 200, 300, 400, 600]);
        assert_eq!(listed_resolutions(&caps().platen.unwrap()), [100, 200, 300, 600]);
    }
}
