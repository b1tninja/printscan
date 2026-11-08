//! Queue names and the URIs made from them.
//!
//! A CUPS queue name is the native printer id of this backend. It is checked before it is put in a
//! URI: not empty, at most [`MAX_QUEUE_NAME`] bytes, no control characters, no white space and none
//! of `/ \ ? # ' "` (the characters CUPS itself does not allow in a name), and not `.` or `..`
//! (which a path would read as a dot-segment). Everything else is
//! allowed, and every byte outside the URI's unreserved characters is percent-encoded in the path,
//! so a name can never reach another resource of the scheduler.

use printscan::print::{ErrorKind, Phase, PrintError};

/// The longest queue name accepted, in bytes (CUPS's own limit is 127).
pub const MAX_QUEUE_NAME: usize = 127;

/// The characters a queue name must not contain, besides control characters and white space.
const FORBIDDEN: &[char] = &['/', '\\', '?', '#', '\'', '"'];

/// Whether `name` can name a CUPS queue; see the [module documentation](self).
pub fn is_valid_queue_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= MAX_QUEUE_NAME
        && !name.chars().any(|c| c.is_control() || c.is_whitespace() || FORBIDDEN.contains(&c))
}

/// [`is_valid_queue_name`] as an error: [`ErrorKind::InvalidRequest`] naming what is wrong.
pub fn check_queue_name(name: &str) -> Result<(), PrintError> {
    if is_valid_queue_name(name) {
        return Ok(());
    }
    let why = if name.is_empty() {
        "it is empty".to_owned()
    } else if name.len() > MAX_QUEUE_NAME {
        format!("it is longer than {MAX_QUEUE_NAME} bytes")
    } else {
        "it is a dot-segment, or contains white space, a control character or one of / \\ ? # ' \"".to_owned()
    };
    Err(PrintError::new(ErrorKind::InvalidRequest, format!("{name:?} is not a CUPS queue name: {why}")).with_phase(Phase::Prepare))
}

/// The path segment of a queue: unreserved characters (RFC 3986 `ALPHA / DIGIT / - . _ ~`) as they
/// are, every other byte of the UTF-8 name as `%XX`.
pub fn encode_segment(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_names_follow_cups_rules() {
        for ok in ["Office", "laser_2", "HP-LaserJet.4", "a", "Büro", "x@y+z", "100%", &"q".repeat(MAX_QUEUE_NAME)] {
            assert!(is_valid_queue_name(ok), "{ok}");
            assert!(check_queue_name(ok).is_ok());
        }
        for no in
            ["", ".", "..", "with space", "tab\there", "a/b", "a\\b", "a?b", "a#b", "a'b", "a\"b", "nul\0", "line\n", &"q".repeat(MAX_QUEUE_NAME + 1)]
        {
            assert!(!is_valid_queue_name(no), "{no:?}");
            assert_eq!(check_queue_name(no).unwrap_err().kind(), ErrorKind::InvalidRequest);
        }
    }

    #[test]
    fn segments_are_percent_encoded_outside_the_unreserved_set() {
        assert_eq!(encode_segment("Office_Laser-2.a~"), "Office_Laser-2.a~");
        assert_eq!(encode_segment("x@y+z"), "x%40y%2Bz");
        assert_eq!(encode_segment("100%"), "100%25");
        assert_eq!(encode_segment("Büro"), "B%C3%BCro");
        // Nothing that could leave the segment survives.
        for name in ["a:b", "a;b", "a..b", "%2F"] {
            let segment = encode_segment(name);
            assert!(!segment.contains(['/', '?', '#', ':', ';']), "{segment}");
        }
    }
}
