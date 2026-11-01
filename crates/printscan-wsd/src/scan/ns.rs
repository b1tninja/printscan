//! The WS-Scan namespace and operation names.

use crate::service::ServiceKind;

/// `wscn`, the WS-Scan schema namespace.
pub const WSCN: &str = ServiceKind::Scan.descriptor().namespace;
