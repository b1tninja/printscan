//! Where the backend may connect.
//!
//! * A scanner's eSCL root is `http://` or `https://`, a **numeric** IPv4 or IPv6 address that is
//!   loopback, link-local, private (RFC 1918) or unique-local (the check is printscan-wsd's
//!   [`address_allowed`]), an optional port (80 or 443 by default) and a path (`/eSCL` when the URL
//!   gives none). Host names are refused (no DNS lookup can redirect a scan), as are public
//!   addresses, user information, queries, fragments, `.` and `..` segments and characters outside
//!   the unreserved set, `/` and `%`.
//! * Every request goes to the same scheme, address and port as the root. The job a scanner
//!   creates is read from the path of its `Location` header only ([`ScannerUrl::job`]): a device
//!   cannot send the client to another host.

use std::fmt;
use std::net::IpAddr;

use printscan_wsd::policy::address_allowed;

use crate::error::{Error, Result};

const MAX_URL: usize = 512;
const MAX_PATH: usize = 256;

/// The root path most scanners serve eSCL under (their mDNS `rs` value).
pub const DEFAULT_ROOT: &str = "/eSCL";

/// A validated scanner endpoint: the eSCL root of a scanner, or a resource under it. Only this type
/// reaches a [`crate::Transport`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScannerUrl {
    tls: bool,
    host: IpAddr,
    port: u16,
    path: String,
}

/// An [`Error::Policy`] refusal with the reason `why`.
fn refuse<T>(why: impl Into<String>) -> Result<T> {
    Err(Error::Policy(why.into()))
}

/// `text` without `prefix`, compared ASCII case-insensitively.
fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) { text.get(prefix.len()..) } else { None }
}

/// The scheme of `text` and the rest after `://`: `Some((true, rest))` for https.
fn scheme(text: &str) -> Option<(bool, &str)> {
    strip_prefix_ci(text, "http://").map(|rest| (false, rest)).or_else(|| strip_prefix_ci(text, "https://").map(|rest| (true, rest)))
}

/// A path is `/`-separated segments of unreserved characters and `%`, with no `.` or `..` segment
/// and no empty segment inside it.
fn path_ok(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= MAX_PATH
        && path.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/' | b'%'))
        && path.split('/').skip(1).all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

impl ScannerUrl {
    /// Parse and validate a scanner's eSCL root. A URL without a path gets [`DEFAULT_ROOT`]; a
    /// trailing `/` is dropped. The error says what is allowed.
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > MAX_URL {
            return refuse("the scanner URL is too long");
        }
        if text.chars().any(|c| c.is_control() || c.is_whitespace() || !c.is_ascii()) {
            return refuse("the scanner URL must be plain ASCII without spaces or control characters");
        }
        let Some((tls, rest)) = scheme(text) else {
            return refuse("the scanner URL must start with http:// or https://");
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (rest.get(..i).unwrap_or(""), rest.get(i..).unwrap_or("/")),
            None => (rest, "/"),
        };
        if authority.contains('@') {
            return refuse("the scanner URL must not carry user information");
        }
        if path.contains(['?', '#']) || authority.contains(['?', '#']) {
            return refuse("the scanner URL must not have a query or fragment");
        }
        let trimmed = path.trim_end_matches('/');
        let path = if trimmed.is_empty() { DEFAULT_ROOT } else { trimmed };
        if !path_ok(path) {
            return refuse("the scanner URL path is not acceptable (letters, digits, - . _ ~ % and / only, no . or .. segments)");
        }
        let (host_text, port_text) = split_authority(authority)?;
        let host: IpAddr = host_text.parse().map_err(|_| {
            Error::Policy(format!("{host_text:?} is not a numeric address: use an IP address (loopback, link-local or private), not a host name"))
        })?;
        if !address_allowed(host) {
            return refuse(format!("{host} is not a loopback, link-local or private address"));
        }
        let port = match port_text {
            None => {
                if tls {
                    443
                } else {
                    80
                }
            }
            Some(p) => match p.parse::<u16>() {
                Ok(n) if n != 0 => n,
                _ => return refuse(format!("{p:?} is not a valid port")),
            },
        };
        Ok(Self { tls, host, port, path: path.to_owned() })
    }

    /// Whether this is an `https` URL.
    pub fn is_tls(&self) -> bool {
        self.tls
    }

    /// The numeric host address.
    pub fn host(&self) -> IpAddr {
        self.host
    }

    /// The port; 80 or 443 when the URL gave none.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The path, starting with `/`.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The canonical URL text, with the port always written.
    pub fn as_url(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        match self.host {
            IpAddr::V4(a) => format!("{scheme}://{a}:{}{}", self.port, self.path),
            IpAddr::V6(a) => format!("{scheme}://[{a}]:{}{}", self.port, self.path),
        }
    }

    /// The same origin with `path`, which must pass the path rules.
    fn with_path(&self, path: &str) -> Result<Self> {
        if !path_ok(path) {
            return Err(Error::Protocol(format!("the scanner named an unacceptable resource path {:?}", cut(path))));
        }
        Ok(Self { path: path.to_owned(), ..self.clone() })
    }

    /// The resource `name` under this URL (`ScannerCapabilities`, `NextDocument`).
    pub fn join(&self, name: &str) -> Result<Self> {
        self.with_path(&format!("{}/{}", self.path.trim_end_matches('/'), name))
    }

    /// The job a scanner created, from the `Location` of its reply to `POST <root>/ScanJobs`: an
    /// absolute URL (whose path alone is used, so the job stays on this scanner whatever host or port
    /// the device writes), an absolute path, or a path relative to `<root>/ScanJobs`. A trailing `/`
    /// is dropped.
    pub fn job(&self, location: &str) -> Result<Self> {
        let location = location.trim();
        if location.is_empty() || location.len() > MAX_URL || location.contains(['?', '#']) {
            return Err(Error::Protocol(format!("the scanner named an unacceptable job location {:?}", cut(location))));
        }
        let path = if let Some((_, rest)) = scheme(location) {
            rest.find('/').and_then(|i| rest.get(i..)).unwrap_or("/").to_owned()
        } else if location.starts_with('/') {
            location.to_owned()
        } else {
            // Relative to `<root>/ScanJobs`: it replaces the last segment.
            format!("{}/{location}", self.path.trim_end_matches('/'))
        };
        self.with_path(path.trim_end_matches('/'))
    }
}

impl fmt::Display for ScannerUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_url())
    }
}

/// At most 80 characters of a device's text, for an error message.
fn cut(text: &str) -> String {
    text.chars().take(80).collect()
}

/// Split `host[:port]` or `[v6][:port]` into the host text and the port text, unparsed.
fn split_authority(authority: &str) -> Result<(&str, Option<&str>)> {
    if let Some(inner) = authority.strip_prefix('[') {
        let Some(close) = inner.find(']') else { return refuse("unterminated [ in the scanner URL") };
        let host = inner.get(..close).unwrap_or("");
        let after = inner.get(close + 1..).unwrap_or("");
        if after.is_empty() {
            return Ok((host, None));
        }
        return match after.strip_prefix(':') {
            Some(p) => Ok((host, Some(p))),
            None => refuse("unexpected text after the address in the scanner URL"),
        };
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    if host.contains(':') {
        return refuse("an IPv6 address in the scanner URL must be in brackets: http://[addr]:port/eSCL");
    }
    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> ScannerUrl {
        ScannerUrl::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn no(s: &str) {
        assert!(matches!(ScannerUrl::parse(s), Err(Error::Policy(_))), "{s} should be refused");
    }

    #[test]
    fn local_numeric_roots_are_accepted_and_canonical() {
        assert_eq!(ok("http://192.168.1.20/eSCL").as_url(), "http://192.168.1.20:80/eSCL");
        assert_eq!(ok("HTTPS://10.0.0.7").as_url(), "https://10.0.0.7:443/eSCL");
        assert_eq!(ok("http://127.0.0.1:8080/").path(), "/eSCL");
        assert_eq!(ok("http://127.0.0.1:8080/eSCL/").path(), "/eSCL");
        assert_eq!(ok("http://[fd12::9]:9095/scan/eSCL").as_url(), "http://[fd12::9]:9095/scan/eSCL");
        assert!(ok("https://169.254.3.3/eSCL").is_tls());
        assert_eq!(ok("http://172.16.0.1/e%53CL").path(), "/e%53CL");
    }

    #[test]
    fn names_public_and_odd_urls_are_refused() {
        for s in [
            "http://localhost/eSCL",
            "http://scanner.local/eSCL",
            "http://8.8.8.8/eSCL",
            "http://100.64.0.1/eSCL",
            "http://0.0.0.0/eSCL",
            "http://[::ffff:127.0.0.1]/eSCL",
            "http://[2001:db8::1]/eSCL",
            "http://fd00::1/eSCL",
            "http://user@127.0.0.1/eSCL",
            "ftp://127.0.0.1/eSCL",
            "ipp://127.0.0.1/eSCL",
            "127.0.0.1/eSCL",
            "http://127.0.0.1:0/eSCL",
            "http://127.0.0.1:70000/eSCL",
            "http://127.0.0.1/eSCL?x=1",
            "http://127.0.0.1/eSCL#f",
            "http://127.0.0.1/a/../eSCL",
            "http://127.0.0.1/a/./eSCL",
            "http://127.0.0.1//eSCL",
            "http://127.0.0.1/e SCL",
            "http://127.0.0.1/e\"SCL",
            "http://127.0.0.1/\u{e9}",
            "",
            "http://",
        ] {
            no(s);
        }
        no(&format!("http://127.0.0.1/{}", "a".repeat(300)));
    }

    #[test]
    fn resources_stay_under_the_root() {
        let root = ok("http://127.0.0.1:8080/eSCL");
        assert_eq!(root.join("ScannerCapabilities").unwrap().as_url(), "http://127.0.0.1:8080/eSCL/ScannerCapabilities");
        assert!(root.join("../admin").is_err());
        assert!(root.join("a?b").is_err());
    }

    #[test]
    fn a_job_location_keeps_its_path_and_never_changes_the_origin() {
        let root = ok("http://192.168.0.9/eSCL");
        let job = |l: &str| root.job(l).map(|u| u.as_url());
        assert_eq!(job("http://192.168.0.9:80/eSCL/ScanJobs/42").unwrap(), "http://192.168.0.9:80/eSCL/ScanJobs/42");
        // Another host or port in the Location is read for its path alone.
        assert_eq!(job("http://8.8.8.8:9999/eSCL/ScanJobs/42/").unwrap(), "http://192.168.0.9:80/eSCL/ScanJobs/42");
        assert_eq!(job("https://scanner.example/eSCL/ScanJobs/a-b").unwrap(), "http://192.168.0.9:80/eSCL/ScanJobs/a-b");
        assert_eq!(job("/eSCL/ScanJobs/7").unwrap(), "http://192.168.0.9:80/eSCL/ScanJobs/7");
        assert_eq!(job("ScanJobs/7").unwrap(), "http://192.168.0.9:80/eSCL/ScanJobs/7");
        for bad in ["", "/", "/eSCL/../x", "/eSCL/ScanJobs/7?x", "http://1.2.3.4", "/a b"] {
            assert!(job(bad).is_err(), "{bad:?}");
        }
        assert!(job(&format!("/{}", "x".repeat(600))).is_err());
    }
}
