//! The values of the SANE network protocol: procedure numbers, status codes, option descriptors,
//! frame parameters, and how each is read from or written to the wire.

use std::fmt;
use std::io::Read;

use crate::error::{Error, Result};
use crate::wire::{Decoder, Encoder, text_of};

/// The major version of the SANE standard this client speaks.
pub const SANE_MAJOR: i32 = 1;
/// The network protocol revision, carried in the build field of the version code.
pub const NET_PROTOCOL_VERSION: i32 = 3;

/// A SANE version code: the major version in the top byte, the minor in the next, the build in the
/// low 16 bits.
pub const fn version_code(major: i32, minor: i32, build: i32) -> i32 {
    ((major & 0xff) << 24) | ((minor & 0xff) << 16) | (build & 0xffff)
}

/// `(major, minor, build)` of a version code.
pub const fn version_parts(code: i32) -> (i32, i32, i32) {
    ((code >> 24) & 0xff, (code >> 16) & 0xff, code & 0xffff)
}

/// The remote procedures, by the number that starts each request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Procedure {
    Init,
    GetDevices,
    Open,
    Close,
    GetOptionDescriptors,
    ControlOption,
    GetParameters,
    Start,
    Cancel,
    Authorize,
    Exit,
}

impl Procedure {
    /// The procedure number.
    pub const fn code(self) -> i32 {
        match self {
            Self::Init => 0,
            Self::GetDevices => 1,
            Self::Open => 2,
            Self::Close => 3,
            Self::GetOptionDescriptors => 4,
            Self::ControlOption => 5,
            Self::GetParameters => 6,
            Self::Start => 7,
            Self::Cancel => 8,
            Self::Authorize => 9,
            Self::Exit => 10,
        }
    }

    /// A short name for messages: `init`, `get devices`, `open`, ...
    pub const fn name(self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::GetDevices => "get devices",
            Self::Open => "open",
            Self::Close => "close",
            Self::GetOptionDescriptors => "get option descriptors",
            Self::ControlOption => "control option",
            Self::GetParameters => "get parameters",
            Self::Start => "start",
            Self::Cancel => "cancel",
            Self::Authorize => "authorize",
            Self::Exit => "exit",
        }
    }
}

/// A SANE status code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Status {
    Good,
    Unsupported,
    Cancelled,
    DeviceBusy,
    Inval,
    Eof,
    Jammed,
    NoDocs,
    CoverOpen,
    IoError,
    NoMem,
    AccessDenied,
    /// The lamp is not ready yet (a later addition to the standard's list).
    WarmingUp,
    /// The scanner is locked by a switch or another user (a later addition).
    HwLocked,
    /// A code this client does not know.
    Other(i32),
}

impl Status {
    /// The status a word carries.
    pub const fn from_word(word: i32) -> Self {
        match word {
            0 => Self::Good,
            1 => Self::Unsupported,
            2 => Self::Cancelled,
            3 => Self::DeviceBusy,
            4 => Self::Inval,
            5 => Self::Eof,
            6 => Self::Jammed,
            7 => Self::NoDocs,
            8 => Self::CoverOpen,
            9 => Self::IoError,
            10 => Self::NoMem,
            11 => Self::AccessDenied,
            12 => Self::WarmingUp,
            13 => Self::HwLocked,
            other => Self::Other(other),
        }
    }

    /// The word for this status.
    pub const fn code(self) -> i32 {
        match self {
            Self::Good => 0,
            Self::Unsupported => 1,
            Self::Cancelled => 2,
            Self::DeviceBusy => 3,
            Self::Inval => 4,
            Self::Eof => 5,
            Self::Jammed => 6,
            Self::NoDocs => 7,
            Self::CoverOpen => 8,
            Self::IoError => 9,
            Self::NoMem => 10,
            Self::AccessDenied => 11,
            Self::WarmingUp => 12,
            Self::HwLocked => 13,
            Self::Other(code) => code,
        }
    }

    /// The symbolic name, `SANE_STATUS_NO_DOCS` and so on; `SANE_STATUS_UNKNOWN` for a code this
    /// client does not know.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Good => "SANE_STATUS_GOOD",
            Self::Unsupported => "SANE_STATUS_UNSUPPORTED",
            Self::Cancelled => "SANE_STATUS_CANCELLED",
            Self::DeviceBusy => "SANE_STATUS_DEVICE_BUSY",
            Self::Inval => "SANE_STATUS_INVAL",
            Self::Eof => "SANE_STATUS_EOF",
            Self::Jammed => "SANE_STATUS_JAMMED",
            Self::NoDocs => "SANE_STATUS_NO_DOCS",
            Self::CoverOpen => "SANE_STATUS_COVER_OPEN",
            Self::IoError => "SANE_STATUS_IO_ERROR",
            Self::NoMem => "SANE_STATUS_NO_MEM",
            Self::AccessDenied => "SANE_STATUS_ACCESS_DENIED",
            Self::WarmingUp => "SANE_STATUS_WARMING_UP",
            Self::HwLocked => "SANE_STATUS_HW_LOCKED",
            Self::Other(_) => "SANE_STATUS_UNKNOWN",
        }
    }

    /// What the status means, in a few words.
    pub const fn meaning(self) -> &'static str {
        match self {
            Self::Good => "success",
            Self::Unsupported => "the operation is not supported",
            Self::Cancelled => "the operation was cancelled",
            Self::DeviceBusy => "the device is busy",
            Self::Inval => "the request or a value is not valid",
            Self::Eof => "no more data",
            Self::Jammed => "the document feeder is jammed",
            Self::NoDocs => "the document feeder is out of documents",
            Self::CoverOpen => "the scanner cover is open",
            Self::IoError => "an error in the device's input or output",
            Self::NoMem => "the daemon ran out of memory",
            Self::AccessDenied => "access to the resource was denied",
            Self::WarmingUp => "the lamp is warming up",
            Self::HwLocked => "the scanner is locked",
            Self::Other(_) => "a status this client does not know",
        }
    }

    /// Whether waiting alone may clear it: busy, warming up, locked.
    pub const fn is_transient(self) -> bool {
        matches!(self, Self::DeviceBusy | Self::WarmingUp | Self::HwLocked)
    }

    /// Whether the device certainly did nothing: it answered busy, warming up or locked.
    pub const fn did_nothing(self) -> bool {
        self.is_transient()
    }
}

impl fmt::Display for Status {
    /// `SANE_STATUS_NO_DOCS (the document feeder is out of documents)`; an unknown code shows its number.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Other(code) => write!(f, "SANE status {code} ({})", self.meaning()),
            _ => write!(f, "{} ({})", self.name(), self.meaning()),
        }
    }
}

/// The type of an option's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ValueType {
    Bool,
    Int,
    /// 16.16 fixed point.
    Fixed,
    String,
    Button,
    Group,
    Other(i32),
}

impl ValueType {
    /// The type a word carries.
    pub const fn from_word(word: i32) -> Self {
        match word {
            0 => Self::Bool,
            1 => Self::Int,
            2 => Self::Fixed,
            3 => Self::String,
            4 => Self::Button,
            5 => Self::Group,
            other => Self::Other(other),
        }
    }

    /// The word for this type.
    pub const fn code(self) -> i32 {
        match self {
            Self::Bool => 0,
            Self::Int => 1,
            Self::Fixed => 2,
            Self::String => 3,
            Self::Button => 4,
            Self::Group => 5,
            Self::Other(code) => code,
        }
    }

    /// Whether values of this type are words (bool, int, fixed).
    pub const fn is_word(self) -> bool {
        matches!(self, Self::Bool | Self::Int | Self::Fixed)
    }
}

/// The physical unit of an option's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Unit {
    None,
    Pixel,
    Bit,
    Mm,
    Dpi,
    Percent,
    Microsecond,
    Other(i32),
}

impl Unit {
    /// The unit a word carries.
    pub const fn from_word(word: i32) -> Self {
        match word {
            0 => Self::None,
            1 => Self::Pixel,
            2 => Self::Bit,
            3 => Self::Mm,
            4 => Self::Dpi,
            5 => Self::Percent,
            6 => Self::Microsecond,
            other => Self::Other(other),
        }
    }

    /// A short suffix for dumps: `px`, `mm`, `dpi`, `%`, ...
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::None | Self::Other(_) => "",
            Self::Pixel => "px",
            Self::Bit => "bit",
            Self::Mm => "mm",
            Self::Dpi => "dpi",
            Self::Percent => "%",
            Self::Microsecond => "us",
        }
    }
}

/// The capability bits of an option.
pub mod cap {
    /// Software can set it.
    pub const SOFT_SELECT: i32 = 1;
    /// It is set on the device (a switch).
    pub const HARD_SELECT: i32 = 2;
    /// Software can read it.
    pub const SOFT_DETECT: i32 = 4;
    /// The driver emulates it.
    pub const EMULATED: i32 = 8;
    /// The driver can choose the value itself.
    pub const AUTOMATIC: i32 = 16;
    /// It does not apply now (another option's value turns it off).
    pub const INACTIVE: i32 = 32;
    /// An option for experts.
    pub const ADVANCED: i32 = 64;
}

/// The bits of a control-option reply's `info` word.
pub mod info {
    /// The value set is not the value asked for (the driver rounded it).
    pub const INEXACT: i32 = 1;
    /// Other options changed: read the descriptors again.
    pub const RELOAD_OPTIONS: i32 = 2;
    /// The frame parameters changed.
    pub const RELOAD_PARAMS: i32 = 4;
}

/// A numeric range: `min..=max` in steps of `quant` (0 for any value).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Range {
    pub min: i32,
    pub max: i32,
    pub quant: i32,
}

impl Range {
    /// `min..=max` in steps of `quant`.
    pub const fn new(min: i32, max: i32, quant: i32) -> Self {
        Self { min, max, quant }
    }

    /// Whether `value` is in the range (the step is not checked: drivers round).
    pub const fn contains(self, value: i32) -> bool {
        self.min <= value && value <= self.max
    }
}

/// What values an option takes.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Constraint {
    None,
    Range(Range),
    WordList(Vec<i32>),
    StringList(Vec<String>),
}

/// One option of an open device.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OptionDescriptor {
    /// The well-known name (`resolution`, `mode`, `source`, `tl-x`, ...); empty for option 0 and groups.
    pub name: String,
    pub title: String,
    pub description: String,
    pub value_type: ValueType,
    pub unit: Unit,
    /// The size of the value in bytes: a multiple of four for words, the buffer size for a string.
    pub size: i32,
    /// [`cap`] bits.
    pub cap: i32,
    pub constraint: Constraint,
}

impl OptionDescriptor {
    /// A descriptor with no title, description, capabilities or constraint.
    pub fn new(name: impl Into<String>, value_type: ValueType, unit: Unit, size: i32) -> Self {
        Self { name: name.into(), title: String::new(), description: String::new(), value_type, unit, size, cap: 0, constraint: Constraint::None }
    }

    /// Sets [`Self::cap`].
    #[must_use]
    pub fn with_cap(mut self, cap: i32) -> Self {
        self.cap = cap;
        self
    }

    /// Sets [`Self::constraint`].
    #[must_use]
    pub fn with_constraint(mut self, constraint: Constraint) -> Self {
        self.constraint = constraint;
        self
    }

    /// Whether the option applies now.
    pub const fn is_active(&self) -> bool {
        self.cap & cap::INACTIVE == 0
    }

    /// Whether software can set it now.
    pub const fn is_settable(&self) -> bool {
        self.is_active() && self.cap & cap::SOFT_SELECT != 0
    }

    /// How many words a word value holds (`size / 4`, at least one).
    pub fn word_count(&self) -> usize {
        usize::try_from(self.size / 4).unwrap_or(0).max(1)
    }
}

/// The options of an open device, by index. Option 0 is the option count; an index the daemon sent
/// no descriptor for is `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct OptionTable {
    pub options: Vec<Option<OptionDescriptor>>,
}

impl OptionTable {
    /// A table of `options`, by index.
    pub fn new(options: Vec<Option<OptionDescriptor>>) -> Self {
        Self { options }
    }

    /// The first option named `name`, with its index.
    pub fn find(&self, name: &str) -> Option<(usize, &OptionDescriptor)> {
        self.options.iter().enumerate().find_map(|(i, d)| d.as_ref().filter(|d| d.name == name).map(|d| (i, d)))
    }

    /// The first option named `name` that applies now.
    pub fn active(&self, name: &str) -> Option<(usize, &OptionDescriptor)> {
        self.find(name).filter(|(_, d)| d.is_active())
    }

    /// Every option with its index.
    pub fn iter(&self) -> impl Iterator<Item = (usize, &OptionDescriptor)> {
        self.options.iter().enumerate().filter_map(|(i, d)| d.as_ref().map(|d| (i, d)))
    }
}

/// An option value as it travels.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Value {
    /// Bool, int or fixed: one word, or several for an array option.
    Words(Vec<i32>),
    /// A string.
    String(String),
    /// A button or group has no value.
    None,
}

impl Value {
    /// One word.
    pub fn word(word: i32) -> Self {
        Self::Words(vec![word])
    }

    /// The first word, for a word value.
    pub fn first_word(&self) -> Option<i32> {
        match self {
            Self::Words(words) => words.first().copied(),
            _ => None,
        }
    }

    /// The text, for a string value.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }
}

/// A 16.16 fixed-point value as a number.
pub fn fixed_to_f64(fixed: i32) -> f64 {
    f64::from(fixed) / 65536.0
}

/// A number as 16.16 fixed point, rounded and saturated.
pub fn f64_to_fixed(value: f64) -> i32 {
    let scaled = (value * 65536.0).round();
    if scaled >= f64::from(i32::MAX) {
        i32::MAX
    } else if scaled <= f64::from(i32::MIN) {
        i32::MIN
    } else {
        scaled as i32
    }
}

/// What one frame of a scan is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Frame {
    Gray,
    /// Red, green and blue interleaved in one frame.
    Rgb,
    /// One channel of a three-pass colour scan.
    Red,
    Green,
    Blue,
    Other(i32),
}

impl Frame {
    /// The frame a word carries.
    pub const fn from_word(word: i32) -> Self {
        match word {
            0 => Self::Gray,
            1 => Self::Rgb,
            2 => Self::Red,
            3 => Self::Green,
            4 => Self::Blue,
            other => Self::Other(other),
        }
    }
}

/// The parameters of the frame about to be (or being) scanned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Parameters {
    pub format: Frame,
    /// This frame completes the image (false for the first two of a three-pass scan).
    pub last_frame: bool,
    pub bytes_per_line: i32,
    pub pixels_per_line: i32,
    /// The number of lines, or -1 when the device does not know in advance (a hand scanner, a
    /// feeder that measures the sheet).
    pub lines: i32,
    /// Bits per sample: 1, 8 or 16.
    pub depth: i32,
}

impl Parameters {
    /// A frame of `format`, `pixels_per_line` wide, `lines` high at `depth` bits per sample, with
    /// `bytes_per_line` per row.
    pub const fn new(format: Frame, last_frame: bool, bytes_per_line: i32, pixels_per_line: i32, lines: i32, depth: i32) -> Self {
        Self { format, last_frame, bytes_per_line, pixels_per_line, lines, depth }
    }

    /// The bytes the frame holds when its line count is known.
    pub fn expected_bytes(&self) -> Option<u64> {
        let lines = u64::try_from(self.lines).ok()?;
        let per_line = u64::try_from(self.bytes_per_line).ok()?;
        lines.checked_mul(per_line)
    }
}

/// The byte order of 16-bit samples on the data connection, as the daemon declares it on start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    /// The order a start reply's word declares: `0x1234` little-endian, `0x4321` big-endian.
    pub fn from_word(word: i32) -> Result<Self> {
        match word {
            0x1234 => Ok(Self::Little),
            0x4321 => Ok(Self::Big),
            other => Err(Error::Protocol(format!("{other:#x} is not a SANE byte order"))),
        }
    }
}

/// One device a daemon offers.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Device {
    /// The name to open it by, unique on the daemon (`driver:bus:...`).
    pub name: String,
    pub vendor: String,
    pub model: String,
    /// `flatbed scanner`, `sheetfed scanner`, `virtual device`, ...
    pub kind: String,
}

impl Device {
    /// A device named `name`.
    pub fn new(name: impl Into<String>, vendor: impl Into<String>, model: impl Into<String>, kind: impl Into<String>) -> Self {
        Self { name: name.into(), vendor: vendor.into(), model: model.into(), kind: kind.into() }
    }

    /// `vendor model`, or the device name when both are empty.
    pub fn display_name(&self) -> String {
        let label = format!("{} {}", self.vendor.trim(), self.model.trim()).trim().to_owned();
        if label.is_empty() { self.name.clone() } else { label }
    }
}

/// A status word; anything but success is an error of `operation`.
pub(crate) fn read_status<R: Read>(d: &mut Decoder<R>) -> Result<Status> {
    Ok(Status::from_word(d.word()?))
}

/// The device list of a get-devices reply: a counted array of pointers to devices, which ends with a
/// null pointer.
pub(crate) fn read_devices<R: Read>(d: &mut Decoder<R>) -> Result<Vec<Device>> {
    let count = d.count("the device list")?;
    let mut devices = Vec::with_capacity(count);
    for _ in 0..count {
        if !d.pointer()? {
            continue;
        }
        let name = d.text()?;
        let vendor = d.text()?;
        let model = d.text()?;
        let kind = d.text()?;
        devices.push(Device { name, vendor, model, kind });
    }
    Ok(devices)
}

/// A constraint of type `kind`.
fn read_constraint<R: Read>(d: &mut Decoder<R>, kind: i32) -> Result<Constraint> {
    match kind {
        0 => Ok(Constraint::None),
        1 => {
            if !d.pointer()? {
                return Ok(Constraint::None);
            }
            let (min, max, quant) = (d.word()?, d.word()?, d.word()?);
            Ok(Constraint::Range(Range { min, max, quant }))
        }
        // The list's first element is its own length, as in the C structure.
        2 => {
            let mut words = d.words("a word list")?;
            if words.is_empty() {
                return Ok(Constraint::WordList(words));
            }
            let declared = usize::try_from(words.remove(0)).unwrap_or(usize::MAX);
            words.truncate(declared);
            Ok(Constraint::WordList(words))
        }
        // A counted array of strings, the last of which is the null string.
        3 => {
            let count = d.count("a string list")?;
            let mut strings = Vec::with_capacity(count);
            for _ in 0..count {
                if let Some(text) = d.string()? {
                    strings.push(text);
                }
            }
            Ok(Constraint::StringList(strings))
        }
        other => Err(Error::Protocol(format!("{other} is not a SANE constraint type"))),
    }
}

/// The reply of get-option-descriptors: a counted array of pointers to descriptors.
pub(crate) fn read_option_table<R: Read>(d: &mut Decoder<R>) -> Result<OptionTable> {
    let count = d.count("the option descriptors")?;
    let mut options = Vec::with_capacity(count);
    for _ in 0..count {
        if !d.pointer()? {
            options.push(None);
            continue;
        }
        let name = d.text()?;
        let title = d.text()?;
        let description = d.text()?;
        let value_type = ValueType::from_word(d.word()?);
        let unit = Unit::from_word(d.word()?);
        let size = d.word()?;
        let cap = d.word()?;
        let kind = d.word()?;
        let constraint = read_constraint(d, kind)?;
        options.push(Some(OptionDescriptor { name, title, description, value_type, unit, size, cap, constraint }));
    }
    Ok(OptionTable { options })
}

/// An option value of `value_type` and `size` bytes, as an array on the wire.
pub(crate) fn write_value(e: &mut Encoder, value_type: ValueType, size: i32, value: &Value) -> Result<()> {
    let size_bytes = usize::try_from(size).map_err(|_| Error::Invalid(format!("an option size of {size}")))?;
    match (value_type, value) {
        (ValueType::Bool | ValueType::Int | ValueType::Fixed, Value::Words(words)) => {
            if words.len() * 4 != size_bytes {
                return Err(Error::Invalid(format!("the option holds {} words, not {}", size_bytes / 4, words.len())));
            }
            e.words(words)?;
        }
        (ValueType::String, Value::String(text)) => {
            if text.contains('\0') {
                return Err(Error::Invalid("an option string cannot hold a NUL character".into()));
            }
            if text.len() >= size_bytes {
                return Err(Error::Invalid(format!("{text:?} is longer than the option's {} characters", size_bytes.saturating_sub(1))));
            }
            let mut buffer = text.as_bytes().to_vec();
            buffer.resize(size_bytes, 0);
            e.chars(&buffer)?;
        }
        (ValueType::Button | ValueType::Group, Value::None) => {
            e.word(0);
        }
        (value_type, value) => return Err(Error::Invalid(format!("a {value:?} cannot be written to an option of type {value_type:?}"))),
    }
    Ok(())
}

/// An option value of `value_type` from the wire.
pub(crate) fn read_value<R: Read>(d: &mut Decoder<R>, value_type: ValueType) -> Result<Value> {
    match value_type {
        ValueType::String => {
            let chars = d.chars()?;
            let end = chars.iter().position(|&b| b == 0).unwrap_or(chars.len());
            Ok(Value::String(text_of(&chars[..end])))
        }
        ValueType::Bool | ValueType::Int | ValueType::Fixed => Ok(Value::Words(d.words("an option value")?)),
        _ => {
            // A button or group carries an empty array; anything else in it is skipped.
            let count = d.count("an option value")?;
            for _ in 0..count {
                d.word()?;
            }
            Ok(Value::None)
        }
    }
}

/// The parameters of a get-parameters reply.
pub(crate) fn read_parameters<R: Read>(d: &mut Decoder<R>) -> Result<Parameters> {
    let format = Frame::from_word(d.word()?);
    let last_frame = d.word()? != 0;
    let bytes_per_line = d.word()?;
    let pixels_per_line = d.word()?;
    let lines = d.word()?;
    let depth = d.word()?;
    Ok(Parameters { format, last_frame, bytes_per_line, pixels_per_line, lines, depth })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Limits;

    #[test]
    fn version_codes() {
        let code = version_code(SANE_MAJOR, 0, NET_PROTOCOL_VERSION);
        assert_eq!(code, 0x0100_0003);
        assert_eq!(version_parts(code), (1, 0, 3));
    }

    #[test]
    fn statuses_round_trip() {
        for word in 0..16 {
            assert_eq!(Status::from_word(word).code(), word);
        }
        assert_eq!(Status::NoDocs.code(), 7);
        assert_eq!(Status::Jammed.name(), "SANE_STATUS_JAMMED");
        assert_eq!(Status::Other(99).to_string(), "SANE status 99 (a status this client does not know)");
    }

    #[test]
    fn a_descriptor_table_reads() {
        let mut e = Encoder::new();
        e.word(3);
        // Option 0: the count.
        e.word(1);
        e.string(Some("")).unwrap().string(Some("Number of options")).unwrap().string(Some("")).unwrap();
        e.word(1).word(0).word(4).word(cap::SOFT_DETECT).word(0);
        // A null descriptor.
        e.word(0);
        // resolution, a word list 75, 150, 300.
        e.word(1);
        e.string(Some("resolution")).unwrap().string(Some("Resolution")).unwrap().string(None).unwrap();
        e.word(1).word(4).word(4).word(cap::SOFT_SELECT | cap::SOFT_DETECT).word(2);
        e.words(&[3, 75, 150, 300]).unwrap();
        let bytes = e.into_bytes();
        let table = read_option_table(&mut Decoder::new(&bytes[..], Limits::default())).unwrap();
        assert_eq!(table.options.len(), 3);
        assert!(table.options[1].is_none());
        let (index, resolution) = table.find("resolution").unwrap();
        assert_eq!(index, 2);
        assert_eq!(resolution.constraint, Constraint::WordList(vec![75, 150, 300]));
        assert_eq!(resolution.unit, Unit::Dpi);
        assert!(resolution.is_settable());
    }

    #[test]
    fn values_are_checked_before_sending() {
        let mut e = Encoder::new();
        assert!(write_value(&mut e, ValueType::Int, 4, &Value::Words(vec![1, 2])).is_err());
        assert!(write_value(&mut e, ValueType::String, 4, &Value::String("Color".into())).is_err());
        assert!(write_value(&mut e, ValueType::Int, 4, &Value::String("x".into())).is_err());
        write_value(&mut e, ValueType::String, 8, &Value::String("Gray".into())).unwrap();
        assert_eq!(e.as_bytes(), &[0, 0, 0, 8, b'G', b'r', b'a', b'y', 0, 0, 0, 0]);
    }

    #[test]
    fn fixed_point() {
        assert_eq!(f64_to_fixed(215.9), 14_149_222);
        assert!((fixed_to_f64(f64_to_fixed(215.9)) - 215.9).abs() < 1e-4);
        assert_eq!(f64_to_fixed(1e12), i32::MAX);
    }
}
