//! The derived `serde` shapes of the scanner description and the page and error records carry everything the
//! apps' CLI and bindings printed with hand-written JSON. The `*_json` functions below are that JSON, kept as the
//! reference; the `cli_*` functions are what an app adds to the derived value (computed fields, flags instead of
//! the tri-state, no `reported`) to print exactly that, the way it decorates any library value it embeds.

use printscan::scan::{
    Area, Color, Connection, HandlingStatus, ImageChoices, Intent, InterruptKind, LengthRecord, LengthSource, NativeCode, PageLengthSupport,
    PageSize, Presence, Rotation, ScanError, ScanErrorInfo, Scanner, Source, ValueSpan,
};
use serde_json::{Value, json};

fn span_json(span: Option<ValueSpan>) -> Value {
    span.map(|span| json!({ "min": span.min, "max": span.max, "step": span.step })).unwrap_or(Value::Null)
}

fn area_json(area: Option<Area>) -> Value {
    area.map(|area| json!({ "width": area.width, "height": area.height })).unwrap_or(Value::Null)
}

fn status_json(status: &HandlingStatus) -> Value {
    Value::Object(status.flags().into_iter().map(|(name, on)| (name.to_string(), Value::Bool(on))).collect())
}

fn page_length_support_json(support: PageLengthSupport) -> Value {
    json!({ "device_detect": support.device_detect.as_str(), "max_length_mils": support.max_length_mils })
}

fn choices_json(choices: Option<&ImageChoices>) -> Value {
    let Some(choices) = choices else {
        return Value::Null;
    };
    json!({
        "dpi": choices.dpi,
        "dpi_y": choices.dpi_y,
        "colors": choices.colors.iter().map(|color| color.as_str()).collect::<Vec<_>>(),
        "brightness": span_json(choices.brightness),
        "contrast": span_json(choices.contrast),
        "threshold": span_json(choices.threshold),
        "rotation": choices.rotation.iter().map(|turn| turn.name()).collect::<Vec<_>>(),
        "formats": choices.formats,
        "compression": choices.compression,
        "intents": choices.intents.iter().map(|intent| intent.as_str()).collect::<Vec<_>>(),
        "page_sizes": choices.page_sizes.iter().map(|size| size.as_str()).collect::<Vec<_>>(),
        "auto_deskew": choices.auto_deskew,
        "orientation": choices.orientation,
        "photometric": choices.photometric,
        "invert": choices.invert,
        "mirror": choices.mirror,
        "lamp": choices.lamp,
    })
}

fn scanner_json(scanner: &Scanner) -> Value {
    let status = scanner.handling_status;
    json!({
        "id": scanner.id,
        "name": scanner.name,
        "feeder": scanner.feeder,
        "flatbed": scanner.flatbed,
        "duplex": scanner.duplex,
        "feeder_ready": scanner.feeder_ready,
        "handling_status": status_json(&status),
        "conditions": status.conditions(),
        "feeder_capacity": scanner.feeder_capacity,
        "connection": scanner.connection.as_str(),
        "port": scanner.port,
        "optical_x": scanner.optical_x,
        "optical_y": scanner.optical_y,
        "pages": span_json(scanner.pages),
        "bed": area_json(scanner.bed),
        "sheet": area_json(scanner.sheet),
        "glass": choices_json(scanner.glass.as_ref()),
        "feeder_choices": choices_json(scanner.feeder_choices.as_ref()),
        "page_length": {
            "glass": page_length_support_json(scanner.length_support(Source::Glass)),
            "feeder": page_length_support_json(scanner.length_support(Source::Feeder)),
        },
    })
}

fn scan_length_json(record: &LengthRecord) -> Value {
    json!({
        "source": record.source.as_str(),
        "length_mils": record.length_mils,
        "length_inches": record.length_inches().map(|inches| (inches * 1000.0).round() / 1000.0),
        "length_mm": record.length_mm().map(|mm| (mm * 10.0).round() / 10.0),
        "approximate": record.approximate,
    })
}

fn scan_error_info_json(info: &ScanErrorInfo) -> Value {
    json!({
        "kind": info.kind.as_str(),
        "phase": info.phase.as_str(),
        "hresult": info.hresult_hex(),
        "code_name": info.code_name(),
        "retryable": info.retryable,
        "resumable": info.resumable,
        "message": info.message,
    })
}

/// Every option a driver can publish, filled in.
fn full_choices() -> ImageChoices {
    let mut choices = ImageChoices::default();
    choices.dpi = vec![100, 200, 300, 600];
    choices.dpi_y = vec![100, 200, 300, 600, 1200];
    choices.colors = vec![Color::Rgb, Color::Gray, Color::BlackAndWhite];
    choices.brightness = Some(ValueSpan::new(-1000, 1000, 1));
    choices.contrast = Some(ValueSpan::new(-500, 500, 10));
    choices.threshold = Some(ValueSpan::new(0, 255, 1));
    choices.rotation = vec![Rotation::Portrait, Rotation::Landscape, Rotation::UpsideDown, Rotation::Left];
    choices.formats = vec!["bmp".into(), "png".into()];
    choices.compression = vec!["none".into(), "jpeg".into()];
    choices.intents = vec![Intent::Text, Intent::MaximizeQuality];
    choices.page_sizes = vec![PageSize::A4, PageSize::Letter, PageSize::Auto];
    choices.auto_deskew = true;
    choices.orientation = vec![0, 1];
    choices.photometric = vec![0];
    choices.invert = true;
    choices.mirror = false;
    choices.lamp = true;
    choices
}

fn full_scanner() -> Scanner {
    let mut scanner = Scanner::new("wia:{6BDD1FC6}\\0000", "Office scanner");
    scanner.feeder = true;
    scanner.flatbed = true;
    scanner.duplex = true;
    scanner.feeder_ready = Some(true);
    scanner.handling_status = HandlingStatus::default().with_feeder_ready(Some(true)).with_jam(true).with_cover_open(true);
    scanner.feeder_capacity = Some(50);
    scanner.connection = Connection::Usb;
    scanner.port = Some("\\\\.\\Usbscan0".into());
    scanner.optical_x = Some(600);
    scanner.optical_y = Some(1200);
    scanner.pages = Some(ValueSpan::new(0, 50, 1));
    scanner.bed = Some(Area::new(8500, 11690));
    scanner.sheet = Some(Area::new(8500, 14000));
    scanner.glass = Some(full_choices());
    scanner.feeder_choices = Some(ImageChoices::default());
    scanner.reported.units = true;
    scanner.reported.glass_length = PageLengthSupport::new(Presence::Absent, Some(11690));
    scanner.reported.feeder_length = PageLengthSupport::new(Presence::Present, Some(14000));
    scanner
}

/// The derived scanner, as the CLI prints it: the handling flags as plain booleans, the conditions and the page
/// length support of each source added, and `reported` left out.
fn cli_scanner(scanner: &Scanner) -> Value {
    let mut v = serde_json::to_value(scanner).unwrap();
    v["handling_status"] = status_json(&scanner.handling_status);
    v["conditions"] = json!(scanner.handling_status.conditions());
    v["page_length"] = json!({ "glass": scanner.length_support(Source::Glass), "feeder": scanner.length_support(Source::Feeder) });
    v.as_object_mut().unwrap().remove("reported");
    v
}

#[test]
fn a_scanner_derives_what_the_cli_and_bindings_print() {
    for scanner in [full_scanner(), Scanner::new("wsd:http://192.0.2.10/scan", "Network scanner"), Scanner::default()] {
        assert_eq!(cli_scanner(&scanner), scanner_json(&scanner), "{}", scanner.name);
    }
    // What the derive writes on its own: the fields by their Rust names, the tri-state, what was reported.
    let v = serde_json::to_value(full_scanner()).unwrap();
    assert_eq!(v["bed"], json!({ "width": 8500, "height": 11690 }));
    assert_eq!(v["handling_status"]["feeder_ready"], json!(true));
    assert_eq!(v["reported"]["glass_length"], json!({ "device_detect": "absent", "max_length_mils": 11690 }));
    assert_eq!(v["glass"]["rotation"], json!(["portrait", "landscape", "180", "270"]));
    assert_eq!(serde_json::to_value(Scanner::default()).unwrap()["handling_status"]["feeder_ready"], Value::Null);
}

/// The derived length record with the lengths in inches and millimetres the CLI adds.
fn cli_length(record: &LengthRecord) -> Value {
    let mut v = serde_json::to_value(record).unwrap();
    v["length_inches"] = json!(record.length_inches().map(|inches| (inches * 1000.0).round() / 1000.0));
    v["length_mm"] = json!(record.length_mm().map(|mm| (mm * 10.0).round() / 10.0));
    v
}

#[test]
fn a_length_record_derives_what_the_cli_prints_and_reads_back() {
    let records = [
        LengthRecord::from_delivered(3300, 300),
        LengthRecord::with_source(4321, 300, LengthSource::Inferred),
        LengthRecord::with_source(0, 300, LengthSource::Size),
        LengthRecord::default(),
    ];
    for record in records {
        assert_eq!(cli_length(&record), scan_length_json(&record), "{record:?}");
        // The printed record reads back too: what the CLI added is ignored.
        assert_eq!(serde_json::from_value::<LengthRecord>(cli_length(&record)).unwrap(), record);
    }
}

/// The derived error record as the CLI prints it: the HRESULT in hex and its name, without the native code.
fn cli_error(info: &ScanErrorInfo) -> Value {
    let mut v = serde_json::to_value(info).unwrap();
    v["hresult"] = json!(info.hresult_hex());
    v["code_name"] = json!(info.code_name());
    v.as_object_mut().unwrap().remove("native");
    v
}

#[test]
fn an_error_record_derives_what_the_cli_prints_and_reads_back() {
    let jam = ScanError::Native {
        kind: InterruptKind::Jam,
        native: Some(NativeCode::hresult(0x8021_0002, Some("WIA_ERROR_PAPER_JAM"))),
        message: "The scanner has a paper jam.".into(),
    };
    let http = ScanError::Native { kind: InterruptKind::Other, native: Some(NativeCode::new("http", 503, None)), message: "busy".into() };
    for error in [jam, http, ScanError::Failed("no".into()), ScanError::Unsupported] {
        let info = ScanErrorInfo::new(&error, Default::default());
        assert_eq!(cli_error(&info), scan_error_info_json(&info), "{error}");
        assert_eq!(serde_json::from_value::<ScanErrorInfo>(serde_json::to_value(&info).unwrap()).unwrap(), info);
    }
}
