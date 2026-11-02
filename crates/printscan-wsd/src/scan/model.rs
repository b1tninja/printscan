//! What a scanner reports in `GetScannerElements`: its description, its configuration (formats,
//! the platen and the feeder) and its status, as typed values.
//!
//! Every list read from a reply is capped and every number is checked: a scanner that answers
//! nonsense gets fewer choices, never a panic or an allocation it chose.

use roxmltree::Node;

use super::ns::WSCN;
use crate::xml;

/// How many values of one list (formats, resolutions, colours, reasons) are kept.
const MAX_LIST: usize = 64;

/// A dimension or offset no scanner has: 100 inches, in thousandths of an inch.
const MAX_MILS: u32 = 100_000;

/// A resolution no scanner has.
const MAX_DPI: u32 = 19_200;

/// The `ScannerDescription` element.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Description {
    pub name: Option<String>,
    pub info: Option<String>,
    pub location: Option<String>,
}

/// A colour mode (`ColorEntry`, `ColorProcessing`). The modes the scan core cannot ask for are
/// kept by name so they still show up in a diagnostic dump.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ColorMode {
    BlackAndWhite1,
    Grayscale8,
    Rgb24,
    Other(String),
}

impl ColorMode {
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "BlackAndWhite1" => Self::BlackAndWhite1,
            "Grayscale8" => Self::Grayscale8,
            "RGB24" => Self::Rgb24,
            other => Self::Other(other.to_owned()),
        }
    }

    /// The value as the protocol writes it.
    pub fn name(&self) -> &str {
        match self {
            Self::BlackAndWhite1 => "BlackAndWhite1",
            Self::Grayscale8 => "Grayscale8",
            Self::Rgb24 => "RGB24",
            Self::Other(s) => s,
        }
    }
}

/// One input: the platen, or one side of the feeder (`PlatenType`, `ADFEntryType`). Sizes are in
/// thousandths of an inch, as the protocol has them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Input {
    /// Optical resolution (width, height); the height is the width when the scanner names none.
    pub optical: Option<(u32, u32)>,
    pub widths: Vec<u32>,
    pub heights: Vec<u32>,
    pub colors: Vec<ColorMode>,
    pub minimum: Option<(u32, u32)>,
    pub maximum: Option<(u32, u32)>,
}

/// The feeder (`ADFType`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Feeder {
    pub duplex: bool,
    pub front: Input,
    pub back: Option<Input>,
}

/// The `ScannerConfiguration` element.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Configuration {
    /// `FormatValue`s, in the scanner's order.
    pub formats: Vec<String>,
    pub auto_exposure: bool,
    pub brightness: bool,
    pub contrast: bool,
    pub size_auto_detect: bool,
    /// `RotationsSupported`, in degrees.
    pub rotations: Vec<u32>,
    pub platen: Option<Input>,
    pub feeder: Option<Feeder>,
}

/// `ScannerState`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScannerState {
    Idle,
    Processing,
    Stopped,
    Other(String),
}

impl ScannerState {
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "Idle" => Self::Idle,
            "Processing" => Self::Processing,
            "Stopped" => Self::Stopped,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// One active condition (`DeviceConditionType`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Condition {
    pub name: String,
    pub component: String,
    pub severity: String,
}

/// The `ScannerStatus` element.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Status {
    pub state: ScannerState,
    /// `ScannerStateReason`s other than `None`.
    pub reasons: Vec<String>,
    pub conditions: Vec<Condition>,
}

impl Status {
    /// Is `reason` among the state reasons or the active conditions' names?
    pub fn names(&self, reason: &str) -> bool {
        self.reasons.iter().any(|r| r == reason) || self.conditions.iter().any(|c| c.name == reason)
    }
}

/// What one `GetScannerElements` returned; a section the scanner left out or marked
/// `Valid="false"` is `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScannerElements {
    pub description: Option<Description>,
    pub configuration: Option<Configuration>,
    pub status: Option<Status>,
}

/// A positive number no larger than `max`.
fn number(node: Node<'_, '_>, max: u32) -> Option<u32> {
    xml::text(node).parse::<u32>().ok().filter(|v| (1..=max).contains(v))
}

fn child_number(node: Node<'_, '_>, local: &str, max: u32) -> Option<u32> {
    xml::child(node, WSCN, local).and_then(|n| number(n, max))
}

fn child_bool(node: Node<'_, '_>, local: &str) -> bool {
    xml::child_text(node, WSCN, local).and_then(|t| xml::parse_bool(&t)).unwrap_or(false)
}

/// `Width`/`Height` of a `DimensionsType` (both required).
fn dimensions(node: Option<Node<'_, '_>>) -> Option<(u32, u32)> {
    let node = node?;
    Some((child_number(node, "Width", MAX_MILS)?, child_number(node, "Height", MAX_MILS)?))
}

/// The numbers of every `{wscn}local` child, deduplicated, ascending, capped.
fn number_list(node: Option<Node<'_, '_>>, local: &str, max: u32) -> Vec<u32> {
    let Some(node) = node else { return Vec::new() };
    let mut out: Vec<u32> = xml::children(node, WSCN, local).take(MAX_LIST).filter_map(|n| number(n, max)).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// One input, read from the children named `<prefix>OpticalResolution`, `<prefix>Resolutions`, ...
fn input(node: Node<'_, '_>, prefix: &str) -> Input {
    let el = |suffix: &str| xml::child(node, WSCN, &format!("{prefix}{suffix}"));
    let optical = el("OpticalResolution").and_then(|o| {
        let w = child_number(o, "Width", MAX_DPI)?;
        Some((w, child_number(o, "Height", MAX_DPI).unwrap_or(w)))
    });
    let resolutions = el("Resolutions");
    let widths = number_list(resolutions.and_then(|r| xml::child(r, WSCN, "Widths")), "Width", MAX_DPI);
    let mut heights = number_list(resolutions.and_then(|r| xml::child(r, WSCN, "Heights")), "Height", MAX_DPI);
    if heights.is_empty() {
        heights.clone_from(&widths);
    }
    let colors = el("Color")
        .map(|c| xml::children(c, WSCN, "ColorEntry").take(MAX_LIST).map(|e| ColorMode::parse(&xml::text(e))).collect())
        .unwrap_or_default();
    Input { optical, widths, heights, colors, minimum: dimensions(el("MinimumSize")), maximum: dimensions(el("MaximumSize")) }
}

/// Parse a `ScannerConfiguration` element.
pub fn parse_configuration(node: Node<'_, '_>) -> Configuration {
    let settings = xml::child(node, WSCN, "DeviceSettings");
    let formats = settings
        .and_then(|s| xml::child(s, WSCN, "FormatsSupported"))
        .map(|f| {
            let mut out: Vec<String> = Vec::new();
            for v in xml::children(f, WSCN, "FormatValue").take(MAX_LIST).map(xml::text) {
                if !v.is_empty() && !out.contains(&v) {
                    out.push(v);
                }
            }
            out
        })
        .unwrap_or_default();
    let flag = |local: &str| settings.is_some_and(|s| child_bool(s, local));
    let rotations = number_list(settings.and_then(|s| xml::child(s, WSCN, "RotationsSupported")), "RotationValue", 359);
    let platen = xml::child(node, WSCN, "Platen").map(|p| input(p, "Platen"));
    let feeder = xml::child(node, WSCN, "ADF").and_then(|adf| {
        let front = xml::child(adf, WSCN, "ADFFront")?;
        Some(Feeder {
            duplex: child_bool(adf, "ADFSupportsDuplex"),
            front: input(front, "ADF"),
            back: xml::child(adf, WSCN, "ADFBack").map(|b| input(b, "ADF")),
        })
    });
    Configuration {
        formats,
        auto_exposure: flag("AutoExposureSupported"),
        brightness: flag("BrightnessSupported"),
        contrast: flag("ContrastSupported"),
        size_auto_detect: flag("DocumentSizeAutoDetectSupported"),
        rotations,
        platen,
        feeder,
    }
}

/// Parse a `ScannerStatus` element.
pub fn parse_status(node: Node<'_, '_>) -> Status {
    let state = ScannerState::parse(&xml::child_text(node, WSCN, "ScannerState").unwrap_or_default());
    let reasons = xml::child(node, WSCN, "ScannerStateReasons")
        .map(|r| xml::children(r, WSCN, "ScannerStateReason").take(MAX_LIST).map(xml::text).filter(|t| !t.is_empty() && t != "None").collect())
        .unwrap_or_default();
    let conditions = xml::child(node, WSCN, "ActiveConditions")
        .map(|a| {
            xml::children(a, WSCN, "DeviceCondition")
                .take(MAX_LIST)
                .map(|c| Condition {
                    name: xml::child_text(c, WSCN, "Name").unwrap_or_default(),
                    component: xml::child_text(c, WSCN, "Component").unwrap_or_default(),
                    severity: xml::child_text(c, WSCN, "Severity").unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    Status { state, reasons, conditions }
}

/// Parse a `ScannerDescription` element.
pub fn parse_description(node: Node<'_, '_>) -> Description {
    Description {
        name: xml::child_text(node, WSCN, "ScannerName"),
        info: xml::child_text(node, WSCN, "ScannerInfo"),
        location: xml::child_text(node, WSCN, "ScannerLocation"),
    }
}

/// Parse a `GetScannerElementsResponse`: each `ElementData` that is valid and holds one of the
/// three sections. Unknown and invalid sections are skipped.
pub fn parse_scanner_elements(response: Node<'_, '_>) -> ScannerElements {
    let mut out = ScannerElements::default();
    let Some(elements) = xml::child(response, WSCN, "ScannerElements") else { return out };
    for data in xml::children(elements, WSCN, "ElementData").take(16) {
        if data.attribute("Valid").and_then(xml::parse_bool) == Some(false) {
            continue;
        }
        if let Some(n) = xml::child(data, WSCN, "ScannerDescription") {
            out.description = Some(parse_description(n));
        }
        if let Some(n) = xml::child(data, WSCN, "ScannerConfiguration") {
            out.configuration = Some(parse_configuration(n));
        }
        if let Some(n) = xml::child(data, WSCN, "ScannerStatus") {
            out.status = Some(parse_status(n));
        }
    }
    out
}

/// What `CreateScanJob` answered: the job's id and token, and the front image's size when the
/// scanner gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CreatedJob {
    pub id: u32,
    pub token: String,
    /// `PixelsPerLine`, `NumberOfLines` and `BytesPerLine` of the front side.
    pub front: Option<(u32, u32, u32)>,
    /// The `Format` the scanner settled on (`DocumentFinalParameters`), when it says.
    pub format: Option<String>,
}

/// Parse a `CreateScanJobResponse`.
pub fn parse_created_job(response: Node<'_, '_>) -> Result<CreatedJob, crate::Error> {
    let id = xml::child(response, WSCN, "JobId")
        .and_then(|n| number(n, u32::MAX))
        .ok_or_else(|| crate::Error::Protocol("CreateScanJobResponse without a JobId".into()))?;
    let token = xml::child_text(response, WSCN, "JobToken")
        .filter(|t| t.len() <= 255)
        .ok_or_else(|| crate::Error::Protocol("CreateScanJobResponse without a JobToken".into()))?;
    let front = xml::child(response, WSCN, "ImageInformation").and_then(|i| xml::child(i, WSCN, "MediaFrontImageInfo")).and_then(|f| {
        let bytes = xml::child_text(f, WSCN, "BytesPerLine").and_then(|t| t.parse().ok()).unwrap_or(0);
        Some((child_number(f, "PixelsPerLine", u32::MAX)?, child_number(f, "NumberOfLines", u32::MAX)?, bytes))
    });
    let format = xml::child(response, WSCN, "DocumentFinalParameters").and_then(|p| xml::child_text(p, WSCN, "Format"));
    Ok(CreatedJob { id, token, front, format })
}

/// A job's `JobStatus`, from `GetJobElements`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct JobStatus {
    /// `JobState` as sent (`Processing`, `Canceled`, `Aborted`, ...).
    pub state: String,
    /// `JobStateReason`s other than `None`.
    pub reasons: Vec<String>,
}

impl JobStatus {
    /// The scanner says the job was cancelled at the device (its panel, or the scanner itself).
    pub fn cancelled_at_device(&self) -> bool {
        self.reasons.iter().any(|r| r == "JobCanceledAtDevice")
    }
}

/// Parse a `GetJobElementsResponse` for its `JobStatus`; `None` when it has no valid one.
pub fn parse_job_status(response: Node<'_, '_>) -> Option<JobStatus> {
    let elements = xml::child(response, WSCN, "JobElements")?;
    xml::children(elements, WSCN, "ElementData").take(16).filter(|d| d.attribute("Valid").and_then(xml::parse_bool) != Some(false)).find_map(|d| {
        let status = xml::child(d, WSCN, "JobStatus")?;
        let state = xml::child_text(status, WSCN, "JobState").unwrap_or_default();
        let reasons = xml::child(status, WSCN, "JobStateReasons")
            .map(|r| xml::children(r, WSCN, "JobStateReason").take(MAX_LIST).map(xml::text).filter(|t| !t.is_empty() && t != "None").collect())
            .unwrap_or_default();
        Some(JobStatus { state, reasons })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_status_reads_state_and_reasons() {
        let doc = format!(
            r#"<wscn:GetJobElementsResponse xmlns:wscn="{WSCN}"><wscn:JobElements>
              <wscn:ElementData Name="wscn:JobStatus" Valid="false"><wscn:JobStatus><wscn:JobState>Processing</wscn:JobState></wscn:JobStatus></wscn:ElementData>
              <wscn:ElementData Name="wscn:JobStatus" Valid="true"><wscn:JobStatus><wscn:JobId>7</wscn:JobId><wscn:JobState>Canceled</wscn:JobState>
                <wscn:JobStateReasons><wscn:JobStateReason>JobCanceledAtDevice</wscn:JobStateReason><wscn:JobStateReason>None</wscn:JobStateReason></wscn:JobStateReasons>
                <wscn:ScansCompleted>0</wscn:ScansCompleted></wscn:JobStatus></wscn:ElementData>
            </wscn:JobElements></wscn:GetJobElementsResponse>"#
        );
        let doc = roxmltree::Document::parse(&doc).unwrap();
        let status = parse_job_status(doc.root_element()).unwrap();
        assert_eq!(status, JobStatus { state: "Canceled".into(), reasons: vec!["JobCanceledAtDevice".into()] });
        assert!(status.cancelled_at_device());
        assert!(!JobStatus { state: "Canceled".into(), reasons: Vec::new() }.cancelled_at_device(), "our own CancelJob is not the device's");

        let empty = format!(r#"<wscn:GetJobElementsResponse xmlns:wscn="{WSCN}"/>"#);
        let empty = roxmltree::Document::parse(&empty).unwrap();
        assert_eq!(parse_job_status(empty.root_element()), None);
    }

    fn parse(body: &str) -> ScannerElements {
        let doc = format!(
            "<wscn:GetScannerElementsResponse xmlns:wscn=\"{WSCN}\"><wscn:ScannerElements>{body}</wscn:ScannerElements></wscn:GetScannerElementsResponse>"
        );
        let doc = roxmltree::Document::parse(&doc).unwrap();
        parse_scanner_elements(doc.root_element())
    }

    const CONFIGURATION: &str = r#"<wscn:ElementData Name="wscn:ScannerConfiguration" Valid="true"><wscn:ScannerConfiguration>
      <wscn:DeviceSettings>
        <wscn:FormatsSupported><wscn:FormatValue>jfif</wscn:FormatValue><wscn:FormatValue>png</wscn:FormatValue><wscn:FormatValue>png</wscn:FormatValue></wscn:FormatsSupported>
        <wscn:CompressionQualityFactorSupported><wscn:MinValue>1</wscn:MinValue><wscn:MaxValue>100</wscn:MaxValue></wscn:CompressionQualityFactorSupported>
        <wscn:ContentTypesSupported><wscn:ContentTypeValue>Auto</wscn:ContentTypeValue></wscn:ContentTypesSupported>
        <wscn:DocumentSizeAutoDetectSupported>false</wscn:DocumentSizeAutoDetectSupported>
        <wscn:AutoExposureSupported>false</wscn:AutoExposureSupported>
        <wscn:BrightnessSupported>true</wscn:BrightnessSupported>
        <wscn:ContrastSupported>1</wscn:ContrastSupported>
        <wscn:ScalingRangeSupported><wscn:ScalingWidth><wscn:MinValue>100</wscn:MinValue><wscn:MaxValue>100</wscn:MaxValue></wscn:ScalingWidth></wscn:ScalingRangeSupported>
        <wscn:RotationsSupported><wscn:RotationValue>0</wscn:RotationValue><wscn:RotationValue>180</wscn:RotationValue></wscn:RotationsSupported>
      </wscn:DeviceSettings>
      <wscn:Platen>
        <wscn:PlatenOpticalResolution><wscn:Width>1200</wscn:Width><wscn:Height>2400</wscn:Height></wscn:PlatenOpticalResolution>
        <wscn:PlatenResolutions><wscn:Widths><wscn:Width>300</wscn:Width><wscn:Width>100</wscn:Width><wscn:Width>0</wscn:Width><wscn:Width>x</wscn:Width></wscn:Widths><wscn:Heights><wscn:Height>100</wscn:Height><wscn:Height>300</wscn:Height></wscn:Heights></wscn:PlatenResolutions>
        <wscn:PlatenColor><wscn:ColorEntry>RGB24</wscn:ColorEntry><wscn:ColorEntry>Grayscale8</wscn:ColorEntry><wscn:ColorEntry>RGB48</wscn:ColorEntry></wscn:PlatenColor>
        <wscn:PlatenMinimumSize><wscn:Width>1</wscn:Width><wscn:Height>1</wscn:Height></wscn:PlatenMinimumSize>
        <wscn:PlatenMaximumSize><wscn:Width>8500</wscn:Width><wscn:Height>11690</wscn:Height></wscn:PlatenMaximumSize>
      </wscn:Platen>
      <wscn:ADF>
        <wscn:ADFSupportsDuplex>true</wscn:ADFSupportsDuplex>
        <wscn:ADFFront>
          <wscn:ADFOpticalResolution><wscn:Width>600</wscn:Width></wscn:ADFOpticalResolution>
          <wscn:ADFResolutions><wscn:Widths><wscn:Width>200</wscn:Width></wscn:Widths><wscn:Heights><wscn:Height>200</wscn:Height></wscn:Heights></wscn:ADFResolutions>
          <wscn:ADFColor><wscn:ColorEntry>BlackAndWhite1</wscn:ColorEntry></wscn:ADFColor>
          <wscn:ADFMinimumSize><wscn:Width>2000</wscn:Width><wscn:Height>2000</wscn:Height></wscn:ADFMinimumSize>
          <wscn:ADFMaximumSize><wscn:Width>8500</wscn:Width><wscn:Height>14000</wscn:Height></wscn:ADFMaximumSize>
        </wscn:ADFFront>
      </wscn:ADF>
    </wscn:ScannerConfiguration></wscn:ElementData>"#;

    #[test]
    fn the_configuration_is_read_with_its_lists_cleaned() {
        let c = parse(CONFIGURATION).configuration.unwrap();
        assert_eq!(c.formats, ["jfif", "png"]);
        assert!(c.brightness && c.contrast && !c.auto_exposure && !c.size_auto_detect);
        assert_eq!(c.rotations, [180]);
        let platen = c.platen.unwrap();
        assert_eq!(platen.optical, Some((1200, 2400)));
        assert_eq!(platen.widths, [100, 300], "zero and junk are dropped, the rest sorted");
        assert_eq!(platen.colors, [ColorMode::Rgb24, ColorMode::Grayscale8, ColorMode::Other("RGB48".into())]);
        assert_eq!(platen.maximum, Some((8500, 11690)));
        let feeder = c.feeder.unwrap();
        assert!(feeder.duplex && feeder.back.is_none());
        assert_eq!(feeder.front.optical, Some((600, 600)), "a missing optical height is the width");
        assert_eq!(feeder.front.maximum, Some((8500, 14000)));
    }

    #[test]
    fn status_reasons_and_conditions_are_read_and_invalid_sections_skipped() {
        let e = parse(
            r#"<wscn:ElementData Name="wscn:ScannerStatus" Valid="true"><wscn:ScannerStatus>
              <wscn:ScannerCurrentTime>2026-11-01T00:00:00Z</wscn:ScannerCurrentTime>
              <wscn:ScannerState>Stopped</wscn:ScannerState>
              <wscn:ScannerStateReasons><wscn:ScannerStateReason>MediaJam</wscn:ScannerStateReason><wscn:ScannerStateReason>None</wscn:ScannerStateReason></wscn:ScannerStateReasons>
              <wscn:ActiveConditions><wscn:DeviceCondition Id="1"><wscn:Time>2026-11-01T00:00:00Z</wscn:Time><wscn:Name>CoverOpen</wscn:Name><wscn:Component>ADF</wscn:Component><wscn:Severity>Critical</wscn:Severity></wscn:DeviceCondition></wscn:ActiveConditions>
            </wscn:ScannerStatus></wscn:ElementData>
            <wscn:ElementData Name="wscn:ScannerDescription" Valid="false"><wscn:ScannerDescription><wscn:ScannerName>x</wscn:ScannerName></wscn:ScannerDescription></wscn:ElementData>"#,
        );
        assert!(e.description.is_none());
        let s = e.status.unwrap();
        assert_eq!(s.state, ScannerState::Stopped);
        assert_eq!(s.reasons, ["MediaJam"]);
        assert!(s.names("CoverOpen") && s.names("MediaJam") && !s.names("None"));
        assert_eq!(s.conditions[0].component, "ADF");
    }

    #[test]
    fn a_created_job_needs_an_id_and_a_token() {
        let read = |body: &str| {
            let doc = format!("<wscn:CreateScanJobResponse xmlns:wscn=\"{WSCN}\">{body}</wscn:CreateScanJobResponse>");
            let doc = roxmltree::Document::parse(&doc).unwrap();
            parse_created_job(doc.root_element())
        };
        let job = read(
            "<wscn:JobId>7</wscn:JobId><wscn:JobToken>t-7</wscn:JobToken>\
             <wscn:ImageInformation><wscn:MediaFrontImageInfo><wscn:PixelsPerLine>2550</wscn:PixelsPerLine><wscn:NumberOfLines>3300</wscn:NumberOfLines><wscn:BytesPerLine>0</wscn:BytesPerLine></wscn:MediaFrontImageInfo></wscn:ImageInformation>\
             <wscn:DocumentFinalParameters><wscn:Format>png</wscn:Format></wscn:DocumentFinalParameters>",
        )
        .unwrap();
        assert_eq!(job, CreatedJob { id: 7, token: "t-7".into(), front: Some((2550, 3300, 0)), format: Some("png".into()) });
        assert!(read("<wscn:JobToken>t</wscn:JobToken>").is_err());
        assert!(read("<wscn:JobId>0</wscn:JobId><wscn:JobToken>t</wscn:JobToken>").is_err());
        assert!(read("<wscn:JobId>3</wscn:JobId>").is_err());
    }
}
