//! List the queues of the local CUPS scheduler (or of the one named), and describe one. Prints
//! nothing.
//!
//! ```text
//! cargo run -p printscan-cups --example cups_queues                      # ipp://127.0.0.1:631/
//! cargo run -p printscan-cups --example cups_queues -- <server-uri> [queue]
//! ```

use std::error::Error;

use printscan::print::backends::Backend;
use printscan_cups::CupsBackend;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut backend = CupsBackend::new();
    if let Some(server) = args.first() {
        backend = backend.with_server(server)?;
    }
    let default = backend.default_queue()?.map(|q| q.name);
    println!("scheduler     {}", backend.server());
    println!("default       {default:?}");
    for queue in backend.queues()? {
        let kind = if queue.is_class { "class" } else { "printer" };
        println!("{:<13} {kind}, {:?}, accepting {:?}, {}", queue.name, queue.state, queue.accepting_jobs, queue.display_name());
    }
    if let Some(queue) = args.get(1).cloned().or(default) {
        let details = backend.printer_details(&queue)?;
        println!("{queue}: formats {:?}", details.formats);
        println!("{queue}: paper {:?}", details.paper);
        println!("{queue}: status {:?}", details.status);
    }
    Ok(())
}
