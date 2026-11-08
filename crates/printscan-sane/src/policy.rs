//! Where the client may connect.
//!
//! A [`SaneHost`] is a **numeric** IPv4 or IPv6 address that is loopback, link-local, private
//! (RFC 1918) or unique-local (`fc00::/7`), with a port (6566 when none is given). Host names (so
//! no DNS), public addresses, multicast, broadcast, unspecified, carrier-grade NAT and IPv4-mapped
//! IPv6 addresses are refused. The data connection of a scan goes to the same address as the
//! control connection, on the port the daemon names; an address the daemon sends is never dialled.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;

use crate::error::{Error, Result};

/// The port `saned` listens on.
pub const DEFAULT_PORT: u16 = 6566;

/// A `saned` the client may talk to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SaneHost {
    addr: SocketAddr,
}

/// Loopback, link-local or private IPv4 (10/8, 172.16/12, 192.168/16).
fn admitted_v4(a: Ipv4Addr) -> bool {
    !(a.is_unspecified() || a.is_multicast() || a.is_broadcast()) && (a.is_loopback() || a.is_link_local() || a.is_private())
}

/// Loopback, link-local (`fe80::/10`) or unique-local (`fc00::/7`) IPv6; never an IPv4-mapped one.
fn admitted_v6(a: Ipv6Addr) -> bool {
    if a.is_unspecified() || a.is_multicast() || a.to_ipv4_mapped().is_some() {
        return false;
    }
    let first = a.segments()[0];
    a.is_loopback() || (first & 0xffc0) == 0xfe80 || (first & 0xfe00) == 0xfc00
}

/// Whether the policy admits `ip`.
pub fn admitted(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => admitted_v4(a),
        IpAddr::V6(a) => admitted_v6(a),
    }
}

impl SaneHost {
    /// `ip` on `port`, when the policy admits it.
    pub fn new(ip: IpAddr, port: u16) -> Result<Self> {
        if !admitted(ip) {
            return Err(Error::Policy(format!("{ip} is not a loopback, link-local or private address; a SANE daemon must be on the local network")));
        }
        if port == 0 {
            return Err(Error::Policy("port 0 is not a SANE daemon's port".into()));
        }
        Ok(Self { addr: SocketAddr::new(ip, port) })
    }

    /// Reads `192.168.1.20`, `192.168.1.20:6566`, `fd00::20`, `[fd00::20]` or `[fd00::20]:6566`.
    /// A host name is refused: there is no DNS.
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let refuse = || Error::Policy(format!("{text:?} is not a numeric address with an optional port (a host name is not looked up)"));
        if let Ok(addr) = SocketAddr::from_str(text) {
            return Self::new(addr.ip(), addr.port());
        }
        if let Some(inner) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            return Self::new(IpAddr::V6(Ipv6Addr::from_str(inner).map_err(|_| refuse())?), DEFAULT_PORT);
        }
        let ip = IpAddr::from_str(text).map_err(|_| refuse())?;
        Self::new(ip, DEFAULT_PORT)
    }

    /// The address.
    pub fn ip(&self) -> IpAddr {
        self.addr.ip()
    }

    /// The control port.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// The control connection's address.
    pub fn socket_addr(&self) -> SocketAddr {
        self.addr
    }

    /// The same address on `port`: where a started frame's data is read.
    pub fn data_addr(&self, port: u16) -> Result<SocketAddr> {
        if port == 0 {
            return Err(Error::Protocol("the daemon named data port 0".into()));
        }
        Ok(SocketAddr::new(self.addr.ip(), port))
    }
}

impl fmt::Display for SaneHost {
    /// `192.168.1.20:6566`, `[fd00::20]:6566`: the port is always written.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.addr)
    }
}

impl FromStr for SaneHost {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        Self::parse(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_addresses_are_admitted() {
        for (text, shown) in [
            ("127.0.0.1", "127.0.0.1:6566"),
            ("10.1.2.3:7000", "10.1.2.3:7000"),
            ("192.168.0.9", "192.168.0.9:6566"),
            ("172.20.0.1", "172.20.0.1:6566"),
            ("169.254.1.1", "169.254.1.1:6566"),
            ("::1", "[::1]:6566"),
            ("[fd00::5]", "[fd00::5]:6566"),
            ("[fe80::1]:6567", "[fe80::1]:6567"),
        ] {
            assert_eq!(SaneHost::parse(text).unwrap().to_string(), shown, "{text}");
        }
    }

    #[test]
    fn everything_else_is_refused() {
        for text in [
            "scanner.local",
            "localhost",
            "203.0.113.7",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "2001:db8::1",
            "::ffff:192.168.0.1",
            "ff02::1",
            "127.0.0.1:0",
            "sane://127.0.0.1",
            "",
        ] {
            assert!(matches!(SaneHost::parse(text), Err(Error::Policy(_))), "{text}");
        }
    }

    #[test]
    fn data_goes_to_the_control_address() {
        let host = SaneHost::parse("192.168.5.5").unwrap();
        assert_eq!(host.data_addr(40000).unwrap(), "192.168.5.5:40000".parse().unwrap());
        assert!(host.data_addr(0).is_err());
    }
}
