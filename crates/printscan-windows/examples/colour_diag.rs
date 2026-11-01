//! A colour diagnostic page, and printing it through a Windows driver.
//!
//! ```text
//! cargo run -p printscan-windows --example colour_diag -- render <out.bmp> [label]
//! cargo run -p printscan-windows --example colour_diag -- print "<printer>" <driver|none|default> [label]
//! ```
//!
//! The page (7.5 x 10 in at 300 dpi inside a 0.5 in margin of a Letter sheet, sRGB, no profile) has:
//! - patches: primaries, secondaries, saturated greens, skin tones, pastels and deep colours;
//! - ramps: 32 grey steps, near-white (230 to 255) and near-black (0 to 25) in steps of 1;
//! - chroma detail: stripes and checkers of colour pairs with equal luma at 1, 2, 4 and 8 pixels,
//!   and text in one colour of such a pair on the other. A path that subsamples chroma (JPEG
//!   4:2:0) turns the fine stripes into a flat colour and the text into nothing while the coarse
//!   ones survive; a path that keeps chroma prints all of them;
//! - black corner squares (for scanning the sheet back) and a label naming the job.
//!
//! `print` sends it with winprint's `ImagePrinter` at 300 dpi. `driver` and `none` merge
//! `psk:PageColorManagement` (`Driver`: the driver converts the colours; `None`: the application
//! already did) when the driver lists it; `default` sends the driver's default ticket.

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("render") => {
            let out = args.get(1).ok_or("render needs an output path")?;
            let label = args.get(2).map_or("PRINTSCAN COLOUR DIAG", String::as_str);
            write_bmp(std::path::Path::new(out), &page(label))?;
            println!("wrote {out}");
            Ok(())
        }
        Some("print") => {
            let printer = args.get(1).ok_or("print needs a printer name")?;
            let mode = args.get(2).map_or("default", String::as_str);
            let label = args.get(3).cloned().unwrap_or_else(|| format!("CM {}", mode.to_uppercase()));
            print(printer, mode, &label)
        }
        _ => {
            eprintln!("usage: colour_diag render <out.bmp> [label] | print <printer> <driver|none|default> [label]");
            std::process::exit(2);
        }
    }
}

#[cfg(not(windows))]
fn main() {}

/// An RGB image, rows top to bottom.
struct Image {
    width: usize,
    height: usize,
    rgb: Vec<u8>,
}

impl Image {
    fn new(width: usize, height: usize) -> Self {
        Self { width, height, rgb: vec![255; width * height * 3] }
    }

    fn set(&mut self, x: usize, y: usize, c: [u8; 3]) {
        if x < self.width && y < self.height {
            let at = (y * self.width + x) * 3;
            self.rgb[at..at + 3].copy_from_slice(&c);
        }
    }

    fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, c: [u8; 3]) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.set(xx, yy, c);
            }
        }
    }

    /// A rectangle filled by `f(dx, dy)`.
    fn paint(&mut self, x: usize, y: usize, w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 3]) {
        for dy in 0..h {
            for dx in 0..w {
                self.set(x + dx, y + dy, f(dx, dy));
            }
        }
    }

    /// `text` in the 5x7 block font, each font pixel `scale` image pixels, one pixel gap.
    fn text(&mut self, x: usize, y: usize, scale: usize, text: &str, c: [u8; 3]) {
        for (i, ch) in text.chars().enumerate() {
            let rows = glyph(ch);
            for (row, bits) in rows.iter().enumerate() {
                for col in 0..5 {
                    if bits & (0x10 >> col) != 0 {
                        self.fill(x + (i * 6 + col) * scale, y + row * scale, scale, scale, c);
                    }
                }
            }
        }
    }
}

/// 5x7 glyphs, one byte per row, bit 4 the left column.
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        ':' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        _ => [0; 7],
    }
}

const DPI: usize = 300;
const BLACK: [u8; 3] = [0, 0, 0];

/// Patches with their labels.
const PATCHES: [([u8; 3], &str); 30] = [
    ([255, 0, 0], "R"),
    ([0, 255, 0], "G"),
    ([0, 0, 255], "B"),
    ([0, 255, 255], "C"),
    ([255, 0, 255], "M"),
    ([255, 255, 0], "Y"),
    ([0, 128, 0], "G128"),
    ([40, 180, 60], "G180"),
    ([128, 255, 128], "LTG"),
    ([0, 100, 60], "DKTEAL"),
    ([154, 205, 50], "YLWGRN"),
    ([0, 200, 140], "SEAGRN"),
    ([224, 172, 140], "SKIN1"),
    ([198, 134, 66], "SKIN2"),
    ([141, 85, 36], "SKIN3"),
    ([255, 219, 172], "SKIN4"),
    ([135, 206, 235], "SKY"),
    ([255, 165, 0], "ORANGE"),
    ([75, 0, 130], "INDIGO"),
    ([128, 0, 32], "BURGUN"),
    ([20, 30, 90], "NAVY"),
    ([255, 105, 180], "PINK"),
    ([250, 240, 230], "LINEN"),
    ([240, 248, 255], "ALICE"),
    ([128, 128, 128], "GRY128"),
    ([64, 64, 64], "GRY64"),
    ([192, 192, 192], "GRY192"),
    ([16, 16, 16], "GRY16"),
    ([128, 128, 0], "OLIVE"),
    ([0, 128, 128], "TEAL"),
];

/// Pairs with (nearly) equal luma and different chroma: subsampling chroma loses the difference.
const PAIRS: [([u8; 3], [u8; 3], &str); 3] =
    [([255, 0, 255], [0, 180, 0], "MAGENTA/GREEN"), ([255, 0, 0], [0, 128, 128], "RED/TEAL"), ([0, 0, 255], [75, 75, 75], "BLUE/GREY")];

/// The page on a full Letter sheet (8.5 x 11 in), the content inside a 0.5 in margin: the image is
/// printed from the corner of the paper, so content at its edge would fall in the printer's
/// unprintable border.
fn page(label: &str) -> Image {
    let content = content(label);
    let margin = DPI / 2;
    let mut sheet = Image::new(DPI * 17 / 2, DPI * 11);
    for y in 0..content.height {
        let from = y * content.width * 3;
        let to = ((y + margin) * sheet.width + margin) * 3;
        sheet.rgb[to..to + content.width * 3].copy_from_slice(&content.rgb[from..from + content.width * 3]);
    }
    sheet
}

/// The diagnostic content, 7.5 x 10 in.
fn content(label: &str) -> Image {
    let (w, h) = (DPI * 15 / 2, DPI * 10);
    let mut img = Image::new(w, h);
    // Corner squares for registration.
    let mark = 60;
    for (x, y) in [(0, 0), (w - mark, 0), (0, h - mark), (w - mark, h - mark)] {
        img.fill(x, y, mark, mark, BLACK);
    }
    img.text(110, 20, 6, label, BLACK);
    img.text(110, 80, 3, "PRINTSCAN COLOUR DIAG  300 DPI  SRGB  NO PROFILE", BLACK);

    // Patches: 6 columns x 5 rows.
    let (pw, ph, x0, y0) = (330, 170, 90, 140);
    for (i, (c, name)) in PATCHES.iter().enumerate() {
        let (col, row) = (i % 6, i / 6);
        let (x, y) = (x0 + col * (pw + 25), y0 + row * (ph + 45));
        img.fill(x, y, pw, ph, *c);
        img.text(x, y + ph + 8, 3, name, BLACK);
    }

    // Ramps.
    let ry = y0 + 5 * (ph + 45) + 20;
    img.text(90, ry, 3, "GREY 32 STEPS", BLACK);
    for s in 0..32usize {
        let v = (s * 255 / 31) as u8;
        img.fill(90 + s * 65, ry + 30, 65, 110, [v, v, v]);
    }
    let ry = ry + 170;
    img.text(90, ry, 3, "NEAR WHITE 230-255", BLACK);
    for s in 0..26usize {
        let v = 230 + s as u8;
        img.fill(90 + s * 80, ry + 30, 80, 90, [v, v, v]);
    }
    img.fill(90, ry + 30, 26 * 80, 2, BLACK);
    let ry = ry + 150;
    img.text(90, ry, 3, "NEAR BLACK 0-25", BLACK);
    for s in 0..26usize {
        let v = s as u8;
        img.fill(90 + s * 80, ry + 30, 80, 90, [v, v, v]);
    }

    // Chroma detail: for each pair, stripes and checkers at 1, 2, 4 and 8 pixels, then text.
    let cy = ry + 160;
    for (p, (a, b, name)) in PAIRS.iter().enumerate() {
        let y = cy + p * 300;
        img.text(90, y, 3, name, BLACK);
        for (k, size) in [1usize, 2, 4, 8].iter().enumerate() {
            let x = 90 + k * 280;
            img.paint(x, y + 30, 120, 110, |dx, _| if (dx / size) % 2 == 0 { *a } else { *b });
            img.paint(x + 130, y + 30, 120, 110, |dx, dy| if (dx / size + dy / size) % 2 == 0 { *a } else { *b });
            img.text(x, y + 150, 3, &format!("{size}PX"), BLACK);
        }
        // Text of one colour on the other, at three sizes.
        let tx = 90 + 4 * 280;
        img.fill(tx, y + 30, 900, 200, *b);
        img.text(tx + 15, y + 40, 1, "CHROMA TEXT 1PX ABCDEFGHIJKLMNOPQRSTUVWXYZ", *a);
        img.text(tx + 15, y + 60, 2, "CHROMA TEXT 2PX ABCDEFGHIJKL", *a);
        img.text(tx + 15, y + 90, 4, "CHROMA 4PX", *a);
        img.text(tx + 15, y + 140, 7, "CHROMA", *a);
    }
    img
}

/// A 24-bit BMP at 300 dpi.
fn write_bmp(path: &std::path::Path, img: &Image) -> std::io::Result<()> {
    let (w, h) = (img.width as u32, img.height as u32);
    let stride = (w * 3).div_ceil(4) * 4;
    let size = 54 + stride * h;
    let ppm = (DPI as f64 / 0.0254).round() as u32;
    let mut b = Vec::with_capacity(size as usize);
    b.extend_from_slice(b"BM");
    for v in [size, 0, 54, 40] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&(w as i32).to_le_bytes());
    b.extend_from_slice(&(h as i32).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    for v in [0u32, stride * h, ppm, ppm, 0, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    for y in (0..img.height).rev() {
        let row = &img.rgb[y * img.width * 3..(y + 1) * img.width * 3];
        for p in row.as_chunks::<3>().0 {
            b.extend_from_slice(&[p[2], p[1], p[0]]);
        }
        b.resize(b.len() + (stride - w * 3) as usize, 0);
    }
    std::fs::write(path, b)
}

#[cfg(windows)]
fn print(printer: &str, mode: &str, label: &str) -> Result<(), Box<dyn std::error::Error>> {
    use winprint::printer::{FilePrinter as _, ImagePrinter, PrinterDevice};
    use winprint::ticket::{PrintCapabilities, PrintTicket, PrintTicketBuilder};

    let device = PrinterDevice::by_os_name(printer)?;
    let dir = std::env::temp_dir().join(format!("printscan-colour-diag-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let bmp = dir.join("page.bmp");
    write_bmp(&bmp, &page(label))?;

    let mut builder = PrintTicketBuilder::new(&device)?;
    if mode != "default" {
        let option = if mode == "none" { "None" } else { "Driver" };
        let caps = String::from_utf8(PrintCapabilities::fetch_xml(&device)?)?;
        if !caps.contains(&format!("psk:{option}\"")) || !caps.contains("psk:PageColorManagement") {
            return Err(format!("{printer} does not list PageColorManagement {option}").into());
        }
        let delta = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<psf:PrintTicket xmlns:psf="http://schemas.microsoft.com/windows/2003/08/printing/printschemaframework" xmlns:psk="http://schemas.microsoft.com/windows/2003/08/printing/printschemakeywords" version="1">
  <psf:Feature name="psk:PageColorManagement"><psf:Option name="psk:{option}"/></psf:Feature>
</psf:PrintTicket>"#
        );
        builder.merge(PrintTicket::from_xml(delta.into_bytes()))?;
    }
    let ticket = builder.build()?;
    let xml = String::from_utf8_lossy(ticket.get_xml()).into_owned();
    let kept = xml
        .find("psk:PageColorManagement")
        .map(|at| xml[at..].split("<psf:Option name=\"").nth(1).and_then(|o| o.split('"').next()).unwrap_or("?").to_owned());
    println!("{printer}: label {label:?}, PageColorManagement in the validated ticket: {kept:?}");
    let outcome = ImagePrinter::new(device).print(&bmp, ticket)?;
    println!("outcome: {outcome:?}");
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
