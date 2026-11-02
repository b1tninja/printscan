//! `Serialize` for the scanner description and the per-page and per-error records (feature `serde`), in the shapes
//! the apps' CLI and bindings have always printed, so they need no JSON code of their own.
//!
//! * [`Scanner`]: its fields by their Rust names, with `handling_status` as the nine flags
//!   ([`HandlingStatus::flags`]), `conditions` ([`HandlingStatus::conditions`]), `bed` and `sheet` as
//!   `{width, height}`, `pages` and every range as `{min, max, step}`, `glass` and `feeder_choices` as
//!   [`ImageChoices`], and `page_length` as `{glass, feeder}` [`PageLengthSupport`]s. `reported` is not written:
//!   what it says is already in the `null`s and the `page_length` presence.
//! * [`LengthRecord`] adds `length_inches` (3 decimals) and `length_mm` (1 decimal) to its fields.
//! * [`ScanErrorInfo`] adds `hresult` (`"0x80210006"`) and `code_name` to its fields.
//!
//! Enums are their wire strings. Unknown values are `null`. The two records still read back with `Deserialize`
//! (the added fields are ignored); the scanner description is written only.

use serde::Serialize;
use serde::ser::{SerializeMap, SerializeStruct, Serializer};

use super::{HandlingStatus, ImageChoices, LengthRecord, PageLengthSupport, Presence, Rotation, ScanErrorInfo, Scanner, Source, ValueSpan};

impl Serialize for ValueSpan {
    /// `{min, max, step}`.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("ValueSpan", 3)?;
        out.serialize_field("min", &self.min)?;
        out.serialize_field("max", &self.max)?;
        out.serialize_field("step", &self.step)?;
        out.end()
    }
}

impl Serialize for Presence {
    /// `unknown`, `absent` or `present`.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl Serialize for Rotation {
    /// `portrait`, `landscape`, `180` or `270`.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl Serialize for HandlingStatus {
    /// The nine flags by name, each `true` or `false` (`feeder_ready` is `true` only when the device said so).
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let flags = self.flags();
        let mut out = serializer.serialize_map(Some(flags.len()))?;
        for (name, on) in flags {
            out.serialize_entry(name, &on)?;
        }
        out.end()
    }
}

impl Serialize for PageLengthSupport {
    /// `{device_detect, max_length_mils}`.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("PageLengthSupport", 2)?;
        out.serialize_field("device_detect", &self.device_detect)?;
        out.serialize_field("max_length_mils", &self.max_length_mils)?;
        out.end()
    }
}

impl Serialize for ImageChoices {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("ImageChoices", 17)?;
        out.serialize_field("dpi", &self.dpi)?;
        out.serialize_field("dpi_y", &self.dpi_y)?;
        out.serialize_field("colors", &self.colors)?;
        out.serialize_field("brightness", &self.brightness)?;
        out.serialize_field("contrast", &self.contrast)?;
        out.serialize_field("threshold", &self.threshold)?;
        out.serialize_field("rotation", &self.rotation)?;
        out.serialize_field("formats", &self.formats)?;
        out.serialize_field("compression", &self.compression)?;
        out.serialize_field("intents", &self.intents)?;
        out.serialize_field("page_sizes", &self.page_sizes)?;
        out.serialize_field("auto_deskew", &self.auto_deskew)?;
        out.serialize_field("orientation", &self.orientation)?;
        out.serialize_field("photometric", &self.photometric)?;
        out.serialize_field("invert", &self.invert)?;
        out.serialize_field("mirror", &self.mirror)?;
        out.serialize_field("lamp", &self.lamp)?;
        out.end()
    }
}

/// A width and height in thousandths of an inch as `{width, height}`.
struct Area((u32, u32));

impl Serialize for Area {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("Area", 2)?;
        out.serialize_field("width", &self.0.0)?;
        out.serialize_field("height", &self.0.1)?;
        out.end()
    }
}

/// What glass and feeder publish about page length.
struct LengthSupport<'a>(&'a Scanner);

impl Serialize for LengthSupport<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("PageLength", 2)?;
        out.serialize_field("glass", &self.0.length_support(Source::Glass))?;
        out.serialize_field("feeder", &self.0.length_support(Source::Feeder))?;
        out.end()
    }
}

impl Serialize for Scanner {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("Scanner", 20)?;
        out.serialize_field("id", &self.id)?;
        out.serialize_field("name", &self.name)?;
        out.serialize_field("feeder", &self.feeder)?;
        out.serialize_field("flatbed", &self.flatbed)?;
        out.serialize_field("duplex", &self.duplex)?;
        out.serialize_field("feeder_ready", &self.feeder_ready)?;
        out.serialize_field("handling_status", &self.handling_status)?;
        out.serialize_field("conditions", &self.handling_status.conditions())?;
        out.serialize_field("feeder_capacity", &self.feeder_capacity)?;
        out.serialize_field("connection", &self.connection)?;
        out.serialize_field("port", &self.port)?;
        out.serialize_field("optical_x", &self.optical_x)?;
        out.serialize_field("optical_y", &self.optical_y)?;
        out.serialize_field("pages", &self.pages)?;
        out.serialize_field("bed", &self.bed.map(Area))?;
        out.serialize_field("sheet", &self.sheet.map(Area))?;
        out.serialize_field("glass", &self.glass)?;
        out.serialize_field("feeder_choices", &self.feeder_choices)?;
        out.serialize_field("page_length", &LengthSupport(self))?;
        out.end()
    }
}

/// `value` rounded to `places` decimals.
fn rounded(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

impl Serialize for LengthRecord {
    /// `{source, length_mils, length_inches, length_mm, approximate}`.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("LengthRecord", 5)?;
        out.serialize_field("source", &self.source)?;
        out.serialize_field("length_mils", &self.length_mils)?;
        out.serialize_field("length_inches", &self.length_inches().map(|inches| rounded(inches, 3)))?;
        out.serialize_field("length_mm", &self.length_mm().map(|mm| rounded(mm, 1)))?;
        out.serialize_field("approximate", &self.approximate)?;
        out.end()
    }
}

impl Serialize for ScanErrorInfo {
    /// `{native, kind, phase, hresult, code_name, retryable, resumable, message}`.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("ScanErrorInfo", 8)?;
        out.serialize_field("native", &self.native)?;
        out.serialize_field("kind", &self.kind)?;
        out.serialize_field("phase", &self.phase)?;
        out.serialize_field("hresult", &self.hresult_hex())?;
        out.serialize_field("code_name", &self.code_name())?;
        out.serialize_field("retryable", &self.retryable)?;
        out.serialize_field("resumable", &self.resumable)?;
        out.serialize_field("message", &self.message)?;
        out.end()
    }
}
