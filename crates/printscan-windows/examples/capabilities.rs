//! Print the colour-related features a Windows printer's driver lists in its PrintCapabilities
//! (read only: nothing is printed).
//!
//! ```text
//! cargo run -p printscan-windows --example capabilities -- "Printer name" [--xml]
//! ```

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use winprint::printer::PrinterDevice;
    use winprint::ticket::PrintCapabilities;

    let mut args = std::env::args().skip(1);
    let Some(name) = args.next() else {
        eprintln!("usage: capabilities <printer name> [--xml]");
        std::process::exit(2);
    };
    let device = PrinterDevice::by_os_name(&name)?;
    let xml = String::from_utf8(PrintCapabilities::fetch_xml(&device)?)?;
    if args.next().as_deref() == Some("--xml") {
        println!("{xml}");
        return Ok(());
    }
    for feature in [
        "PageColorManagement",
        "PageDestinationColorProfile",
        "PageICMRenderingIntent",
        "PageSourceColorProfile",
        "PageOutputColor",
        "PageDeviceColorSpaceUsage",
        "PageBlackGenerationProcessing",
        "JobOptimalDestinationColorProfile",
    ] {
        let listed = xml.contains(&format!("psk:{feature}\""));
        let options: Vec<&str> = match xml.find(&format!("name=\"psk:{feature}\"")) {
            Some(at) => {
                let rest = &xml[at..];
                let end = rest.find("</psf:Feature>").unwrap_or(rest.len());
                rest[..end].split("<psf:Option name=\"").skip(1).filter_map(|o| o.split('"').next()).collect()
            }
            None => Vec::new(),
        };
        println!("{feature:36} {}{}", if listed { "listed" } else { "-" }, if options.is_empty() { String::new() } else { format!("  {options:?}") });
    }
    Ok(())
}

#[cfg(not(windows))]
fn main() {}
