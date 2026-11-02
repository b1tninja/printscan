//! The `ScanTicket` of a `CreateScanJob`, planned from [`ScanSettings`] against what the scanner
//! reported in its configuration.

use printscan::scan::{Color, PageSize, Region, Rotation, ScanError, ScanSettings, Source};

use super::model::{ColorMode, Configuration, Input};
use crate::xml::esc;

/// The formats this backend asks for when the request names none, best first: lossless before
/// lossy, single-image formats only (one `RetrieveImage` is one page).
const PREFERRED: &[&str] = &["png", "tiff-single-uncompressed", "dib", "exif", "jfif"];

/// Where the paper is (`InputSource`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputSource {
    Platen,
    Adf,
    AdfDuplex,
}

impl InputSource {
    pub fn name(self) -> &'static str {
        match self {
            Self::Platen => "Platen",
            Self::Adf => "ADF",
            Self::AdfDuplex => "ADFDuplex",
        }
    }
}

/// One scan job's ticket. Sizes and offsets are in thousandths of an inch.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Ticket {
    pub job_name: String,
    pub user_name: String,
    /// A `FormatValue` (`png`, `jfif`, ...).
    pub format: String,
    /// `ImagesToTransfer`: images, so both sides of a two-sided sheet count. `0` asks for every
    /// sheet in the feeder.
    pub images: u32,
    pub input: InputSource,
    pub color: ColorMode,
    pub dpi: (u32, u32),
    /// The sheet's size (`InputMediaSize`).
    pub media: (u32, u32),
    /// The window on it: `(x, y, width, height)`.
    pub region: (u32, u32, u32, u32),
    pub brightness: Option<i32>,
    pub contrast: Option<i32>,
    /// Degrees, when the scanner lists the value.
    pub rotation: Option<u32>,
}

impl Ticket {
    /// The ticket with another format.
    pub fn with_format(&self, format: &str) -> Self {
        Self { format: format.to_owned(), ..self.clone() }
    }

    fn media_side(&self, element: &str) -> String {
        let (x, y, w, h) = self.region;
        let (dx, dy) = self.dpi;
        format!(
            "<wscn:{element}><wscn:ScanRegion><wscn:ScanRegionXOffset>{x}</wscn:ScanRegionXOffset><wscn:ScanRegionYOffset>{y}</wscn:ScanRegionYOffset>\
             <wscn:ScanRegionWidth>{w}</wscn:ScanRegionWidth><wscn:ScanRegionHeight>{h}</wscn:ScanRegionHeight></wscn:ScanRegion>\
             <wscn:ColorProcessing>{}</wscn:ColorProcessing>\
             <wscn:Resolution><wscn:Width>{dx}</wscn:Width><wscn:Height>{dy}</wscn:Height></wscn:Resolution></wscn:{element}>",
            esc(self.color.name())
        )
    }

    /// The `ScanTicket` element, its `DocumentParameters` in schema order.
    pub fn xml(&self) -> String {
        let mut p = format!(
            "<wscn:Format>{}</wscn:Format><wscn:ImagesToTransfer>{}</wscn:ImagesToTransfer><wscn:InputSource>{}</wscn:InputSource>",
            esc(&self.format),
            self.images,
            self.input.name()
        );
        let (w, h) = self.media;
        p.push_str(&format!(
            "<wscn:InputSize><wscn:DocumentSizeAutoDetect>false</wscn:DocumentSizeAutoDetect>\
             <wscn:InputMediaSize><wscn:Width>{w}</wscn:Width><wscn:Height>{h}</wscn:Height></wscn:InputMediaSize></wscn:InputSize>"
        ));
        if self.brightness.is_some() || self.contrast.is_some() {
            p.push_str("<wscn:Exposure><wscn:AutoExposure>false</wscn:AutoExposure><wscn:ExposureSettings>");
            if let Some(c) = self.contrast {
                p.push_str(&format!("<wscn:Contrast>{c}</wscn:Contrast>"));
            }
            if let Some(b) = self.brightness {
                p.push_str(&format!("<wscn:Brightness>{b}</wscn:Brightness>"));
            }
            p.push_str("</wscn:ExposureSettings></wscn:Exposure>");
        }
        if let Some(r) = self.rotation {
            p.push_str(&format!("<wscn:Rotation>{r}</wscn:Rotation>"));
        }
        p.push_str("<wscn:MediaSides>");
        p.push_str(&self.media_side("MediaFront"));
        if self.input == InputSource::AdfDuplex {
            p.push_str(&self.media_side("MediaBack"));
        }
        p.push_str("</wscn:MediaSides>");
        format!(
            "<wscn:ScanTicket><wscn:JobDescription><wscn:JobName>{}</wscn:JobName><wscn:JobOriginatingUserName>{}</wscn:JobOriginatingUserName></wscn:JobDescription>\
             <wscn:DocumentParameters>{p}</wscn:DocumentParameters></wscn:ScanTicket>",
            esc(&cut(&self.job_name)),
            esc(&cut(&self.user_name))
        )
    }
}

/// At most 255 characters (`String255ExtType`).
fn cut(s: &str) -> String {
    s.chars().take(255).collect()
}

/// The wire format for a format name the scan core uses (`bmp`, `png`, `jpeg`, `tiff`), or the
/// name itself when it is already a `FormatValue`.
pub fn wire_format(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "bmp" | "dib" => "dib".into(),
        "jpeg" | "jpg" | "jfif" => "jfif".into(),
        "tiff" | "tif" => "tiff-single-uncompressed".into(),
        other => other.into(),
    }
}

/// The colour mode for a request.
pub fn color_mode(color: Color) -> ColorMode {
    match color {
        Color::Rgb => ColorMode::Rgb24,
        Color::Gray => ColorMode::Grayscale8,
        Color::BlackAndWhite => ColorMode::BlackAndWhite1,
        // A mode this backend does not know yet: colour, which every scanner offers.
        _ => ColorMode::Rgb24,
    }
}

/// The formats to fall back to after the scanner refused `refused`, in the order Microsoft's WSD
/// scan driver uses (PNG, EXIF for colour or grey, G4 for black and white, uncompressed TIFF),
/// without the ones already tried.
pub fn fallbacks(color: &ColorMode, tried: &[String]) -> Vec<&'static str> {
    let mut out = vec!["png"];
    if matches!(color, ColorMode::Rgb24 | ColorMode::Grayscale8) {
        out.push("exif");
    }
    if *color == ColorMode::BlackAndWhite1 {
        out.push("tiff-single-g4");
    }
    out.push("tiff-single-uncompressed");
    out.retain(|f| !tried.iter().any(|t| t == f));
    out
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

/// The resolution nearest `want` that `listed` has, preferring the next one up; `want` when the
/// scanner lists none.
fn snap(want: u32, listed: &[u32]) -> u32 {
    listed.iter().copied().find(|&d| d >= want).or_else(|| listed.last().copied()).unwrap_or(want)
}

fn rotation_degrees(rotation: Rotation) -> u32 {
    match rotation {
        Rotation::Portrait => 0,
        Rotation::Landscape => 90,
        Rotation::UpsideDown => 180,
        Rotation::Left => 270,
        _ => 0,
    }
}

/// A WS-Scan exposure value (-1000..=1000).
fn exposure(value: Option<i32>, supported: bool) -> Option<i32> {
    value.filter(|_| supported).map(|v| v.clamp(-1000, 1000))
}

/// Plan the ticket for `settings` on `source` (already resolved, never [`Source::Auto`]).
pub fn plan(settings: &ScanSettings, config: &Configuration, source: Source, job_name: &str, user_name: &str) -> Result<Ticket, ScanError> {
    let (input, side): (InputSource, &Input) = match source {
        Source::Glass => (InputSource::Platen, config.platen.as_ref().ok_or_else(|| ScanError::Failed("this scanner has no glass".into()))?),
        _ => {
            let feeder = config.feeder.as_ref().ok_or_else(|| ScanError::Failed("this scanner has no document feeder".into()))?;
            if settings.duplex && !feeder.duplex {
                return Err(ScanError::Failed("this scanner's feeder cannot scan both sides".into()));
            }
            (if settings.duplex { InputSource::AdfDuplex } else { InputSource::Adf }, &feeder.front)
        }
    };
    let color = color_mode(settings.color);
    if !side.colors.is_empty() && !side.colors.contains(&color) {
        return Err(ScanError::Failed(format!("the scanner does not offer {} on this source", color.name())));
    }
    let format = match &settings.format {
        Some(name) => {
            let wire = wire_format(name);
            if !config.formats.is_empty() && !config.formats.contains(&wire) {
                return Err(ScanError::Failed(format!("the scanner does not offer the {name} format")));
            }
            wire
        }
        None => PREFERRED.iter().find(|f| config.formats.iter().any(|c| c == *f)).copied().unwrap_or("png").to_owned(),
    };
    let dpi = (snap(settings.dpi, &side.widths), snap(settings.dpi_y.unwrap_or(settings.dpi), &side.heights));
    let (max_w, max_h) = side.maximum.unwrap_or((8500, 11000));
    let media = settings.page_size.and_then(page_size).map_or((max_w, max_h), |(w, h)| (w.min(max_w), h.min(max_h)));
    let region = match settings.region {
        Some(Region { left, top, width, height, .. }) => {
            if width == 0 || height == 0 || left.saturating_add(width) > max_w || top.saturating_add(height) > max_h {
                return Err(ScanError::Failed(format!("the region does not fit the scanner's {max_w} x {max_h} area (thousandths of an inch)")));
            }
            (left, top, width, height)
        }
        None => (0, 0, media.0, media.1),
    };
    let images = match source {
        Source::Glass => 1,
        _ => settings.pages.min(printscan::scan::MAX_PAGES).saturating_mul(if settings.duplex { 2 } else { 1 }),
    };
    let rotation = settings.rotation.map(rotation_degrees).filter(|&r| r != 0 && config.rotations.contains(&r));
    Ok(Ticket {
        job_name: job_name.to_owned(),
        user_name: user_name.to_owned(),
        format,
        images,
        input,
        color,
        dpi,
        media,
        region,
        brightness: exposure(settings.brightness, config.brightness),
        contrast: exposure(settings.contrast, config.contrast),
        rotation,
    })
}

#[cfg(test)]
mod tests {
    use super::super::model::Feeder;
    use super::*;

    fn config() -> Configuration {
        let platen = Input {
            widths: vec![100, 200, 300, 600],
            heights: vec![100, 200, 300, 600],
            colors: vec![ColorMode::Rgb24, ColorMode::Grayscale8, ColorMode::BlackAndWhite1],
            maximum: Some((8500, 11690)),
            ..Input::default()
        };
        let front = Input { maximum: Some((8500, 14000)), ..platen.clone() };
        Configuration {
            formats: vec!["jfif".into(), "dib".into(), "png".into()],
            brightness: true,
            platen: Some(platen),
            feeder: Some(Feeder { duplex: true, front, back: None }),
            ..Configuration::default()
        }
    }

    #[test]
    fn a_glass_scan_takes_one_lossless_image_of_the_whole_bed() {
        let s = ScanSettings::default().with_dpi(250).with_color(Color::Rgb).with_pages(5).with_brightness(5000).with_contrast(10);
        let t = plan(&s, &config(), Source::Glass, "job", "me").unwrap();
        assert_eq!((t.format.as_str(), t.images, t.input, t.dpi), ("png", 1, InputSource::Platen, (300, 300)));
        assert_eq!(t.region, (0, 0, 8500, 11690));
        assert_eq!((t.brightness, t.contrast), (Some(1000), None), "clamped, and contrast is not offered");
        let xml = t.xml();
        assert!(xml.contains("<wscn:InputSource>Platen</wscn:InputSource>"));
        assert!(xml.contains("<wscn:ColorProcessing>RGB24</wscn:ColorProcessing>"));
        assert!(!xml.contains("MediaBack"));
        // Schema order inside DocumentParameters.
        let at = |s: &str| xml.find(s).unwrap();
        assert!(at("<wscn:Format>") < at("<wscn:ImagesToTransfer>") && at("<wscn:InputSource>") < at("<wscn:InputSize>"));
        assert!(at("<wscn:InputSize>") < at("<wscn:Exposure>") && at("<wscn:Exposure>") < at("<wscn:MediaSides>"));
    }

    #[test]
    fn a_two_sided_feeder_scan_counts_images_and_has_a_back() {
        let s =
            ScanSettings::default().with_source(Source::Feeder).with_duplex(true).with_pages(3).with_page_size(PageSize::Legal).with_format("bmp");
        let t = plan(&s, &config(), Source::Feeder, "job", "me").unwrap();
        assert_eq!((t.format.as_str(), t.images, t.input, t.region), ("dib", 6, InputSource::AdfDuplex, (0, 0, 8500, 14000)));
        assert!(t.xml().contains("<wscn:MediaBack>"));
        let all = s.with_pages(0);
        assert_eq!(plan(&all, &config(), Source::Feeder, "j", "u").unwrap().images, 0);
    }

    #[test]
    fn what_the_scanner_does_not_offer_is_refused_before_anything_is_sent() {
        let mut c = config();
        assert!(plan(&ScanSettings::default().with_format("tiff"), &c, Source::Glass, "j", "u").is_err());
        let off = ScanSettings::default().with_region(Region::new(8000, 0, 1000, 1000));
        assert!(plan(&off, &c, Source::Glass, "j", "u").is_err());
        c.feeder = None;
        assert!(plan(&ScanSettings::default(), &c, Source::Feeder, "j", "u").is_err());
        c.platen.as_mut().unwrap().colors = vec![ColorMode::Rgb24];
        assert!(plan(&ScanSettings::default().with_color(Color::Gray), &c, Source::Glass, "j", "u").is_err());
    }

    #[test]
    fn fallbacks_follow_the_windows_driver() {
        assert_eq!(fallbacks(&ColorMode::Rgb24, &["jfif".into()]), ["png", "exif", "tiff-single-uncompressed"]);
        assert_eq!(fallbacks(&ColorMode::BlackAndWhite1, &["png".into()]), ["tiff-single-g4", "tiff-single-uncompressed"]);
    }

    #[test]
    fn names_are_escaped_and_cut() {
        let s = ScanSettings::default();
        let t = plan(&s, &config(), Source::Glass, &"<&>".repeat(100), "me").unwrap();
        let xml = t.xml();
        assert!(xml.contains("<wscn:JobName>&lt;&amp;&gt;"));
        assert_eq!(xml.matches("&lt;").count(), 85, "255 characters");
    }
}
