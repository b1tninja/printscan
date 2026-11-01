//! Ask a WS-Print printer what it reports (read only: GetPrinterElements, nothing is printed).
//!
//! ```text
//! cargo run -p printscan-wsd --example wsd_probe -- http://192.0.2.7:80/WebServices/PrinterService
//! ```

use std::error::Error;

use printscan::print::{Registry, namespaced_id};
use printscan_wsd::print::WsdBackend;

fn main() -> Result<(), Box<dyn Error>> {
    let Some(url) = std::env::args().nth(1) else {
        eprintln!("usage: wsd_probe <printer service url>");
        std::process::exit(2);
    };
    let backend = WsdBackend::new();
    let native = backend.add(&url)?;
    let registry = Registry::new();
    registry.register(Box::new(backend))?;
    let details = registry.printer_details(&namespaced_id("wsd", &native))?;
    println!("printer       {}", details.name);
    println!("formats       {:?}", details.formats);
    println!("raster types  {:?}", details.raster_types.iter().map(|t| t.keyword()).collect::<Vec<_>>());
    println!("colour        {:?}", details.colour);
    println!("two-sided     {:?}", details.two_sided);
    println!("paper         {:?}", details.paper);
    println!("resolutions   {:?}", details.resolutions.iter().map(|r| (r.x_dpi, r.y_dpi)).collect::<Vec<_>>());
    println!("honoured      {:?}", details.honoured);
    println!("icc profiles  {:?}", details.icc_profiles);
    println!("status        {:?}", details.status);
    Ok(())
}
