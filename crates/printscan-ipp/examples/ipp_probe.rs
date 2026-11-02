//! Ask an IPP printer what it advertises, and optionally print one job to it.
//!
//! ```text
//! cargo run -p printscan-ipp --example ipp_probe -- <ipp-uri>                     # read only
//! cargo run -p printscan-ipp --example ipp_probe -- <ipp-uri> pdf <file> [intent]
//! cargo run -p printscan-ipp --example ipp_probe -- <ipp-uri> cmyk [intent]
//! ```
//!
//! `cmyk` sends one small page of PWG raster in `cmyk_8` (four patches: cyan, magenta, yellow,
//! black); `intent` is a `print-rendering-intent` keyword such as `relative-bpc`. A job asks for
//! consent on the command line by being given at all.

use std::error::Error;

use printscan::print::pwg::{self, Duplex, PwgOptions, RasterType, TypedPage};
use printscan::print::{Document, DocumentFormat, Job, JobOptions, Registry, RenderingIntent, SubmitRequest, UserConfirmed, namespaced_id};
use printscan_ipp::IppBackend;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(uri) = args.first() else {
        eprintln!("usage: ipp_probe <ipp-uri> [pdf <file> [intent] | cmyk [intent]]");
        std::process::exit(2);
    };
    let backend = IppBackend::new();
    let native = backend.add(uri)?;
    let registry = Registry::new();
    registry.register(Box::new(backend))?;
    let id = namespaced_id("ipp", &native);

    let details = registry.printer_details(&id)?;
    println!("printer       {} ({id})", details.name);
    println!("formats       {:?}", details.formats);
    println!("raster types  {:?}", details.raster_types.iter().map(|t| t.keyword()).collect::<Vec<_>>());
    println!("colour        {:?}", details.colour);
    println!("resolutions   {:?}", details.resolutions.iter().map(|r| (r.x_dpi, r.y_dpi)).collect::<Vec<_>>());
    println!("honoured      {:?}", details.honoured);
    println!("icc profiles  {:?}", details.icc_profiles.iter().map(|p| (&p.name, &p.uri)).collect::<Vec<_>>());
    println!("status        {:?}", details.status);

    let intent = |at: usize| -> Result<Option<RenderingIntent>, Box<dyn Error>> {
        args.get(at).map(|k| RenderingIntent::from_keyword(k).ok_or_else(|| format!("unknown intent {k}").into())).transpose()
    };
    let (document, options) = match args.get(1).map(String::as_str) {
        None => return Ok(()),
        Some("pdf") => {
            let path = args.get(2).ok_or("pdf needs a file")?;
            (std::fs::read(path)?, (DocumentFormat::Pdf, intent(3)?))
        }
        Some("cmyk") => (cmyk_page()?, (DocumentFormat::PwgRaster, intent(2)?)),
        Some(other) => return Err(format!("unknown action {other}").into()),
    };
    let (format, intent) = options;
    let mut job_options = JobOptions::new().with_media("iso_a4_210x297mm");
    if let Some(intent) = intent {
        job_options = job_options.with_rendering_intent(intent);
    }
    let job = Job::default().with_printer(id.clone()).with_title("printscan ipp_probe");
    let request =
        SubmitRequest::new(Document::new(format, &document), job).with_options(job_options).with_consent(UserConfirmed::attest(Some(true))?);
    let submitted = registry.submit_job(request)?;
    println!("submitted     job {:?}", submitted.job.map(|j| j.to_string()));
    println!("message       {}", submitted.message);
    println!("warnings      {:?}", submitted.warnings);
    println!("status        {:?}", submitted.status);
    Ok(())
}

/// One A4 page at 75 dpi in `cmyk_8`: four bands of pure cyan, magenta, yellow and black ink on
/// otherwise blank paper (0 is no ink).
fn cmyk_page() -> Result<Vec<u8>, Box<dyn Error>> {
    let (width, height) = (620u32, 877u32);
    let mut data = vec![0u8; (width * height * 4) as usize];
    for band in 0..4u32 {
        for y in 100 + band * 60..140 + band * 60 {
            for x in 60..560 {
                let at = ((y * width + x) * 4 + band) as usize;
                data[at] = 255;
            }
        }
    }
    let page = TypedPage::new(width, height, RasterType::Cmyk8, data);
    Ok(pwg::encode_typed(&[page], &PwgOptions::new((75, 75), Duplex::Off))?)
}
