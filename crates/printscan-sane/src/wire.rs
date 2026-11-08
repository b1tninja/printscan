//! The SANE network encoding.
//!
//! * A word (`SANE_Word`, and every integer, boolean, fixed-point value, status and enumeration
//!   carried as one) is four bytes, big-endian.
//! * A string is a word holding its length **including** the terminating NUL, then that many
//!   bytes. Length 0 is a null string, which this crate reads as `None`.
//! * An array is a word holding the number of elements, then the elements.
//! * A pointer is a word, zero for null; a non-null pointer is followed by the value it points to.
//!
//! Reading is strict and bounded: every count and length is checked against [`Limits`] before
//! anything is allocated, and a reply that ends early is an error, never a panic.

use std::io::Read;

use crate::error::{Error, Result};

/// Bounds on what a reply may make this crate allocate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// The longest string, in bytes with its NUL (default 64 KiB).
    pub max_string: usize,
    /// The most elements in one array: devices, option descriptors, list entries (default 4096).
    pub max_array: usize,
    /// The most image bytes one frame may carry (default 1 GiB, a 1200 dpi colour Legal page with room
    /// to spare).
    pub max_image: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_string: 64 * 1024, max_array: 4096, max_image: 1 << 30 }
    }
}

setters!(Limits {
    with_max_string => max_string: usize,
    with_max_array => max_array: usize,
    with_max_image => max_image: u64,
});

/// Builds one request.
#[derive(Clone, Debug, Default)]
pub struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    /// An empty request.
    pub fn new() -> Self {
        Self::default()
    }

    /// One word.
    pub fn word(&mut self, word: i32) -> &mut Self {
        self.bytes.extend_from_slice(&word.to_be_bytes());
        self
    }

    /// A count or size as a word; one that does not fit is [`Error::Invalid`].
    pub fn count(&mut self, len: usize) -> Result<&mut Self> {
        let word = i32::try_from(len).map_err(|_| Error::Invalid(format!("{len} does not fit in a SANE word")))?;
        Ok(self.word(word))
    }

    /// A string, or the null string for `None`. A string with a NUL in it cannot be carried and is
    /// [`Error::Invalid`].
    pub fn string(&mut self, text: Option<&str>) -> Result<&mut Self> {
        let Some(text) = text else { return Ok(self.word(0)) };
        if text.contains('\0') {
            return Err(Error::Invalid("a SANE string cannot hold a NUL character".into()));
        }
        self.count(text.len() + 1)?;
        self.bytes.extend_from_slice(text.as_bytes());
        self.bytes.push(0);
        Ok(self)
    }

    /// An array of words.
    pub fn words(&mut self, words: &[i32]) -> Result<&mut Self> {
        self.count(words.len())?;
        for &word in words {
            self.word(word);
        }
        Ok(self)
    }

    /// An array of characters, written as given (a string option's value buffer).
    pub fn chars(&mut self, chars: &[u8]) -> Result<&mut Self> {
        self.count(chars.len())?;
        self.bytes.extend_from_slice(chars);
        Ok(self)
    }

    /// The encoded request.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The encoded request, taken.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Reads one reply from a byte stream.
#[derive(Debug)]
pub struct Decoder<R> {
    inner: R,
    limits: Limits,
}

/// Bytes as text: UTF-8 when they are, otherwise each byte as the Latin-1 character it is (SANE
/// strings carry no declared encoding, and many drivers write Latin-1).
pub fn text_of(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

impl<R: Read> Decoder<R> {
    /// A decoder over `inner` with `limits`.
    pub fn new(inner: R, limits: Limits) -> Self {
        Self { inner, limits }
    }

    /// The limits in force.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The stream, back.
    pub fn into_inner(self) -> R {
        self.inner
    }

    /// The stream, borrowed.
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    /// Exactly `buf.len()` bytes.
    pub fn fill(&mut self, buf: &mut [u8]) -> Result<()> {
        self.inner.read_exact(buf).map_err(|e| Error::io("reading the SANE reply", &e))
    }

    /// One word.
    pub fn word(&mut self) -> Result<i32> {
        let mut buf = [0; 4];
        self.fill(&mut buf)?;
        Ok(i32::from_be_bytes(buf))
    }

    /// A word that must be a boolean (0 or 1).
    pub fn boolean(&mut self) -> Result<bool> {
        match self.word()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(Error::Protocol(format!("{other} is not a SANE boolean"))),
        }
    }

    /// A pointer word: whether a value follows.
    pub fn pointer(&mut self) -> Result<bool> {
        Ok(self.word()? != 0)
    }

    /// An array length, checked against [`Limits::max_array`].
    pub fn count(&mut self, what: &'static str) -> Result<usize> {
        let word = self.word()?;
        let count = usize::try_from(word).map_err(|_| Error::Protocol(format!("{what}: a negative count ({word})")))?;
        if count > self.limits.max_array {
            return Err(Error::TooLarge { what, limit: self.limits.max_array as u64 });
        }
        Ok(count)
    }

    /// `len` raw bytes, `len` already checked by the caller.
    fn bytes(&mut self, len: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0; len];
        self.fill(&mut buf)?;
        Ok(buf)
    }

    /// A string; `None` for the null string. The text ends at the first NUL.
    pub fn string(&mut self) -> Result<Option<String>> {
        let word = self.word()?;
        let len = usize::try_from(word).map_err(|_| Error::Protocol(format!("a string of negative length ({word})")))?;
        if len == 0 {
            return Ok(None);
        }
        if len > self.limits.max_string {
            return Err(Error::TooLarge { what: "a string", limit: self.limits.max_string as u64 });
        }
        let bytes = self.bytes(len)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        Ok(Some(text_of(&bytes[..end])))
    }

    /// A string, the null string read as empty.
    pub fn text(&mut self) -> Result<String> {
        Ok(self.string()?.unwrap_or_default())
    }

    /// An array of words.
    pub fn words(&mut self, what: &'static str) -> Result<Vec<i32>> {
        let count = self.count(what)?;
        (0..count).map(|_| self.word()).collect()
    }

    /// An array of characters (a string option's value buffer), checked against [`Limits::max_string`].
    pub fn chars(&mut self) -> Result<Vec<u8>> {
        let word = self.word()?;
        let len = usize::try_from(word).map_err(|_| Error::Protocol(format!("a value of negative length ({word})")))?;
        if len > self.limits.max_string {
            return Err(Error::TooLarge { what: "a string value", limit: self.limits.max_string as u64 });
        }
        self.bytes(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoder(bytes: &[u8]) -> Decoder<&[u8]> {
        Decoder::new(bytes, Limits::default())
    }

    #[test]
    fn words_and_strings_round_trip() {
        let mut e = Encoder::new();
        e.word(-2).word(0x0102_0304);
        e.string(Some("Flatbed")).unwrap().string(None).unwrap().string(Some("")).unwrap();
        e.words(&[1, 2, 3]).unwrap();
        let bytes = e.into_bytes();
        assert_eq!(&bytes[4..8], &[1, 2, 3, 4]);
        // "Flatbed" is 7 bytes and a NUL.
        assert_eq!(&bytes[8..12], &8i32.to_be_bytes());
        let mut d = decoder(&bytes);
        assert_eq!(d.word().unwrap(), -2);
        assert_eq!(d.word().unwrap(), 0x0102_0304);
        assert_eq!(d.string().unwrap().as_deref(), Some("Flatbed"));
        assert_eq!(d.string().unwrap(), None);
        assert_eq!(d.string().unwrap().as_deref(), Some(""));
        assert_eq!(d.words("w").unwrap(), vec![1, 2, 3]);
        assert!(d.word().is_err());
    }

    #[test]
    fn a_nul_cannot_be_sent() {
        assert!(matches!(Encoder::new().string(Some("a\0b")), Err(Error::Invalid(_))));
    }

    #[test]
    fn hostile_lengths_are_refused_before_allocating() {
        // A string that claims 2 GiB.
        assert!(matches!(decoder(&i32::MAX.to_be_bytes()).string(), Err(Error::TooLarge { .. })));
        assert!(matches!(decoder(&(-5i32).to_be_bytes()).string(), Err(Error::Protocol(_))));
        assert!(matches!(decoder(&100_000i32.to_be_bytes()).count("devices"), Err(Error::TooLarge { .. })));
        assert!(matches!(decoder(&(-1i32).to_be_bytes()).count("devices"), Err(Error::Protocol(_))));
        // A string cut short.
        let mut short = 10i32.to_be_bytes().to_vec();
        short.extend_from_slice(b"abc");
        assert!(matches!(decoder(&short).string(), Err(Error::Transport { .. })));
        assert!(matches!(decoder(&2i32.to_be_bytes()).boolean(), Err(Error::Protocol(_))));
    }

    #[test]
    fn text_without_utf8_is_latin1() {
        let mut bytes = 4i32.to_be_bytes().to_vec();
        bytes.extend_from_slice(&[b'G', 0xe9, b'o', 0]);
        assert_eq!(decoder(&bytes).string().unwrap().as_deref(), Some("G\u{e9}o"));
        // Anything after the first NUL is dropped.
        let mut bytes = 5i32.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"ab\0cd");
        assert_eq!(decoder(&bytes).string().unwrap().as_deref(), Some("ab"));
    }
}
