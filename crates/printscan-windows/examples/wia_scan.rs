//! List WIA scanners, read one's capabilities, or scan one page from its glass.
//!
//! ```text
//! cargo run -p printscan-windows --example wia_scan -- list                 # opens no device
//! cargo run -p printscan-windows --example wia_scan -- details <id>         # opens the device, reads properties
//! cargo run -p printscan-windows --example wia_scan -- scan <id> <out> [dpi] [rgb|gray|bw]
//! ```
//!
//! `<id>` is a registry id as `list` prints it (`wia:<WIA device id>`). `scan` takes one page from
//! the glass and writes it as the scanner delivered it (BMP, PNG, JPEG or TIFF).

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use printscan::scan::{self, Color, ScanSettings, Source};

    let args: Vec<String> = std::env::args().skip(1).collect();
    if !printscan_windows::wia::register() {
        return Err("no WIA backend".into());
    }
    match args.first().map(String::as_str) {
        Some("list") => {
            let listing = scan::scanner_list()?;
            for s in &listing.items {
                println!("{}  {}  ({:?}{})", s.id, s.name, s.connection, s.port.as_deref().map(|p| format!(", {p}")).unwrap_or_default());
            }
            for failed in &listing.errors {
                println!("backend {}: {}", failed.backend, failed.error);
            }
        }
        Some("details") => {
            let id = args.get(1).ok_or("details needs an id")?;
            println!("{:#?}", scan::scanner_details(id)?);
        }
        Some("scan") => {
            let id = args.get(1).ok_or("scan needs an id")?;
            let out = args.get(2).ok_or("scan needs an output path")?;
            let dpi = args.get(3).map(|d| d.parse()).transpose()?.unwrap_or(300);
            let color = match args.get(4).map(String::as_str) {
                Some("gray") => Color::Gray,
                Some("bw") => Color::BlackAndWhite,
                _ => Color::Rgb,
            };
            let settings = ScanSettings { device: id.clone(), source: Source::Glass, dpi, color, pages: 1, ..ScanSettings::default() };
            let acquired = scan::acquire(&settings)?;
            let page = acquired.pages.first().ok_or("no page")?;
            std::fs::write(out, &page.bytes)?;
            println!(
                "{} page(s) from {:?}; first: {} {}x{} at {} dpi, {} bytes -> {out}",
                acquired.pages.len(),
                acquired.source,
                page.format,
                page.width,
                page.height,
                page.dpi,
                page.bytes.len()
            );
        }
        _ => {
            eprintln!("usage: wia_scan list | details <id> | scan <id> <out> [dpi] [rgb|gray|bw]");
            std::process::exit(2);
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn main() {}
