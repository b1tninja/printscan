//! Paper sizes: the PWG 5101.1 self-describing name a job asks for ([`crate::print::JobOptions::with_media`]), the
//! size, the name a person reads, and the PPD keyword a CUPS queue knows the size by.
//!
//! An app lays its pages out on a [`Media`] and asks for [`Media::pwg`]; each backend turns that into its own
//! ticket. [`parse_pwg`] reads the size out of any self-describing name, so a size missing from [`CATALOGUE`]
//! still has dimensions.
//!
//! [`Media::name`] is stable: an app may save it (a preference) and find the size again with [`by_name`].

/// One paper size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Media {
    /// The English name a person reads (`"US Letter"`, `"ARCH D (24 x 36 in)"`). Stable, and unique in
    /// [`CATALOGUE`].
    pub name: &'static str,
    /// The PWG 5101.1 self-describing name (`"na_letter_8.5x11in"`, `"iso_a4_210x297mm"`).
    pub pwg: &'static str,
    /// Width and height in hundredths of a millimetre (the unit of IPP `media-size`), portrait.
    pub size: (u32, u32),
    /// The PPD keyword CUPS drivers use for the size (`"Letter"`, `"A4"`, `"w288h432"`), when the common
    /// sizes have one; `None` where the PWG name is what a queue matches.
    pub ppd: Option<&'static str>,
}

impl Media {
    /// The size in PostScript points (1/72 in), as a PDF page is laid out: from the dimensions the PWG name
    /// states, so an inch size is a whole number of points (US #10 is 297 x 684, where [`Media::size`] rounds
    /// 4.125 in to 104.78 mm).
    pub fn points(&self) -> (f64, f64) {
        match stated(self.pwg) {
            Some((w, h, unit)) => (w * 72.0 / unit.inches(), h * 72.0 / unit.inches()),
            None => {
                let pt = |hmm: u32| f64::from(hmm) * 72.0 / 2540.0;
                (pt(self.size.0), pt(self.size.1))
            }
        }
    }

    /// The `media` value for a CUPS queue: the PPD keyword when the size has one, else the PWG name (CUPS
    /// feeds the PPD size of the same dimensions whatever the driver calls it).
    pub fn cups_media(&self) -> &'static str {
        self.ppd.unwrap_or(self.pwg)
    }
}

/// `width x height` in inches, in hundredths of a millimetre.
const fn inches(width_thousandths: u32, height_thousandths: u32) -> (u32, u32) {
    // 1 in = 2540 hundredths of a millimetre; sizes are given in thousandths of an inch to stay in integers.
    ((width_thousandths * 254 + 50) / 100, (height_thousandths * 254 + 50) / 100)
}

/// `width x height` in millimetres, in hundredths of a millimetre.
const fn mm(width: u32, height: u32) -> (u32, u32) {
    (width * 100, height * 100)
}

/// The sizes apps offer, in the order a paper list shows them.
pub const CATALOGUE: &[Media] = &[
    Media { name: "US Letter", pwg: "na_letter_8.5x11in", size: inches(8500, 11000), ppd: Some("Letter") },
    Media { name: "US Legal", pwg: "na_legal_8.5x14in", size: inches(8500, 14000), ppd: Some("Legal") },
    Media { name: "Tabloid", pwg: "na_ledger_11x17in", size: inches(11000, 17000), ppd: Some("Tabloid") },
    Media { name: "A3", pwg: "iso_a3_297x420mm", size: mm(297, 420), ppd: Some("A3") },
    Media { name: "A4", pwg: "iso_a4_210x297mm", size: mm(210, 297), ppd: Some("A4") },
    Media { name: "A5", pwg: "iso_a5_148x210mm", size: mm(148, 210), ppd: Some("A5") },
    Media { name: "A6", pwg: "iso_a6_105x148mm", size: mm(105, 148), ppd: None },
    Media { name: "ARCH A (9 x 12 in)", pwg: "na_arch-a_9x12in", size: inches(9000, 12000), ppd: None },
    Media { name: "ARCH B (12 x 18 in)", pwg: "na_arch-b_12x18in", size: inches(12000, 18000), ppd: None },
    Media { name: "ARCH C (18 x 24 in)", pwg: "na_arch-c_18x24in", size: inches(18000, 24000), ppd: None },
    Media { name: "ARCH D (24 x 36 in)", pwg: "na_arch-d_24x36in", size: inches(24000, 36000), ppd: None },
    // PWG 5101.1 has ARCH E1 as its `wide-format` size.
    Media { name: "ARCH E1 (30 x 42 in)", pwg: "na_wide-format_30x42in", size: inches(30000, 42000), ppd: None },
    Media { name: "ARCH E (36 x 48 in)", pwg: "na_arch-e_36x48in", size: inches(36000, 48000), ppd: None },
    Media { name: "ANSI C (17 x 22 in)", pwg: "na_c_17x22in", size: inches(17000, 22000), ppd: None },
    Media { name: "ANSI D (22 x 34 in)", pwg: "na_d_22x34in", size: inches(22000, 34000), ppd: None },
    Media { name: "ANSI E (34 x 44 in)", pwg: "na_e_34x44in", size: inches(34000, 44000), ppd: None },
    Media { name: "A2", pwg: "iso_a2_420x594mm", size: mm(420, 594), ppd: None },
    Media { name: "A1", pwg: "iso_a1_594x841mm", size: mm(594, 841), ppd: None },
    Media { name: "A0", pwg: "iso_a0_841x1189mm", size: mm(841, 1189), ppd: None },
    // The usual 4 x 6 in shipping label; `w288h432` is the keyword label printers' PPDs use.
    Media { name: "4 x 6 in label (101.6 x 152.4 mm)", pwg: "na_index-4x6_4x6in", size: inches(4000, 6000), ppd: Some("w288h432") },
    Media { name: "US #10 Envelope (4.125 x 9.5 in)", pwg: "na_number-10_4.125x9.5in", size: inches(4125, 9500), ppd: Some("Env10") },
];

/// The catalogue entry called `name` ([`Media::name`]).
pub fn by_name(name: &str) -> Option<&'static Media> {
    CATALOGUE.iter().find(|m| m.name == name)
}

/// The catalogue entry for the PWG name `pwg` (ASCII case is ignored).
pub fn by_pwg(pwg: &str) -> Option<&'static Media> {
    CATALOGUE.iter().find(|m| m.pwg.eq_ignore_ascii_case(pwg))
}

/// The catalogue entry a PPD keyword stands for (`"Letter"`, `"w288h432"`), or that has `keyword` as its PWG name.
pub fn by_ppd(keyword: &str) -> Option<&'static Media> {
    CATALOGUE.iter().find(|m| m.ppd == Some(keyword)).or_else(|| by_pwg(keyword))
}

/// The catalogue entry of `size` (hundredths of a millimetre, either orientation), within `tolerance`
/// hundredths of a millimetre on each side.
pub fn by_size(size: (u32, u32), tolerance: u32) -> Option<&'static Media> {
    let near = |a: u32, b: u32| a.abs_diff(b) <= tolerance;
    let (w, h) = (size.0.min(size.1), size.0.max(size.1));
    CATALOGUE.iter().find(|m| near(m.size.0, w) && near(m.size.1, h))
}

/// The width and height (hundredths of a millimetre) a PWG 5101.1 self-describing name states:
/// `class_name_WxHunit` with `unit` `mm` or `in` (`iso_a4_210x297mm`, `na_number-10_4.125x9.5in`,
/// `custom_57x31mm`). `None` for anything else, a zero or a size over 10 m.
pub fn parse_pwg(pwg: &str) -> Option<(u32, u32)> {
    let (w, h, unit) = stated(pwg)?;
    let hmm = |side: f64| (side * unit.hmm()).round() as u32;
    Some((hmm(w), hmm(h)))
}

/// The unit a PWG self-describing name states its size in.
#[derive(Clone, Copy)]
enum Unit {
    Mm,
    In,
}

impl Unit {
    /// Hundredths of a millimetre in one unit.
    fn hmm(self) -> f64 {
        match self {
            Self::Mm => 100.0,
            Self::In => 2540.0,
        }
    }

    /// Units in an inch.
    fn inches(self) -> f64 {
        match self {
            Self::Mm => 25.4,
            Self::In => 1.0,
        }
    }
}

/// The width, height and unit a PWG self-describing name states, as written (see [`parse_pwg`] for what is
/// accepted).
fn stated(pwg: &str) -> Option<(f64, f64, Unit)> {
    let dims = pwg.rsplit('_').next()?;
    let (numbers, unit) = match dims.strip_suffix("mm") {
        Some(n) => (n, Unit::Mm),
        None => (dims.strip_suffix("in")?, Unit::In),
    };
    let (w, h) = numbers.split_once('x')?;
    let side = |text: &str| -> Option<f64> {
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return None;
        }
        let value = text.parse::<f64>().ok()?;
        (1.0..=1_000_000.0).contains(&(value * unit.hmm())).then_some(value)
    };
    Some((side(w)?, side(h)?, unit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_has_the_size_its_pwg_name_states_and_names_are_unique() {
        for (i, m) in CATALOGUE.iter().enumerate() {
            assert_eq!(parse_pwg(m.pwg), Some(m.size), "{}", m.name);
            assert!(m.size.0 <= m.size.1, "{} is portrait", m.name);
            assert!(CATALOGUE[..i].iter().all(|o| o.name != m.name && o.pwg != m.pwg), "{} is listed once", m.name);
            assert_eq!(by_name(m.name), Some(m));
            assert_eq!(by_pwg(&m.pwg.to_ascii_uppercase()), Some(m));
            assert_eq!(by_ppd(m.cups_media()), Some(m));
            assert_eq!(by_size((m.size.1, m.size.0), 0), Some(m), "landscape finds {}", m.name);
        }
    }

    #[test]
    fn sizes_in_points_are_the_ones_pdf_pages_are_laid_out_on() {
        // The page sizes the apps laid out on, in points.
        let laid_out = [
            ("US Letter", (612.0, 792.0)),
            ("A4", (595.28, 841.89)),
            ("A6", (297.64, 419.53)),
            ("ARCH E1 (30 x 42 in)", (2160.0, 3024.0)),
            ("A0", (2383.94, 3370.39)),
            ("4 x 6 in label (101.6 x 152.4 mm)", (288.0, 432.0)),
            ("US #10 Envelope (4.125 x 9.5 in)", (297.0, 684.0)),
        ];
        for (name, (w, h)) in laid_out {
            let (pw, ph) = by_name(name).unwrap().points();
            // Metric sizes were laid out to the hundredth of a point; inch sizes are exact.
            assert!((pw - w).abs() < 0.005 && (ph - h).abs() < 0.005, "{name}: {pw} x {ph}");
        }
        assert_eq!(by_name("US #10 Envelope (4.125 x 9.5 in)").unwrap().points(), (297.0, 684.0), "not 104.78 mm");
        assert_eq!(by_name("ARCH D (24 x 36 in)").unwrap().points(), (1728.0, 2592.0));
    }

    #[test]
    fn a_self_describing_name_outside_the_catalogue_still_has_a_size() {
        assert_eq!(parse_pwg("custom_57x31mm"), Some((5700, 3100)));
        assert_eq!(parse_pwg("na_foolscap_8.5x13in"), Some((21590, 33020)));
        assert_eq!(by_pwg("custom_57x31mm"), None);
        for bad in ["", "A4", "iso_a4_210x297", "iso_a4_x297mm", "iso_a4_0x297mm", "iso_a4_-1x297mm", "iso_a4_1e3x297mm", "x_99999999x1mm"] {
            assert_eq!(parse_pwg(bad), None, "{bad}");
        }
    }

    #[test]
    fn cups_gets_the_ppd_keyword_where_there_is_one_and_the_pwg_name_otherwise() {
        assert_eq!(by_name("US Letter").unwrap().cups_media(), "Letter");
        assert_eq!(by_name("4 x 6 in label (101.6 x 152.4 mm)").unwrap().cups_media(), "w288h432");
        assert_eq!(by_name("A6").unwrap().cups_media(), "iso_a6_105x148mm");
        assert_eq!(by_name("ARCH E1 (30 x 42 in)").unwrap().cups_media(), "na_wide-format_30x42in");
    }
}
