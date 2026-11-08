//! List a SANE daemon's devices, read one's options, or scan from it.
//!
//! ```text
//! cargo run -p printscan-sane --example sane_scan -- list <daemon address>             # get devices only
//! cargo run -p printscan-sane --example sane_scan -- details <daemon address> <device>  # option descriptors, no scan
//! cargo run -p printscan-sane --example sane_scan -- scan <daemon address> <device> <out-prefix> [glass|feeder] [pages] [dpi] [rgb|gray|bw]
//! ```
//!
//! The address is a numeric local one (`127.0.0.1` for the local `saned`). `scan` writes each page
//! as `<out-prefix>-<n>.png`.

use std::error::Error;

use printscan::scan::{Backend as _, Color, ScanSettings, Source};
use printscan_sane::scan::SaneBackend;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(command), Some(host)) = (args.first(), args.get(1)) else {
        eprintln!(
            "usage: sane_scan list <host> | details <host> <device> | scan <host> <device> <out-prefix> [glass|feeder] [pages] [dpi] [rgb|gray|bw]"
        );
        std::process::exit(2);
    };
    let backend = SaneBackend::new();
    backend.add_host(host)?;
    if command == "list" {
        for row in backend.scanner_list()? {
            println!("{}  {}", row.id, row.name);
        }
        return Ok(());
    }
    let device = args.get(2).ok_or("name a device (see `list`)")?;
    let id = format!("{}/{device}", printscan_sane::SaneHost::parse(host)?);
    match command.as_str() {
        "details" => {
            println!("{:#?}", backend.scanner_details(&id)?);
            print!("{}", backend.diagnostic_dump(&id)?);
        }
        "scan" => {
            let out = args.get(3).ok_or("scan needs an output prefix")?;
            let source = if args.get(4).map(String::as_str) == Some("feeder") { Source::Feeder } else { Source::Glass };
            let pages = args.get(5).map(|p| p.parse()).transpose()?.unwrap_or(1);
            let dpi = args.get(6).map(|d| d.parse()).transpose()?.unwrap_or(300);
            let color = match args.get(7).map(String::as_str) {
                Some("gray") => Color::Gray,
                Some("bw") => Color::BlackAndWhite,
                _ => Color::Rgb,
            };
            let settings = ScanSettings::new(id).with_source(source).with_pages(pages).with_dpi(dpi).with_color(color);
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
