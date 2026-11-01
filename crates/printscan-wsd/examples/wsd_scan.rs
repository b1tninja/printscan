//! Read a WS-Scan scanner's details, or scan from it.
//!
//! ```text
//! cargo run -p printscan-wsd --example wsd_scan -- details <scanner service url>     # GetScannerElements only
//! cargo run -p printscan-wsd --example wsd_scan -- scan <url> <out-prefix> [glass|feeder] [pages] [dpi] [rgb|gray|bw]
//! ```
//!
//! `scan` writes each page as `<out-prefix>-<n>.<format>`, as the scanner delivered it.

use std::error::Error;

use printscan::scan::{Backend as _, Color, ScanSettings, Source};
use printscan_wsd::scan::WsdScanBackend;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(command), Some(url)) = (args.first(), args.get(1)) else {
        eprintln!("usage: wsd_scan details <url> | scan <url> <out-prefix> [glass|feeder] [pages] [dpi] [rgb|gray|bw]");
        std::process::exit(2);
    };
    let backend = WsdScanBackend::new();
    let id = backend.add(url)?;
    match command.as_str() {
        "details" => println!("{:#?}", backend.scanner_details(&id)?),
        "scan" => {
            let out = args.get(2).ok_or("scan needs an output prefix")?;
            let source = if args.get(3).map(String::as_str) == Some("feeder") { Source::Feeder } else { Source::Glass };
            let pages = args.get(4).map(|p| p.parse()).transpose()?.unwrap_or(1);
            let dpi = args.get(5).map(|d| d.parse()).transpose()?.unwrap_or(300);
            let color = match args.get(6).map(String::as_str) {
                Some("gray") => Color::Gray,
                Some("bw") => Color::BlackAndWhite,
                _ => Color::Rgb,
            };
            let settings = ScanSettings { device: id, source, pages, dpi, color, ..ScanSettings::default() };
            let acquired = backend.acquire(&settings)?;
            for (n, page) in acquired.pages.iter().enumerate() {
                let path = format!("{out}-{}.{}", n + 1, page.format);
                std::fs::write(&path, &page.bytes)?;
                println!("{path}: {}x{} at {} dpi, {} bytes", page.width, page.height, page.dpi, page.bytes.len());
            }
        }
        other => return Err(format!("unknown command {other:?}").into()),
    }
    Ok(())
}
