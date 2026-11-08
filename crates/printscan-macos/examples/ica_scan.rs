//! List ImageCaptureCore scanners, read one's capabilities, or scan one page from its glass.
//!
//! ```text
//! cargo run -p printscan-macos --example ica_scan -- list                 # opens no device
//! cargo run -p printscan-macos --example ica_scan -- details <id>         # opens a session, reads the units
//! cargo run -p printscan-macos --example ica_scan -- dump [filter]        # the same, as text
//! cargo run -p printscan-macos --example ica_scan -- scan <id> <out> [dpi] [rgb|gray|bw]
//! ```
//!
//! `<id>` is a registry id as `list` prints it (`ica:<persistent id>`). Add `--network` before the
//! command to browse network scanners too. `scan` takes one page from the glass and writes it as
//! ImageCaptureCore delivered it (TIFF). The example calls from the main thread, so the backend
//! runs the main run loop while it waits.

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use printscan::scan::{self, Color, ScanSettings, Source};
    use printscan_macos::ica::{Ica, IcaOptions};

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let network = args.first().is_some_and(|arg| arg == "--network");
    if network {
        args.remove(0);
    }
    scan::register(Box::new(Ica::with_options(IcaOptions::default().with_network(network))))?;
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
        Some("dump") => print!("{}", scan::diagnostic_dump(args.get(1).map_or("", String::as_str))?),
        Some("scan") => {
            let id = args.get(1).ok_or("scan needs an id")?;
            let out = args.get(2).ok_or("scan needs an output path")?;
            let dpi = args.get(3).map(|d| d.parse()).transpose()?.unwrap_or(300);
            let color = match args.get(4).map(String::as_str) {
                Some("gray") => Color::Gray,
                Some("bw") => Color::BlackAndWhite,
                _ => Color::Rgb,
            };
            let settings = ScanSettings::default().with_device(id.clone()).with_source(Source::Glass).with_dpi(dpi).with_color(color).with_pages(1);
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
            eprintln!("usage: ica_scan [--network] list | details <id> | dump [filter] | scan <id> <out> [dpi] [rgb|gray|bw]");
            std::process::exit(2);
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {}
