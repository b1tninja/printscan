//! The well-known SANE options in the scan core's terms.
//!
//! | Option | Read as | Written from |
//! |---|---|---|
//! | `source` (string list) | glass, feeder and duplex entries | `ScanSettings::source`, `duplex` |
//! | `mode` (string list) | `Lineart`, `Gray`, `Color` and their usual spellings | `ScanSettings::color` |
//! | `resolution`, `x-resolution`, `y-resolution` | dpi lists | `dpi`, `dpi_y` |
//! | `tl-x`, `tl-y`, `br-x`, `br-y` (mm or pixels) | the bed's size | `region` |
//! | `brightness`, `contrast`, `threshold` | spans | the same settings |
//!
//! SANE options are one set for the device, whatever the source, and other options' constraints
//! may change when the source does. Choices are read for the source the device is set to.

use printscan::scan::{Area, Color, ImageChoices, Region, ScanError, Source, ValueSpan};

use crate::protocol::{Constraint, OptionDescriptor, OptionTable, Unit, Value, ValueType, f64_to_fixed, fixed_to_f64};

/// Thousandths of an inch per millimetre.
const MILS_PER_MM: f64 = 1000.0 / 25.4;

/// What a `source` entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceKind {
    Glass,
    Feeder,
    Duplex,
    /// A film adapter, a negative holder: not offered.
    Other,
}

/// Classify one entry of the `source` list by its usual spellings (`Flatbed`, `ADF`, `ADF Duplex`,
/// `Automatic Document Feeder`, ...).
pub(crate) fn source_kind(entry: &str) -> SourceKind {
    let lower = entry.to_ascii_lowercase();
    if lower.contains("duplex") {
        SourceKind::Duplex
    } else if lower.contains("adf") || lower.contains("feeder") || lower.contains("document") {
        SourceKind::Feeder
    } else if lower.contains("flatbed") || lower.contains("platen") || lower.contains("glass") || lower == "normal" {
        SourceKind::Glass
    } else {
        SourceKind::Other
    }
}

/// The entries of a string-list option.
fn strings(desc: &OptionDescriptor) -> &[String] {
    match &desc.constraint {
        Constraint::StringList(list) => list,
        _ => &[],
    }
}

/// What the device can feed from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sources {
    pub glass: bool,
    pub feeder: bool,
    pub duplex: bool,
}

/// The sources of a device, from its `source` list; without one, from its type (a `sheetfed`
/// scanner feeds, anything else is a flatbed).
pub(crate) fn sources(table: &OptionTable, device_kind: &str) -> Sources {
    let Some((_, desc)) = table.active("source") else {
        let sheetfed = device_kind.to_ascii_lowercase().contains("sheetfed");
        return Sources { glass: !sheetfed, feeder: sheetfed, duplex: false };
    };
    let mut found = Sources::default();
    for entry in strings(desc) {
        match source_kind(entry) {
            SourceKind::Glass => found.glass = true,
            SourceKind::Feeder => found.feeder = true,
            SourceKind::Duplex => {
                found.feeder = true;
                found.duplex = true;
            }
            SourceKind::Other => {}
        }
    }
    found
}

/// The `source` entry for `source` (never `Auto`), or `None` when the device has no `source` option.
/// One-sided feeding prefers a plain feeder entry over a duplex one.
pub(crate) fn source_value(table: &OptionTable, source: Source, duplex: bool) -> Result<Option<String>, ScanError> {
    let Some((_, desc)) = table.active("source") else {
        if duplex {
            return Err(ScanError::Failed("this scanner has no two-sided source".into()));
        }
        return Ok(None);
    };
    let list = strings(desc);
    let wanted: &[SourceKind] = match (source, duplex) {
        (_, true) => &[SourceKind::Duplex],
        (Source::Glass, false) => &[SourceKind::Glass],
        _ => &[SourceKind::Feeder, SourceKind::Duplex],
    };
    for kind in wanted {
        if let Some(entry) = list.iter().find(|e| source_kind(e) == *kind) {
            return Ok(Some(entry.clone()));
        }
    }
    let named = if duplex {
        "two-sided feeder"
    } else if source == Source::Glass {
        "flatbed"
    } else {
        "document feeder"
    };
    Err(ScanError::Failed(format!("this scanner has no {named} source (it lists {})", list.join(", "))))
}

/// The colour a `mode` entry gives, by its usual spellings.
fn mode_color(entry: &str) -> Option<Color> {
    let lower = entry.to_ascii_lowercase();
    if lower.contains("lineart") || lower.contains("binary") || lower == "black & white" || lower == "black and white" || lower == "bw" {
        Some(Color::BlackAndWhite)
    } else if lower.contains("gray") || lower.contains("grey") {
        Some(Color::Gray)
    } else if lower.contains("color") || lower.contains("colour") || lower == "rgb" {
        Some(Color::Rgb)
    } else {
        None
    }
}

/// The modes as colours, in the device's order, each once.
fn colors(desc: &OptionDescriptor) -> Vec<Color> {
    let mut out = Vec::new();
    for color in strings(desc).iter().filter_map(|e| mode_color(e)) {
        if !out.contains(&color) {
            out.push(color);
        }
    }
    out
}

/// The `mode` entry for `color`. Black and white falls back to grey (thresholded in software)
/// when the device has no 1-bit mode. `None` when the device has no `mode` option.
pub(crate) fn mode_value(table: &OptionTable, color: Color) -> Result<Option<String>, ScanError> {
    let Some((_, desc)) = table.active("mode") else { return Ok(None) };
    let list = strings(desc);
    // The standard spelling first, then any other that means the same.
    let standard = |c: Color| match c {
        Color::BlackAndWhite => "Lineart",
        Color::Rgb => "Color",
        _ => "Gray",
    };
    let pick = |c: Color| list.iter().find(|e| e.as_str() == standard(c)).or_else(|| list.iter().find(|e| mode_color(e) == Some(c)));
    if let Some(entry) = pick(color) {
        return Ok(Some(entry.clone()));
    }
    if color == Color::BlackAndWhite
        && let Some(entry) = pick(Color::Gray)
    {
        return Ok(Some(entry.clone()));
    }
    Err(ScanError::Failed(format!("this scanner has no {} mode (it lists {})", color.name(), list.join(", "))))
}

/// A word option's value as a whole number (a fixed value rounded).
pub(crate) fn as_number(desc: &OptionDescriptor, word: i32) -> i32 {
    match desc.value_type {
        ValueType::Fixed => fixed_to_f64(word).round() as i32,
        _ => word,
    }
}

/// `number` as the option's word.
fn word_of(desc: &OptionDescriptor, number: f64) -> i32 {
    match desc.value_type {
        ValueType::Fixed => f64_to_fixed(number),
        _ => number.round() as i32,
    }
}

/// The resolutions a resolution option offers. A range is offered as the usual steps inside it
/// (and its ends when none is).
pub(crate) fn dpi_choices(desc: &OptionDescriptor) -> Vec<u32> {
    const USUAL: [u32; 9] = [75, 100, 150, 200, 300, 400, 600, 1200, 2400];
    match &desc.constraint {
        Constraint::WordList(words) => {
            let mut dpi: Vec<u32> = words.iter().filter_map(|&w| u32::try_from(as_number(desc, w)).ok()).filter(|&d| d > 0).collect();
            dpi.sort_unstable();
            dpi.dedup();
            dpi
        }
        Constraint::Range(range) => {
            let (min, max) = (as_number(desc, range.min), as_number(desc, range.max));
            let quant = as_number(desc, range.quant);
            let fits = |d: i32| d >= min && d <= max && (quant <= 0 || (d - min) % quant == 0);
            let dpi: Vec<u32> = USUAL.iter().copied().filter(|&d| fits(d as i32)).collect();
            if !dpi.is_empty() {
                return dpi;
            }
            [min, max].iter().filter_map(|&d| u32::try_from(d).ok()).filter(|&d| d > 0).collect()
        }
        _ => Vec::new(),
    }
}

/// The value that asks `desc` (a resolution option) for `dpi`, or an error naming what it takes.
pub(crate) fn resolution_value(desc: &OptionDescriptor, dpi: u32) -> Result<Value, ScanError> {
    let wanted = i32::try_from(dpi).unwrap_or(i32::MAX);
    let accepted = match &desc.constraint {
        Constraint::WordList(words) => words.iter().any(|&w| as_number(desc, w) == wanted),
        Constraint::Range(range) => (as_number(desc, range.min)..=as_number(desc, range.max)).contains(&wanted),
        _ => true,
    };
    if !accepted {
        let listed = dpi_choices(desc).iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
        return Err(ScanError::Failed(format!("this scanner does not scan at {dpi} dpi (it offers {listed})")));
    }
    Ok(Value::word(word_of(desc, f64::from(dpi))))
}

/// A numeric option as a span, rounded to whole numbers.
fn span(desc: &OptionDescriptor) -> Option<ValueSpan> {
    match &desc.constraint {
        Constraint::Range(range) => Some(ValueSpan::new(as_number(desc, range.min), as_number(desc, range.max), as_number(desc, range.quant).max(1))),
        _ => None,
    }
}

/// The value for a brightness, contrast or threshold `number` inside `desc`'s range.
pub(crate) fn level_value(desc: &OptionDescriptor, name: &str, number: i32) -> Result<Value, ScanError> {
    if let Some(span) = span(desc)
        && !(span.min..=span.max).contains(&number)
    {
        return Err(ScanError::Failed(format!("{name} is {} to {} on this scanner", span.min, span.max)));
    }
    Ok(Value::word(word_of(desc, f64::from(number))))
}

/// A scan-area option's maximum, in thousandths of an inch (millimetre options only).
fn extent_mils(desc: &OptionDescriptor) -> Option<u32> {
    let Constraint::Range(range) = &desc.constraint else { return None };
    if desc.unit != Unit::Mm {
        return None;
    }
    let mm = match desc.value_type {
        ValueType::Fixed => fixed_to_f64(range.max),
        _ => f64::from(range.max),
    };
    (mm > 0.0).then(|| (mm * MILS_PER_MM).round() as u32)
}

/// The size of the scan area, from the ranges of `br-x` and `br-y`.
pub(crate) fn bed(table: &OptionTable) -> Option<Area> {
    let width = extent_mils(table.active("br-x")?.1)?;
    let height = extent_mils(table.active("br-y")?.1)?;
    Some(Area::new(width, height))
}

/// What the device offers, in the scan core's terms.
pub(crate) fn choices(table: &OptionTable) -> ImageChoices {
    let mut choices = ImageChoices::default();
    let resolution = table.active("resolution").or_else(|| table.active("x-resolution"));
    if let Some((_, desc)) = resolution {
        choices.dpi = dpi_choices(desc);
    }
    if let Some((_, desc)) = table.active("y-resolution") {
        choices.dpi_y = dpi_choices(desc);
    }
    if let Some((_, desc)) = table.active("mode") {
        choices.colors = colors(desc);
    }
    choices.brightness = table.active("brightness").and_then(|(_, d)| span(d));
    choices.contrast = table.active("contrast").and_then(|(_, d)| span(d));
    choices.threshold = table.active("threshold").and_then(|(_, d)| span(d));
    choices.formats = vec!["png".into()];
    choices
}

/// The four scan-area writes for `region` (thousandths of an inch) at `dpi`: `(name, value)` in
/// the order `tl-x`, `tl-y`, `br-x`, `br-y`. Millimetre options get millimetres, pixel options
/// pixels at the scan's resolution.
pub(crate) fn area_values(table: &OptionTable, region: Region, dpi: (u32, u32)) -> Result<Vec<(&'static str, Value)>, ScanError> {
    let region = printscan::scan::fit_region(region, bed(table))?;
    let edges = [
        ("tl-x", region.left, dpi.0),
        ("tl-y", region.top, dpi.1),
        ("br-x", region.left.saturating_add(region.width), dpi.0),
        ("br-y", region.top.saturating_add(region.height), dpi.1),
    ];
    let mut out = Vec::with_capacity(4);
    for (name, mils, axis_dpi) in edges {
        let Some((_, desc)) = table.active(name) else {
            return Err(ScanError::Failed(format!("this scanner has no {name} option, so a scan region cannot be set")));
        };
        let number = match desc.unit {
            Unit::Mm => f64::from(mils) / MILS_PER_MM,
            Unit::Pixel => f64::from(printscan::scan::pixels_for(mils, axis_dpi)),
            other => return Err(ScanError::Failed(format!("the {name} option is in {other:?}, which this backend cannot convert"))),
        };
        let mut word = word_of(desc, number);
        // Keep inside the device's range: rounding can step just past the edge.
        if let Constraint::Range(range) = &desc.constraint {
            word = word.clamp(range.min, range.max);
        }
        out.push((name, Value::word(word)));
    }
    Ok(out)
}

/// One line per option, for a diagnostic dump: `--resolution 75|150|300 dpi [settable]`.
pub(crate) fn describe(desc: &OptionDescriptor) -> String {
    let constraint = match &desc.constraint {
        Constraint::None => String::new(),
        Constraint::Range(r) if desc.value_type == ValueType::Fixed => {
            format!(" {}..{} (step {})", fixed_to_f64(r.min), fixed_to_f64(r.max), fixed_to_f64(r.quant))
        }
        Constraint::Range(r) => format!(" {}..{} (step {})", r.min, r.max, r.quant),
        Constraint::WordList(words) if desc.value_type == ValueType::Fixed => {
            format!(" {}", words.iter().map(|&w| fixed_to_f64(w).to_string()).collect::<Vec<_>>().join("|"))
        }
        Constraint::WordList(words) => format!(" {}", words.iter().map(i32::to_string).collect::<Vec<_>>().join("|")),
        Constraint::StringList(list) => format!(" {}", list.join("|")),
    };
    let mut flags = Vec::new();
    if !desc.is_active() {
        flags.push("inactive");
    }
    if desc.is_settable() {
        flags.push("settable");
    }
    if desc.cap & crate::protocol::cap::ADVANCED != 0 {
        flags.push("advanced");
    }
    let name = if desc.name.is_empty() { desc.title.as_str() } else { desc.name.as_str() };
    format!("--{name} {:?}{constraint} {} [{}]", desc.value_type, desc.unit.suffix(), flags.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Range, cap};

    fn list(name: &str, entries: &[&str]) -> OptionDescriptor {
        OptionDescriptor::new(name, ValueType::String, Unit::None, 32)
            .with_cap(cap::SOFT_SELECT)
            .with_constraint(Constraint::StringList(entries.iter().map(|e| (*e).to_owned()).collect()))
    }

    fn mm(name: &str, max: f64) -> OptionDescriptor {
        OptionDescriptor::new(name, ValueType::Fixed, Unit::Mm, 4).with_cap(cap::SOFT_SELECT).with_constraint(Constraint::Range(Range::new(
            0,
            f64_to_fixed(max),
            0,
        )))
    }

    fn table(options: Vec<OptionDescriptor>) -> OptionTable {
        OptionTable::new(options.into_iter().map(Some).collect())
    }

    #[test]
    fn sources_by_their_usual_names() {
        let t = table(vec![list("source", &["Flatbed", "ADF", "ADF Duplex", "Transparency Adapter"])]);
        assert_eq!(sources(&t, "flatbed scanner"), Sources { glass: true, feeder: true, duplex: true });
        assert_eq!(source_value(&t, Source::Glass, false).unwrap().as_deref(), Some("Flatbed"));
        assert_eq!(source_value(&t, Source::Feeder, false).unwrap().as_deref(), Some("ADF"));
        assert_eq!(source_value(&t, Source::Feeder, true).unwrap().as_deref(), Some("ADF Duplex"));
        let t = table(vec![list("source", &["Automatic Document Feeder"])]);
        assert!(source_value(&t, Source::Glass, false).is_err());
        assert_eq!(sources(&table(vec![]), "sheetfed scanner"), Sources { glass: false, feeder: true, duplex: false });
        assert_eq!(source_value(&table(vec![]), Source::Glass, false).unwrap(), None);
        assert!(source_value(&table(vec![]), Source::Feeder, true).is_err());
    }

    #[test]
    fn modes_by_their_usual_names() {
        let t = table(vec![list("mode", &["Lineart", "Halftone", "Gray", "Color"])]);
        assert_eq!(choices(&t).colors, vec![Color::BlackAndWhite, Color::Gray, Color::Rgb]);
        assert_eq!(mode_value(&t, Color::Rgb).unwrap().as_deref(), Some("Color"));
        assert_eq!(mode_value(&t, Color::BlackAndWhite).unwrap().as_deref(), Some("Lineart"));
        let t = table(vec![list("mode", &["True Gray", "24bit Color"])]);
        assert_eq!(mode_value(&t, Color::Gray).unwrap().as_deref(), Some("True Gray"));
        // No 1-bit mode: grey, thresholded in software.
        assert_eq!(mode_value(&t, Color::BlackAndWhite).unwrap().as_deref(), Some("True Gray"));
        let t = table(vec![list("mode", &["Color"])]);
        assert!(mode_value(&t, Color::Gray).is_err());
    }

    #[test]
    fn resolutions_from_lists_and_ranges() {
        let listed = OptionDescriptor::new("resolution", ValueType::Int, Unit::Dpi, 4).with_constraint(Constraint::WordList(vec![300, 75, 150]));
        assert_eq!(dpi_choices(&listed), vec![75, 150, 300]);
        assert_eq!(resolution_value(&listed, 150).unwrap(), Value::word(150));
        assert!(resolution_value(&listed, 200).unwrap_err().to_string().contains("75, 150, 300"));
        let ranged = OptionDescriptor::new("resolution", ValueType::Fixed, Unit::Dpi, 4).with_constraint(Constraint::Range(Range::new(
            f64_to_fixed(50.0),
            f64_to_fixed(600.0),
            f64_to_fixed(50.0),
        )));
        assert_eq!(dpi_choices(&ranged), vec![100, 150, 200, 300, 400, 600]);
        assert_eq!(resolution_value(&ranged, 300).unwrap(), Value::word(f64_to_fixed(300.0)));
        assert!(resolution_value(&ranged, 1200).is_err());
    }

    #[test]
    fn the_area_in_millimetres() {
        let t = table(vec![mm("tl-x", 215.9), mm("tl-y", 297.0), mm("br-x", 215.9), mm("br-y", 297.0)]);
        assert_eq!(bed(&t), Some(Area::new(8500, 11693)));
        // One inch in from the top left, two by three inches.
        let writes = area_values(&t, Region::new(1000, 1000, 2000, 3000), (300, 300)).unwrap();
        let mm_of = |v: &Value| fixed_to_f64(v.first_word().unwrap());
        assert_eq!(writes.iter().map(|(n, _)| *n).collect::<Vec<_>>(), ["tl-x", "tl-y", "br-x", "br-y"]);
        assert!((mm_of(&writes[0].1) - 25.4).abs() < 0.01);
        assert!((mm_of(&writes[2].1) - 76.2).abs() < 0.01);
        assert!((mm_of(&writes[3].1) - 101.6).abs() < 0.01);
        // A region past the bed is cut to it.
        let writes = area_values(&t, Region::new(0, 0, 20_000, 20_000), (300, 300)).unwrap();
        assert!((mm_of(&writes[2].1) - 215.9).abs() < 0.01);
        assert!(area_values(&table(vec![]), Region::new(0, 0, 10, 10), (300, 300)).is_err());
    }

    #[test]
    fn levels_are_checked_against_their_range() {
        let b = OptionDescriptor::new("brightness", ValueType::Int, Unit::None, 4).with_constraint(Constraint::Range(Range::new(-100, 100, 1)));
        assert_eq!(level_value(&b, "brightness", 20).unwrap(), Value::word(20));
        assert!(level_value(&b, "brightness", 500).is_err());
        let t = table(vec![b]);
        assert_eq!(choices(&t).brightness, Some(ValueSpan::new(-100, 100, 1)));
    }
}
