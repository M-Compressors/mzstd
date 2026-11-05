//! The one error type every binding maps onto its own exception.

use mzstd::dictionary::DictionaryError;
use mzstd::{ConfigError, DecodeError, EncodeError};

/// Why a call failed.
///
/// Bindings surface the [`Display`](std::fmt::Display) text as the
/// exception message; the underlying `mzstd` error stays reachable through
/// [`std::error::Error::source`].
///
/// # Examples
///
/// ```
/// use mzstd_bindings::MzstdError;
///
/// let error = mzstd_bindings::compress(b"", Some(23)).unwrap_err();
/// assert!(matches!(error, MzstdError::InvalidLevel { level: 23, .. }));
/// assert_eq!(error.to_string(), "compression level 23 is outside -131072..=22");
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MzstdError {
    /// The compression level is outside
    /// [`MIN_LEVEL`](crate::MIN_LEVEL)`..=`[`MAX_LEVEL`](crate::MAX_LEVEL).
    #[error("compression level {level} is outside {min}..={max}", min = crate::MIN_LEVEL, max = crate::MAX_LEVEL)]
    InvalidLevel {
        /// The rejected level.
        level: i32,
        /// The validation failure.
        #[source]
        source: ConfigError,
    },
    /// The encoder failed.
    #[error("compression failed: {0}")]
    Compression(#[from] EncodeError),
    /// The input is not valid Zstandard data or breaks a decoding limit.
    #[error("decompression failed: {0}")]
    Decompression(#[from] DecodeError),
    /// A trained dictionary is malformed.
    #[error("invalid dictionary: {0}")]
    Dictionary(#[from] DictionaryError),
    /// The compression bound of the input does not fit in `usize`.
    #[error("input of {input_size} bytes is too large to compress")]
    InputTooLarge {
        /// The input size that overflowed.
        input_size: usize,
    },
    /// A stream was used after [`finish`](crate::StreamCompressor::finish)
    /// or after an earlier call failed.
    #[error("the stream is finished or failed; create a new one")]
    StreamClosed,
}
