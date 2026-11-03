//! Print one file to a WS-Print printer and follow the job to its end.
//!
//! ```text
//! cargo run -p printscan-wsd --example wsd_print -- <printer service url> <file> [full|description|jobname|empty]
//! ```
//!
//! The format comes from the extension: `.pdf`, `.ps`, `.pwg`, anything else as the printer's own
//! language (`application/octet-stream`). The last argument is the ticket shape (default `full`);
//! `description` is for printers that do not answer a ticket with `JobProcessing` or
//! `DocumentProcessing` in it. Running it is the consent: it prints.

use std::error::Error;
use std::time::{Duration, Instant};

use printscan::print::{Document, DocumentFormat, Job, Registry, SubmitRequest, UserConfirmed, namespaced_id};
use printscan_wsd::print::{TicketShape, WsdBackend};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(url), Some(file)) = (args.first(), args.get(1)) else {
        eprintln!("usage: wsd_print <printer service url> <file> [full|description|jobname|empty]");
        std::process::exit(2);
    };
    let shape = match args.get(2).map_or("full", String::as_str) {
        "full" => TicketShape::Full,
        "description" => TicketShape::Description,
        "jobname" => TicketShape::JobNameOnly,
        "empty" => TicketShape::Empty,
        other => return Err(format!("unknown ticket shape {other}").into()),
    };
    let format = match file.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("pdf") => DocumentFormat::Pdf,
        Some("ps") => DocumentFormat::PostScript,
        Some("pwg") => DocumentFormat::PwgRaster,
        _ => DocumentFormat::OctetStream,
    };
    let document = std::fs::read(file)?;

    let backend = WsdBackend::new().with_ticket_shape(shape);
    let native = backend.add(url)?;
    let registry = Registry::new();
    registry.register(Box::new(backend))?;
    let id = namespaced_id("wsd", &native);

    let job = Job::default().with_printer(id).with_title("printscan wsd_print");
    let request = SubmitRequest::new(Document::new(format, &document), job).with_consent(UserConfirmed::attest(Some(true))?);
    let submitted = registry.submit_job(request)?;
    println!("submitted  {}", submitted.message);
    for warning in &submitted.warnings {
        println!("warning    {warning}");
    }
    let Some(job) = submitted.job else {
        println!("the printer gives no job to follow");
        return Ok(());
    };

    let started = Instant::now();
    let mut last = None;
    loop {
        let status = registry.job_status(&job)?;
        let line = format!("{:?} {:?}", status.state, status.reasons);
        if last.as_ref() != Some(&line) {
            println!("{:>5.1}s    {line}", started.elapsed().as_secs_f32());
            last = Some(line);
        }
        if status.state.is_terminal() {
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(300) {
            return Err("the job did not finish within 5 minutes".into());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}
