//! PWG raster (`image/pwg-raster`) writer. The encoder itself moved to [`printscan::print::pwg`], which is
//! available without the `wsd` feature (an app's [`DocumentConverter`](printscan::print::DocumentConverter)
//! uses it too); this module keeps the old paths and the [`WsdError`] result type of
//! [`encode`].

use super::error::{Result, WsdError};

pub use printscan::print::pwg::{Duplex, MAX_PAGE_BYTES, MAX_PAGES, MAX_SIDE, MAX_STREAM_BYTES, PwgError, PwgOptions, RasterPage};

#[cfg(test)]
pub(crate) use printscan::print::pwg::decode_for_tests;

impl From<PwgError> for WsdError {
    fn from(error: PwgError) -> Self {
        match error {
            PwgError::TooLarge(what, limit) => WsdError::TooLarge(what, limit),
            PwgError::Invalid(why) => WsdError::Encode(why),
            other => WsdError::Encode(other.to_string()),
        }
    }
}

/// Pack `pages` into a PWG raster stream; see [`printscan::print::pwg::encode`].
pub fn encode(pages: &[RasterPage], opts: &PwgOptions) -> Result<Vec<u8>> {
    printscan::print::pwg::encode(pages, opts).map_err(WsdError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_old_paths_still_encode_and_errors_keep_their_wsd_shape() {
        let page = RasterPage::new(2, 2, true, vec![0, 1, 2, 3]);
        let opts = PwgOptions::new((300, 300), Duplex::Off);
        let bytes = encode(std::slice::from_ref(&page), &opts).unwrap();
        assert_eq!(decode_for_tests(&bytes).unwrap(), [page]);
        assert!(matches!(encode(&[], &opts), Err(WsdError::Encode(_))));
        let huge = RasterPage::new(MAX_SIDE, MAX_SIDE, false, vec![]);
        assert!(matches!(encode(&[huge], &opts), Err(WsdError::TooLarge(..))));
    }
}
