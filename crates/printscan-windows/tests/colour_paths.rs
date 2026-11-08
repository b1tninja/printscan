//! Does the XPS/Direct2D print path hand an image's RGB values to the driver unchanged? Prints one
//! strip of colour patches to a temporary PWG raster printer (the in-box Microsoft PWG Raster Class
//! Driver on a file port: a lossless raster) with winprint's `ImagePrinter`, once with the default
//! ticket and once with a ticket asking for no colour management (`psk:PageColorManagement` =
//! `psk:None`, which this driver drops), decodes the raster and compares the patches.
//!
//! Measured 2026-11-01 on Windows 11: exact (largest channel error 0). A GDI printer with ICM off
//! measured 2 on the same driver (HALFTONE resampling); see `docs/colour.md`. With
//! `PRINTSCAN_COLOUR_OUT` set, the rasters are kept there.
//!
//! The spooler backend is measured the same way with application-managed colour: the patches as a PDF
//! of DeviceRGB fills, through `WindowsSpooler` to the PWG raster printer (which lists no colour
//! management: the job prints with a warning), and to a temporary printer on the in-box PostScript
//! driver `MS Publisher Color Printer`, whose ticket provider lists `PageColorManagement` and
//! `PageICMRenderingIntent` from the DEVMODE: the ticket gets them and the driver keeps them.
//! Run by hand: `cargo test -p printscan-windows --test colour_paths -- --ignored --nocapture`.

#![cfg(all(windows, feature = "print"))]

use std::path::Path;
use std::time::Duration;

use printscan::print::backends::{Backend as _, Document, JobHint, SubmitRequest};
use printscan::print::{Job, JobOptions, RenderingIntent, Scaling};
use printscan_windows::spooler::WindowsSpooler;
use winprint::printer::{FilePrinter as _, ImagePrinter, PrintOutcome};
use winprint::test_utils::file_device::{FilePrinterDevice, PublisherColor, PwgRaster};
use winprint::ticket::{PrintTicket, PrintTicketBuilder};

/// The patches: primaries, secondaries, a saturated mid green, a skin tone, mid grey and a dark blue.
pub const PATCHES: [[u8; 3]; 10] = [
    [255, 0, 0],
    [0, 255, 0],
    [0, 0, 255],
    [0, 255, 255],
    [255, 0, 255],
    [255, 255, 0],
    [40, 180, 60],
    [224, 172, 140],
    [128, 128, 128],
    [20, 30, 90],
];
const SIDE: u32 = 48;

fn strip() -> (u32, u32, Vec<u8>) {
    let (w, h) = (SIDE * PATCHES.len() as u32, SIDE);
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for _ in 0..h {
        for x in 0..w {
            data.extend_from_slice(&PATCHES[(x / SIDE) as usize]);
        }
    }
    (w, h, data)
}

/// A 24-bit BMP of the strip, for the printers that take a file.
fn write_bmp(path: &Path, w: u32, h: u32, rgb: &[u8]) {
    let stride = (w * 3).div_ceil(4) * 4;
    let size = 54 + stride * h;
    let mut b = Vec::with_capacity(size as usize);
    b.extend_from_slice(b"BM");
    for v in [size, 0, 54, 40] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&(w as i32).to_le_bytes());
    b.extend_from_slice(&(h as i32).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    for v in [0u32, stride * h, 11811, 11811, 0, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    for y in (0..h).rev() {
        let row = &rgb[(y * w * 3) as usize..((y + 1) * w * 3) as usize];
        for p in row.as_chunks::<3>().0 {
            b.extend_from_slice(&[p[2], p[1], p[0]]);
        }
        b.resize(b.len() + (stride - w * 3) as usize, 0);
    }
    std::fs::write(path, b).unwrap();
}

fn no_colour_management(device: &FilePrinterDevice<PwgRaster>) -> PrintTicket {
    let delta = r#"<?xml version="1.0" encoding="UTF-8"?>
<psf:PrintTicket xmlns:psf="http://schemas.microsoft.com/windows/2003/08/printing/printschemaframework" xmlns:psk="http://schemas.microsoft.com/windows/2003/08/printing/printschemakeywords" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema" version="1">
  <psf:Feature name="psk:PageColorManagement"><psf:Option name="psk:None"/></psf:Feature>
</psf:PrintTicket>"#;
    let mut builder = PrintTicketBuilder::new(device.device()).unwrap();
    builder.merge(PrintTicket::from_xml(delta.as_bytes().to_vec())).unwrap();
    let ticket = builder.build().unwrap();
    let xml = String::from_utf8_lossy(ticket.get_xml()).into_owned();
    println!("PageColorManagement kept in the validated ticket: {}", xml.contains("PageColorManagement"));
    ticket
}

/// The first page of a PWG raster (PWG 5102.4) with 8-bit RGB pixels: width, height and RGB rows.
fn decode_pwg(data: &[u8]) -> (usize, usize, Vec<u8>) {
    assert_eq!(&data[..4], b"RaS2");
    let header = &data[4..4 + 1796];
    let field = |at: usize| u32::from_be_bytes(header[at..at + 4].try_into().unwrap()) as usize;
    let (width, height, bits_per_pixel, bytes_per_line) = (field(372), field(376), field(388), field(392));
    assert_eq!(bits_per_pixel, 24, "8-bit RGB pages");
    let mut pos = 4 + 1796;
    let mut pixels = Vec::with_capacity(bytes_per_line * height);
    while pixels.len() < bytes_per_line * height {
        let repeat = usize::from(data[pos]) + 1;
        pos += 1;
        let mut line = Vec::with_capacity(bytes_per_line);
        while line.len() < bytes_per_line {
            let control = data[pos];
            pos += 1;
            match control {
                128 => line.resize(bytes_per_line, 255),
                0..=127 => {
                    let pixel = &data[pos..pos + 3];
                    pos += 3;
                    for _ in 0..=control {
                        line.extend_from_slice(pixel);
                    }
                }
                _ => {
                    let n = (257 - usize::from(control)) * 3;
                    line.extend_from_slice(&data[pos..pos + n]);
                    pos += n;
                }
            }
        }
        for _ in 0..repeat {
            pixels.extend_from_slice(&line);
        }
    }
    pixels.truncate(bytes_per_line * height);
    (width, height, pixels)
}

/// The largest channel error over the patches, each sampled 7x7 at its centre in the inked box.
fn patch_error(width: usize, height: usize, rgb: &[u8]) -> u8 {
    let at = |x: usize, y: usize| &rgb[(y * width + x) * 3..(y * width + x) * 3 + 3];
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..height {
        for x in 0..width {
            if at(x, y).iter().any(|&v| v < 245) {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    assert!(x1 > x0, "the page has the strip");
    let pitch = (x1 - x0 + 1) as f64 / PATCHES.len() as f64;
    let cy = (y0 + y1) / 2;
    let mut worst = 0;
    for (i, want) in PATCHES.iter().enumerate() {
        let cx = (x0 as f64 + pitch * (i as f64 + 0.5)) as usize;
        let mut sum = [0u32; 3];
        for y in cy - 3..=cy + 3 {
            for x in cx - 3..=cx + 3 {
                for (s, v) in sum.iter_mut().zip(at(x, y)) {
                    *s += u32::from(*v);
                }
            }
        }
        for (s, w) in sum.iter().zip(want) {
            worst = worst.max(((f64::from(*s) / 49.0).round() as i32 - i32::from(*w)).unsigned_abs() as u8);
        }
    }
    worst
}

/// Wait for the job, keep the raster when asked, and return the largest patch error.
fn finish(device: FilePrinterDevice<PwgRaster>, keep: Option<&Path>, name: &str) -> u8 {
    device.wait_for_idle(Duration::from_secs(120)).unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let data = std::fs::read(device.file_path()).unwrap();
    if let Some(keep) = keep {
        std::fs::write(keep.join(format!("{name}.pwg")), &data).unwrap();
    }
    let (width, height, rgb) = decode_pwg(&data);
    let error = patch_error(width, height, &rgb);
    println!("{name}: largest channel error {error}");
    error
}

#[test]
#[ignore]
fn colour_paths() {
    let keep = std::env::var_os("PRINTSCAN_COLOUR_OUT").map(std::path::PathBuf::from);
    let out = keep.clone().unwrap_or_else(|| std::env::temp_dir().join(format!("printscan-colour-{}", std::process::id())));
    std::fs::create_dir_all(&out).unwrap();
    let keep = keep.as_deref();
    let (w, h, rgb) = strip();
    let bmp = out.join("strip.bmp");
    write_bmp(&bmp, w, h, &rgb);

    let device = FilePrinterDevice::<PwgRaster>::new().unwrap();
    assert_eq!(ImagePrinter::new(device.device().clone()).print(&bmp, PrintTicket::default()).unwrap(), PrintOutcome::Completed);
    let image_default = finish(device, keep, "image-default");

    let device = FilePrinterDevice::<PwgRaster>::new().unwrap();
    let ticket = no_colour_management(&device);
    assert_eq!(ImagePrinter::new(device.device().clone()).print(&bmp, ticket).unwrap(), PrintOutcome::Completed);
    let image_no_cm = finish(device, keep, "image-no-cm");

    // The XPS path hands the pixels over exactly.
    assert_eq!((image_default, image_no_cm), (0, 0));
}

/// An A4 PDF with the patches as DeviceRGB fills, side by side across the page.
fn patches_pdf() -> Vec<u8> {
    let mut content = String::new();
    for (i, [r, g, b]) in PATCHES.iter().enumerate() {
        let channel = |v: &u8| f64::from(*v) / 255.0;
        content.push_str(&format!("{:.6} {:.6} {:.6} rg {} 600 48 48 re f\n", channel(r), channel(g), channel(b), 57 + 48 * i));
    }
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 3 0 R >>".to_owned(),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    pdf
}

/// A job for the printer `name` asking for application-managed colour, converted with the relative intent.
fn application_managed<'a>(pdf: &'a [u8], name: &str) -> SubmitRequest<'a> {
    let job = Job::default().with_printer(name.to_owned()).with_title("printscan colour test").with_scaling(Scaling::ShrinkToFit);
    let options = JobOptions::new().with_application_managed_colour(true).with_rendering_intent(RenderingIntent::Relative);
    SubmitRequest::new(Document::pdf(pdf), job).with_options(options)
}

#[test]
#[ignore]
fn application_managed_colour_through_the_spooler_keeps_the_values() {
    let keep = std::env::var_os("PRINTSCAN_COLOUR_OUT").map(std::path::PathBuf::from);
    let pdf = patches_pdf();
    let device = FilePrinterDevice::<PwgRaster>::new().unwrap();
    let details = WindowsSpooler::default().printer_details(device.device().name()).unwrap();
    let honoured = details.honoured.unwrap();
    println!("PWG raster honours application-managed colour: {}", honoured.contains(&JobHint::ApplicationManagedColour));
    let submitted = WindowsSpooler::default().submit(application_managed(&pdf, device.device().name())).unwrap();
    println!("warnings: {:?}", submitted.warnings);
    // This driver has no colour-management setting: the job prints and says so.
    assert!(!honoured.contains(&JobHint::ApplicationManagedColour));
    assert!(submitted.warnings.iter().any(|w| w.contains("no colour-management setting")), "{:?}", submitted.warnings);
    assert!(submitted.warnings.iter().any(|w| w.contains("relative colorimetric")), "{:?}", submitted.warnings);
    assert_eq!(finish(device, keep.as_deref(), "spooler-application-managed"), 0);
}

#[test]
#[ignore]
fn application_managed_colour_reaches_a_driver_that_offers_it() {
    let pdf = patches_pdf();
    let device = FilePrinterDevice::<PublisherColor>::new().expect("add a temporary printer on MS Publisher Color Printer");
    let details = WindowsSpooler::default().printer_details(device.device().name()).unwrap();
    let honoured = details.honoured.unwrap();
    assert!(honoured.contains(&JobHint::ApplicationManagedColour), "{honoured:?}");
    assert!(honoured.contains(&JobHint::RenderingIntent), "{honoured:?}");
    // The validated ticket kept PageColorManagement None and the intent, or the job would be refused or warned about.
    let submitted = WindowsSpooler::default().submit(application_managed(&pdf, device.device().name())).unwrap();
    println!("warnings: {:?}", submitted.warnings);
    assert!(
        !submitted.warnings.iter().any(|w| w.contains("colour-management setting") || w.contains("rendering intent")),
        "{:?}",
        submitted.warnings
    );
    device.wait_for_idle(Duration::from_secs(120)).unwrap();
}
