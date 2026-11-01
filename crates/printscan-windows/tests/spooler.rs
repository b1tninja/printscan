//! The spooler backend against a temporary print-to-file printer (the in-box Microsoft Print To PDF
//! driver on a file port), so nothing reaches a real printer. Needs Windows and the right to add a
//! printer; the printer is removed afterwards.

#![cfg(all(windows, feature = "print"))]

use std::time::Duration;

use printscan::print::backends::{Backend, Document, ErrorKind, SubmitRequest};
use printscan::print::{CancelToken, Job, JobOptions};
use printscan_windows::spooler::WindowsSpooler;
use winprint::test_utils::file_device::{FilePrinterDevice, Pdf};

/// A one-page PDF (A4) with a filled square, its cross-reference table at the right offsets.
fn one_page_pdf() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R >>",
        "<< /Length 31 >>\nstream\n0 0 1 rg 100 600 200 150 re f\nendstream",
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

fn job_for(device: &FilePrinterDevice<Pdf>) -> Job {
    Job { printer: Some(device.device().name().to_owned()), title: "printscan spooler test".into(), ..Job::default() }
}

#[test]
fn a_pdf_reaches_the_printer_with_the_settings_it_offers() {
    let device = FilePrinterDevice::<Pdf>::new().expect("add a temporary print-to-file printer");
    let details = WindowsSpooler.printer_details(device.device().name()).unwrap();
    assert!(!details.paper.is_empty(), "the driver lists paper");
    assert!(!details.resolutions.is_empty(), "the driver lists resolutions");

    let pdf = one_page_pdf();
    let options = JobOptions::new().with_media("iso_a4_210x297mm");
    let request = SubmitRequest::new(Document::pdf(&pdf), job_for(&device)).with_options(options);
    let submitted = WindowsSpooler.submit(request).unwrap();
    assert!(submitted.job.is_none(), "the spooler gives no job handle");
    assert!(submitted.warnings.is_empty(), "A4 is offered: {:?}", submitted.warnings);

    device.wait_for_idle(Duration::from_secs(120)).unwrap();
    // The queue can be idle a moment before the driver has finished writing the file.
    let started = std::time::Instant::now();
    let written = loop {
        let written = std::fs::read(device.file_path()).unwrap_or_default();
        if written.ends_with(b"%%EOF")
            || written.ends_with(
                b"%%EOF
",
            )
            || written.ends_with(
                b"%%EOF
",
            )
            || started.elapsed() > Duration::from_secs(60)
        {
            break written;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(written.starts_with(b"%PDF"), "the driver wrote a PDF ({} bytes)", written.len());
}

#[test]
fn paper_the_driver_does_not_have_is_a_warning_not_an_error() {
    let device = FilePrinterDevice::<Pdf>::new().expect("add a temporary print-to-file printer");
    let pdf = one_page_pdf();
    let options = JobOptions::new().with_media("custom_57x31mm");
    let submitted = WindowsSpooler.submit(SubmitRequest::new(Document::pdf(&pdf), job_for(&device)).with_options(options)).unwrap();
    assert!(submitted.warnings.iter().any(|w| w.contains("custom_57x31mm")), "{:?}", submitted.warnings);
    device.wait_for_idle(Duration::from_secs(120)).unwrap();
}

#[test]
fn a_cancelled_request_ends_cancelled() {
    let device = FilePrinterDevice::<Pdf>::new().expect("add a temporary print-to-file printer");
    let pdf = one_page_pdf();
    let cancel = CancelToken::new();
    cancel.cancel();
    let error = WindowsSpooler.submit(SubmitRequest::new(Document::pdf(&pdf), job_for(&device)).with_cancel(cancel)).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Cancelled, "{error}");
}

#[test]
fn an_unknown_printer_is_not_found() {
    let pdf = one_page_pdf();
    let job = Job { printer: Some("printscan: no such printer".into()), ..Job::default() };
    let error = WindowsSpooler.submit(SubmitRequest::new(Document::pdf(&pdf), job)).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NotFound, "{error}");
}
