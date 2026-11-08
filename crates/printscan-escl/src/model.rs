//! The `ScannerCapabilities` and `ScannerStatus` documents, as far as the backend uses them.
//!
//! Lengths are in the protocol's unit, three hundredths of an inch. Unknown enumeration values are
//! kept as `Other(text)`; lists are capped, so a hostile document cannot grow them without bound.

use roxmltree::Node;

use crate::error::{Error, Result};
use crate::xml::{self, child, child_i32, child_text, child_u32, children};

/// The eSCL namespace.
pub const SCAN_NS: &str = "http://schemas.hp.com/imaging/escl/2011/05/03";
/// The PWG semantic model namespace eSCL documents use beside it.
pub const PWG_NS: &str = "http://www.pwg.org/schemas/2010/12/sm";

/// The most entries read from any one list.
const MAX_LIST: usize = 64;

/// A colour mode (`scan:ColorMode`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ColorMode {
    BlackAndWhite1,
    Grayscale8,
    Grayscale16,
    Rgb24,
    Rgb48,
    Other(String),
}

impl ColorMode {
    /// Read a value.
    pub fn parse(text: &str) -> Self {
        match text {
            "BlackAndWhite1" => Self::BlackAndWhite1,
            "Grayscale8" => Self::Grayscale8,
            "Grayscale16" => Self::Grayscale16,
            "RGB24" => Self::Rgb24,
            "RGB48" => Self::Rgb48,
            other => Self::Other(other.to_owned()),
        }
    }

    /// The value on the wire.
    pub fn name(&self) -> &str {
        match self {
            Self::BlackAndWhite1 => "BlackAndWhite1",
            Self::Grayscale8 => "Grayscale8",
            Self::Grayscale16 => "Grayscale16",
            Self::Rgb24 => "RGB24",
            Self::Rgb48 => "RGB48",
            Self::Other(text) => text,
        }
    }
}

/// A numeric range a scanner publishes (resolution ranges, `BrightnessSupport`, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Range {
    pub min: i32,
    pub max: i32,
    pub normal: Option<i32>,
    pub step: i32,
}

impl Range {
    /// `min` to `max` in steps of `step` (1 when not positive).
    pub fn new(min: i32, max: i32, step: i32) -> Self {
        Self { min, max, normal: None, step: step.max(1) }
    }

    /// `Min`, `Max`, `Normal` and `Step` children of `node`; `None` without a valid min and max.
    fn read(node: Node<'_, '_>) -> Option<Self> {
        let min = child_i32(node, SCAN_NS, "Min")?;
        let max = child_i32(node, SCAN_NS, "Max")?;
        if max < min {
            return None;
        }
        let step = child_i32(node, SCAN_NS, "Step").filter(|s| *s > 0).unwrap_or(1);
        Some(Self { min, max, normal: child_i32(node, SCAN_NS, "Normal"), step })
    }
}

/// What one input (the platen, or the feeder one-sided or two-sided) offers. The setting profiles a
/// scanner lists are merged: the MVP does not keep which combinations go together.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct InputCaps {
    pub min_width: Option<u32>,
    pub max_width: Option<u32>,
    pub min_height: Option<u32>,
    pub max_height: Option<u32>,
    pub max_optical_x: Option<u32>,
    pub max_optical_y: Option<u32>,
    pub color_modes: Vec<ColorMode>,
    /// MIME types (`image/jpeg`, `application/pdf`), from `pwg:DocumentFormat` and `scan:DocumentFormatExt`.
    pub formats: Vec<String>,
    /// Discrete `(x, y)` resolutions.
    pub resolutions: Vec<(u32, u32)>,
    pub x_range: Option<Range>,
    pub y_range: Option<Range>,
    /// `scan:SupportedIntents` values (`Document`, `Photo`, `Preview`, ...).
    pub intents: Vec<String>,
}

impl InputCaps {
    /// The largest area, `(width, height)` in three hundredths of an inch.
    pub fn max_area(&self) -> Option<(u32, u32)> {
        Some((self.max_width?, self.max_height?)).filter(|(w, h)| *w > 0 && *h > 0)
    }
}

fn push_unique<T: PartialEq>(list: &mut Vec<T>, value: T) {
    if list.len() < MAX_LIST && !list.contains(&value) {
        list.push(value);
    }
}

fn read_input(node: Node<'_, '_>) -> InputCaps {
    let mut caps = InputCaps {
        min_width: child_u32(node, SCAN_NS, "MinWidth"),
        max_width: child_u32(node, SCAN_NS, "MaxWidth"),
        min_height: child_u32(node, SCAN_NS, "MinHeight"),
        max_height: child_u32(node, SCAN_NS, "MaxHeight"),
        max_optical_x: child_u32(node, SCAN_NS, "MaxOpticalXResolution"),
        max_optical_y: child_u32(node, SCAN_NS, "MaxOpticalYResolution"),
        ..InputCaps::default()
    };
    if let Some(intents) = child(node, SCAN_NS, "SupportedIntents") {
        for intent in children(intents, SCAN_NS, "Intent", MAX_LIST) {
            push_unique(&mut caps.intents, xml::text(intent));
        }
    }
    let Some(profiles) = child(node, SCAN_NS, "SettingProfiles") else { return caps };
    for profile in children(profiles, SCAN_NS, "SettingProfile", 16) {
        if let Some(modes) = child(profile, SCAN_NS, "ColorModes") {
            for mode in children(modes, SCAN_NS, "ColorMode", MAX_LIST) {
                push_unique(&mut caps.color_modes, ColorMode::parse(&xml::text(mode)));
            }
        }
        if let Some(formats) = child(profile, SCAN_NS, "DocumentFormats") {
            for format in
                formats.children().filter(|c| xml::is(*c, PWG_NS, "DocumentFormat") || xml::is(*c, SCAN_NS, "DocumentFormatExt")).take(MAX_LIST)
            {
                let text = xml::text(format).to_ascii_lowercase();
                if !text.is_empty() && text.len() <= 64 {
                    push_unique(&mut caps.formats, text);
                }
            }
        }
        let Some(supported) = child(profile, SCAN_NS, "SupportedResolutions") else { continue };
        if let Some(discrete) = child(supported, SCAN_NS, "DiscreteResolutions") {
            for r in children(discrete, SCAN_NS, "DiscreteResolution", MAX_LIST) {
                if let (Some(x), Some(y)) = (child_u32(r, SCAN_NS, "XResolution"), child_u32(r, SCAN_NS, "YResolution"))
                    && x > 0
                    && y > 0
                {
                    push_unique(&mut caps.resolutions, (x, y));
                }
            }
        }
        if let Some(range) = child(supported, SCAN_NS, "ResolutionRange") {
            caps.x_range = caps.x_range.or_else(|| child(range, SCAN_NS, "XResolutionRange").and_then(Range::read));
            caps.y_range = caps.y_range.or_else(|| child(range, SCAN_NS, "YResolutionRange").and_then(Range::read));
        }
    }
    caps.resolutions.sort_unstable();
    caps
}

/// A `ScannerCapabilities` document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Capabilities {
    /// `pwg:Version` (`2.0`, `2.63`, ...).
    pub version: Option<String>,
    pub make_and_model: Option<String>,
    pub serial_number: Option<String>,
    pub uuid: Option<String>,
    pub platen: Option<InputCaps>,
    pub adf_simplex: Option<InputCaps>,
    pub adf_duplex: Option<InputCaps>,
    pub feeder_capacity: Option<u32>,
    /// `scan:AdfOption` values (`DetectPaperLoaded`, `Duplex`, ...).
    pub adf_options: Vec<String>,
    pub brightness: Option<Range>,
    pub contrast: Option<Range>,
    pub threshold: Option<Range>,
}

impl Capabilities {
    /// Whether the feeder can scan both sides.
    pub fn duplex(&self) -> bool {
        self.adf_duplex.is_some() || (self.adf_simplex.is_some() && self.adf_options.iter().any(|o| o == "Duplex"))
    }

    /// The caps of the feeder, for two sides when `duplex` (falling back to the one-sided caps).
    pub fn feeder(&self, duplex: bool) -> Option<&InputCaps> {
        if duplex { self.adf_duplex.as_ref().or(self.adf_simplex.as_ref()) } else { self.adf_simplex.as_ref().or(self.adf_duplex.as_ref()) }
    }
}

/// Parse a `ScannerCapabilities` document.
pub fn parse_capabilities(bytes: &[u8]) -> Result<Capabilities> {
    let doc = xml::parse(bytes)?;
    let root = doc.root_element();
    if !xml::is(root, SCAN_NS, "ScannerCapabilities") {
        return Err(Error::Protocol(format!("expected ScannerCapabilities, got {}", root.tag_name().name().chars().take(64).collect::<String>())));
    }
    let mut caps = Capabilities {
        version: child_text(root, PWG_NS, "Version"),
        make_and_model: child_text(root, PWG_NS, "MakeAndModel"),
        serial_number: child_text(root, PWG_NS, "SerialNumber"),
        uuid: child_text(root, SCAN_NS, "UUID"),
        brightness: child(root, SCAN_NS, "BrightnessSupport").and_then(Range::read),
        contrast: child(root, SCAN_NS, "ContrastSupport").and_then(Range::read),
        threshold: child(root, SCAN_NS, "ThresholdSupport").and_then(Range::read),
        ..Capabilities::default()
    };
    caps.platen = child(root, SCAN_NS, "Platen").and_then(|p| child(p, SCAN_NS, "PlatenInputCaps")).map(read_input);
    if let Some(adf) = child(root, SCAN_NS, "Adf") {
        caps.adf_simplex = child(adf, SCAN_NS, "AdfSimplexInputCaps").map(read_input);
        caps.adf_duplex = child(adf, SCAN_NS, "AdfDuplexInputCaps").map(read_input);
        caps.feeder_capacity = child_u32(adf, SCAN_NS, "FeederCapacity");
        if let Some(options) = child(adf, SCAN_NS, "AdfOptions") {
            for option in children(options, SCAN_NS, "AdfOption", MAX_LIST) {
                push_unique(&mut caps.adf_options, xml::text(option));
            }
        }
    }
    Ok(caps)
}

/// The scanner's state (`pwg:State`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScannerState {
    Idle,
    Processing,
    Testing,
    Stopped,
    Down,
    Other(String),
}

impl ScannerState {
    fn parse(text: &str) -> Self {
        match text {
            "Idle" => Self::Idle,
            "Processing" => Self::Processing,
            "Testing" => Self::Testing,
            "Stopped" => Self::Stopped,
            "Down" => Self::Down,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// The feeder's state (`scan:AdfState`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AdfState {
    Processing,
    Empty,
    Jam,
    Loaded,
    Mispick,
    HatchOpen,
    DuplexPageTooShort,
    DuplexPageTooLong,
    MultipickDetected,
    InputTrayFailed,
    InputTrayOverloaded,
    Other(String),
}

impl AdfState {
    /// Read a value (`ScannerAdfLoaded`, ...).
    pub fn parse(text: &str) -> Self {
        match text {
            "ScannerAdfProcessing" => Self::Processing,
            "ScannerAdfEmpty" => Self::Empty,
            "ScannerAdfJam" => Self::Jam,
            "ScannerAdfLoaded" => Self::Loaded,
            "ScannerAdfMispick" => Self::Mispick,
            "ScannerAdfHatchOpen" => Self::HatchOpen,
            "ScannerAdfDuplexPageTooShort" => Self::DuplexPageTooShort,
            "ScannerAdfDuplexPageTooLong" => Self::DuplexPageTooLong,
            "ScannerAdfMultipickDetected" => Self::MultipickDetected,
            "ScannerAdfInputTrayFailed" => Self::InputTrayFailed,
            "ScannerAdfInputTrayOverloaded" => Self::InputTrayOverloaded,
            other => Self::Other(other.to_owned()),
        }
    }

    /// The value on the wire.
    pub fn name(&self) -> &str {
        match self {
            Self::Processing => "ScannerAdfProcessing",
            Self::Empty => "ScannerAdfEmpty",
            Self::Jam => "ScannerAdfJam",
            Self::Loaded => "ScannerAdfLoaded",
            Self::Mispick => "ScannerAdfMispick",
            Self::HatchOpen => "ScannerAdfHatchOpen",
            Self::DuplexPageTooShort => "ScannerAdfDuplexPageTooShort",
            Self::DuplexPageTooLong => "ScannerAdfDuplexPageTooLong",
            Self::MultipickDetected => "ScannerAdfMultipickDetected",
            Self::InputTrayFailed => "ScannerAdfInputTrayFailed",
            Self::InputTrayOverloaded => "ScannerAdfInputTrayOverloaded",
            Self::Other(text) => text,
        }
    }
}

/// A job's state (`pwg:JobState`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum JobState {
    Pending,
    Processing,
    Completed,
    Canceled,
    Aborted,
    Other(String),
}

impl JobState {
    fn parse(text: &str) -> Self {
        match text {
            "Pending" => Self::Pending,
            "Processing" => Self::Processing,
            "Completed" => Self::Completed,
            "Canceled" => Self::Canceled,
            "Aborted" => Self::Aborted,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// One `scan:JobInfo` of the status.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct JobInfo {
    /// `pwg:JobUri`, as the scanner wrote it (usually a path such as `/eSCL/ScanJobs/<id>`).
    pub uri: Option<String>,
    pub uuid: Option<String>,
    pub state: Option<JobState>,
    /// `pwg:JobStateReason` values.
    pub reasons: Vec<String>,
    pub images_completed: Option<u32>,
    pub images_to_transfer: Option<u32>,
}

/// A `ScannerStatus` document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Status {
    pub state: Option<ScannerState>,
    pub adf_state: Option<AdfState>,
    pub jobs: Vec<JobInfo>,
}

impl Status {
    /// The job whose `JobUri` ends with `path` (the job's path on this scanner).
    pub fn job(&self, path: &str) -> Option<&JobInfo> {
        let path = path.trim_end_matches('/');
        self.jobs.iter().find(|j| j.uri.as_deref().is_some_and(|u| !path.is_empty() && u.trim_end_matches('/').ends_with(path)))
    }
}

/// Parse a `ScannerStatus` document.
pub fn parse_status(bytes: &[u8]) -> Result<Status> {
    let doc = xml::parse(bytes)?;
    let root = doc.root_element();
    if !xml::is(root, SCAN_NS, "ScannerStatus") {
        return Err(Error::Protocol(format!("expected ScannerStatus, got {}", root.tag_name().name().chars().take(64).collect::<String>())));
    }
    let mut status = Status {
        state: child_text(root, PWG_NS, "State").map(|s| ScannerState::parse(&s)),
        adf_state: child_text(root, SCAN_NS, "AdfState").map(|s| AdfState::parse(&s)),
        jobs: Vec::new(),
    };
    if let Some(jobs) = child(root, SCAN_NS, "Jobs") {
        for info in children(jobs, SCAN_NS, "JobInfo", MAX_LIST) {
            let mut job = JobInfo {
                uri: child_text(info, PWG_NS, "JobUri"),
                uuid: child_text(info, PWG_NS, "JobUuid"),
                state: child_text(info, PWG_NS, "JobState").map(|s| JobState::parse(&s)),
                images_completed: child_u32(info, PWG_NS, "ImagesCompleted"),
                images_to_transfer: child_u32(info, PWG_NS, "ImagesToTransfer"),
                ..JobInfo::default()
            };
            if let Some(reasons) = child(info, PWG_NS, "JobStateReasons") {
                for reason in children(reasons, PWG_NS, "JobStateReason", 16) {
                    push_unique(&mut job.reasons, xml::text(reason));
                }
            }
            status.jobs.push(job);
        }
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<scan:ScannerCapabilities xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03" xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm">
  <pwg:Version>2.63</pwg:Version>
  <pwg:MakeAndModel>Example Scanner</pwg:MakeAndModel>
  <scan:UUID>00000000-0000-0000-0000-000000000001</scan:UUID>
  <scan:Platen><scan:PlatenInputCaps>
    <scan:MinWidth>16</scan:MinWidth><scan:MaxWidth>2550</scan:MaxWidth>
    <scan:MinHeight>16</scan:MinHeight><scan:MaxHeight>3507</scan:MaxHeight>
    <scan:SettingProfiles><scan:SettingProfile>
      <scan:ColorModes><scan:ColorMode>RGB24</scan:ColorMode><scan:ColorMode>Grayscale8</scan:ColorMode><scan:ColorMode>RGB24</scan:ColorMode></scan:ColorModes>
      <scan:DocumentFormats><pwg:DocumentFormat>image/jpeg</pwg:DocumentFormat><pwg:DocumentFormat>application/pdf</pwg:DocumentFormat><scan:DocumentFormatExt>image/png</scan:DocumentFormatExt></scan:DocumentFormats>
      <scan:SupportedResolutions><scan:DiscreteResolutions>
        <scan:DiscreteResolution><scan:XResolution>300</scan:XResolution><scan:YResolution>300</scan:YResolution></scan:DiscreteResolution>
        <scan:DiscreteResolution><scan:XResolution>100</scan:XResolution><scan:YResolution>100</scan:YResolution></scan:DiscreteResolution>
      </scan:DiscreteResolutions></scan:SupportedResolutions>
    </scan:SettingProfile></scan:SettingProfiles>
    <scan:SupportedIntents><scan:Intent>Document</scan:Intent><scan:Intent>Photo</scan:Intent></scan:SupportedIntents>
    <scan:MaxOpticalXResolution>1200</scan:MaxOpticalXResolution>
  </scan:PlatenInputCaps></scan:Platen>
  <scan:Adf>
    <scan:AdfSimplexInputCaps><scan:MaxWidth>2550</scan:MaxWidth><scan:MaxHeight>4200</scan:MaxHeight>
      <scan:SettingProfiles><scan:SettingProfile><scan:SupportedResolutions><scan:ResolutionRange>
        <scan:XResolutionRange><scan:Min>75</scan:Min><scan:Max>600</scan:Max><scan:Step>25</scan:Step></scan:XResolutionRange>
        <scan:YResolutionRange><scan:Min>75</scan:Min><scan:Max>600</scan:Max><scan:Step>25</scan:Step></scan:YResolutionRange>
      </scan:ResolutionRange></scan:SupportedResolutions></scan:SettingProfile></scan:SettingProfiles>
    </scan:AdfSimplexInputCaps>
    <scan:FeederCapacity>50</scan:FeederCapacity>
    <scan:AdfOptions><scan:AdfOption>DetectPaperLoaded</scan:AdfOption><scan:AdfOption>Duplex</scan:AdfOption></scan:AdfOptions>
  </scan:Adf>
  <scan:BrightnessSupport><scan:Min>0</scan:Min><scan:Max>100</scan:Max><scan:Normal>50</scan:Normal><scan:Step>1</scan:Step></scan:BrightnessSupport>
</scan:ScannerCapabilities>"#;

    #[test]
    fn capabilities_are_read() {
        let caps = parse_capabilities(CAPS.as_bytes()).unwrap();
        assert_eq!(caps.version.as_deref(), Some("2.63"));
        assert_eq!(caps.make_and_model.as_deref(), Some("Example Scanner"));
        let platen = caps.platen.as_ref().unwrap();
        assert_eq!(platen.max_area(), Some((2550, 3507)));
        assert_eq!(platen.color_modes, [ColorMode::Rgb24, ColorMode::Grayscale8]);
        assert_eq!(platen.formats, ["image/jpeg", "application/pdf", "image/png"]);
        assert_eq!(platen.resolutions, [(100, 100), (300, 300)]);
        assert_eq!(platen.intents, ["Document", "Photo"]);
        let adf = caps.adf_simplex.as_ref().unwrap();
        assert_eq!(adf.x_range, Some(Range { min: 75, max: 600, normal: None, step: 25 }));
        assert!(caps.duplex(), "the Duplex option without duplex caps");
        assert_eq!(caps.feeder(true), caps.adf_simplex.as_ref());
        assert_eq!(caps.feeder_capacity, Some(50));
        assert_eq!(caps.brightness.map(|b| (b.min, b.max, b.normal)), Some((0, 100, Some(50))));
    }

    #[test]
    fn other_documents_and_namespaces_are_refused() {
        assert!(matches!(parse_capabilities(b"<ScannerCapabilities/>"), Err(Error::Protocol(_))));
        assert!(matches!(parse_status(CAPS.as_bytes()), Err(Error::Protocol(_))));
        assert!(parse_capabilities(b"not xml").is_err());
    }

    #[test]
    fn status_is_read() {
        let doc = r#"<scan:ScannerStatus xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03" xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm">
          <pwg:Version>2.0</pwg:Version><pwg:State>Processing</pwg:State><scan:AdfState>ScannerAdfJam</scan:AdfState>
          <scan:Jobs><scan:JobInfo><pwg:JobUri>/eSCL/ScanJobs/abc</pwg:JobUri><pwg:JobUuid>abc</pwg:JobUuid>
            <pwg:ImagesCompleted>1</pwg:ImagesCompleted><pwg:JobState>Aborted</pwg:JobState>
            <pwg:JobStateReasons><pwg:JobStateReason>JobCanceledByUser</pwg:JobStateReason></pwg:JobStateReasons></scan:JobInfo></scan:Jobs>
        </scan:ScannerStatus>"#;
        let status = parse_status(doc.as_bytes()).unwrap();
        assert_eq!(status.state, Some(ScannerState::Processing));
        assert_eq!(status.adf_state, Some(AdfState::Jam));
        let job = status.job("/eSCL/ScanJobs/abc").unwrap();
        assert_eq!(
            (job.state.clone(), job.images_completed, job.reasons.clone()),
            (Some(JobState::Aborted), Some(1), vec!["JobCanceledByUser".to_owned()])
        );
        assert!(status.job("/eSCL/ScanJobs/zzz").is_none());
        assert_eq!(AdfState::parse("ScannerAdfSomethingNew"), AdfState::Other("ScannerAdfSomethingNew".into()));
    }
}
