//! The spooler backend against a temporary print-to-file printer (the in-box Microsoft Print To PDF
//! driver on a file port), so nothing reaches a real printer. Needs Windows and the right to add a
//! printer; the printer is removed afterwards.

#![cfg(all(windows, feature = "print"))]

use std::path::Path;
use std::time::{Duration, Instant};

use printscan::print::backends::{Backend, Document, ErrorKind, SubmitRequest};
use printscan::print::{CancelToken, Duplex, Job, JobOptions, PrintError, Scaling, Submitted};
use printscan_windows::spooler::WindowsSpooler;
use winprint::test_utils::file_device::{FilePrinterDevice, Pdf};

/// A one-page PDF (A4) with a filled square, its cross-reference table at the right offsets.
fn one_page_pdf() -> Vec<u8> {
    pdf_of(&[(595.0, 842.0)])
}

/// A PDF of pages of these sizes (points), each with a filled square.
fn pdf_of(sizes: &[(f64, f64)]) -> Vec<u8> {
    let kids: Vec<String> = (0..sizes.len()).map(|i| format!("{} 0 R", 4 + i)).collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), sizes.len()),
        "<< /Length 31 >>\nstream\n0 0 1 rg 100 100 200 150 re f\nendstream".to_owned(),
    ];
    for (w, h) in sizes {
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents 3 0 R >>"));
    }
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

/// The file at `path` once it is a whole PDF (it ends with `%%EOF` and an optional line end), or
/// what it holds when `limit` runs out.
fn whole_pdf(path: &Path, limit: Duration) -> Vec<u8> {
    let started = Instant::now();
    loop {
        let written = std::fs::read(path).unwrap_or_default();
        let whole = [&b"%%EOF"[..], b"%%EOF\n", b"%%EOF\r\n"].iter().any(|end| written.ends_with(end));
        if whole || started.elapsed() > limit {
            return written;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn job_for(device: &FilePrinterDevice<Pdf>) -> Job {
    Job::default().with_printer(device.device().name().to_owned()).with_title("printscan spooler test")
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

    let submitted_at = Instant::now();
    device.wait_for_idle(Duration::from_secs(120)).unwrap();
    let idle_after = submitted_at.elapsed();
    // The queue can be idle a moment before the driver has finished writing the file.
    let written = whole_pdf(device.file_path(), Duration::from_secs(120));
    // This failed now and then (CI and once locally) with nothing written; the message says what
    // the test saw, so the next failure tells more.
    assert!(
        written.starts_with(b"%PDF"),
        "the driver wrote no PDF: the queue was idle {:.1} s after submit, then {:.1} s of waiting; the output file {} ({} bytes)",
        idle_after.as_secs_f32(),
        submitted_at.elapsed().saturating_sub(idle_after).as_secs_f32(),
        if device.file_path().exists() { "exists" } else { "does not exist" },
        written.len()
    );
}

/// The pages of a PDF the driver wrote, as their MediaBox sizes (points, rounded), in file order.
fn page_sizes(pdf: &[u8]) -> Vec<(i64, i64)> {
    let text = String::from_utf8_lossy(pdf);
    text.match_indices("/MediaBox")
        .filter_map(|(at, _)| {
            let rest = &text[at..];
            let (open, close) = (rest.find('[')?, rest.find(']')?);
            let numbers: Vec<f64> = rest.get(open + 1..close)?.split_whitespace().filter_map(|n| n.parse().ok()).collect();
            let [x0, y0, x1, y1] = numbers[..] else { return None };
            Some(((x1 - x0).round() as i64, (y1 - y0).round() as i64))
        })
        .collect()
}

/// Print `pdf` as `job` with `options` on a temporary print-to-file printer: the submit's result and what the
/// printer wrote (nothing when the submit failed).
fn printed(pdf: &[u8], job: impl FnOnce(Job) -> Job, options: JobOptions) -> (Result<Submitted, PrintError>, Vec<u8>) {
    let device = FilePrinterDevice::<Pdf>::new().expect("add a temporary print-to-file printer");
    let result = WindowsSpooler.submit(SubmitRequest::new(Document::pdf(pdf), job(job_for(&device))).with_options(options));
    let written = if result.is_ok() {
        device.wait_for_idle(Duration::from_secs(120)).unwrap();
        whole_pdf(device.file_path(), Duration::from_secs(120))
    } else {
        Vec::new()
    };
    (result, written)
}

#[test]
fn paper_the_driver_does_not_have_refuses_an_exact_job_and_shrinks_onto_its_default_otherwise() {
    // 5 x 18 in is not among Microsoft Print To PDF's papers (ARCH D is).
    let odd = pdf_of(&[(360.0, 1296.0)]);
    let (refused, _) = printed(&odd, |job| job, JobOptions::new());
    let error = refused.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidRequest, "{error}");
    assert!(error.to_string().contains("5.00 x 18.00 in"), "{error}");

    let (submitted, written) = printed(&odd, |job| job.with_scaling(Scaling::ShrinkToFit), JobOptions::new());
    let submitted = submitted.unwrap();
    assert!(submitted.warnings.iter().any(|w| w.contains("default paper")), "{:?}", submitted.warnings);
    let sizes = page_sizes(&written);
    assert_eq!(sizes.len(), 1, "{sizes:?}");
    assert!(sizes[0].1 < 1296, "printed on the default paper, not 5 x 18 in: {sizes:?}");

    // A media name without a size is a warning; the first page's size stands.
    let (submitted, _) = printed(&one_page_pdf(), |job| job, JobOptions::new().with_media("Letter"));
    assert!(submitted.unwrap().warnings.iter().any(|w| w.contains("Letter")));
}

#[test]
fn portrait_and_landscape_pages_print_as_one_job_on_one_paper() {
    let (submitted, written) = printed(&pdf_of(&[(595.0, 842.0), (842.0, 595.0), (595.0, 842.0)]), |job| job, JobOptions::new());
    submitted.unwrap();
    assert_eq!(page_sizes(&written), [(595, 842); 3], "the landscape page is turned onto the portrait paper");
    // The first page decides: a landscape document prints on landscape paper.
    let (submitted, written) = printed(&pdf_of(&[(842.0, 595.0), (595.0, 842.0)]), |job| job, JobOptions::new());
    submitted.unwrap();
    assert_eq!(page_sizes(&written), [(842, 595); 2]);
}

#[test]
fn an_exact_job_with_a_page_of_another_size_is_refused_before_anything_is_sent() {
    let mixed = pdf_of(&[(595.0, 842.0), (612.0, 792.0)]);
    let (refused, _) = printed(&mixed, |job| job, JobOptions::new());
    let error = refused.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidRequest, "{error}");
    assert!(error.to_string().starts_with("page 2 is 8.50 x 11.00 in"), "{error}");
    // Shrunk to fit, it prints, on the first page's paper.
    let (submitted, written) = printed(&mixed, |job| job.with_scaling(Scaling::ShrinkToFit), JobOptions::new());
    submitted.unwrap();
    assert_eq!(page_sizes(&written), [(595, 842); 2]);
}

#[test]
fn two_sided_on_a_one_sided_driver_is_refused_and_one_sided_prints() {
    let pdf = one_page_pdf();
    let (refused, _) = printed(&pdf, |job| job.with_duplex(Duplex::LongEdge), JobOptions::new());
    let error = refused.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Rejected, "{error}");
    let message = error.to_string();
    assert!(message.contains("two-sided on the long edge") && message.contains("nothing was sent"), "{error}");
    let (submitted, _) = printed(&pdf, |job| job.with_duplex(Duplex::Off), JobOptions::new());
    submitted.unwrap();
}

#[test]
fn copies_come_out_collated_whether_the_driver_or_the_job_makes_them() {
    let (submitted, written) = printed(&pdf_of(&[(595.0, 842.0), (842.0, 595.0)]), |job| job.with_copies(2), JobOptions::new());
    let submitted = submitted.unwrap();
    let sizes = page_sizes(&written);
    if submitted.message.contains("drawn into one job") {
        // The job drew them: the document twice, in order.
        assert_eq!(sizes, [(595, 842); 4], "{}", submitted.message);
    } else {
        // The driver makes them: the file holds the document once.
        assert_eq!(sizes, [(595, 842); 2], "{}", submitted.message);
    }
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
    let job = Job::default().with_printer("printscan: no such printer");
    let error = WindowsSpooler.submit(SubmitRequest::new(Document::pdf(&pdf), job)).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NotFound, "{error}");
}
