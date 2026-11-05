//! Binding-friendly facade over [`mzstd`].
//!
//! This crate is the API surface that [alef](https://github.com/xberg-io/alef)
//! turns into the Python, Node.js and WebAssembly packages. Every item takes
//! and returns plain bytes, integers and two stateful stream types, so each
//! target language can map it onto its own `bytes`/`Buffer`/`Uint8Array`
//! without generics, lifetimes or caller-provided output buffers.
//!
//! Rust callers should depend on [`mzstd`] directly: it offers the same
//! codecs with borrowed output, reusable contexts and the full parameter set.
//!
//! One-shot calls produce exactly the bytes of zstd 1.5.7's `ZSTD_compress2`
//! at the same level, and streams those of `ZSTD_compressStream2`, because
//! they forward to [`mzstd::Compressor`].
//!
//! # Examples
//!
//! ```
//! let frame = mzstd_bindings::compress(b"hello, hello, hello", Some(19))?;
//! let restored = mzstd_bindings::decompress(&frame, None)?;
//! assert_eq!(restored, b"hello, hello, hello");
//! # Ok::<(), mzstd_bindings::MzstdError>(())
//! ```

mod core;
mod error;
mod stream;

pub use error::MzstdError;
pub use stream::{StreamCompressor, StreamDecompressor};

/// The level used when a call passes `None`: zstd's default, 3.
pub const DEFAULT_LEVEL: i32 = mzstd::Level::DEFAULT.get();

/// The fastest (most negative) level zstd accepts.
pub const MIN_LEVEL: i32 = mzstd::Level::MIN.get();

/// The strongest level zstd accepts.
pub const MAX_LEVEL: i32 = mzstd::Level::MAX.get();

/// Compresses `data` into one Zstandard frame.
///
/// `level` defaults to [`DEFAULT_LEVEL`]; any value in
/// [`MIN_LEVEL`]`..=`[`MAX_LEVEL`] is accepted, and 0 also means the default.
/// The frame records the content size and carries no checksum, as zstd's
/// own defaults do.
///
/// # Errors
///
/// [`MzstdError::InvalidLevel`] for a level outside the accepted range and
/// [`MzstdError::Compression`] when the encoder fails.
///
/// # Examples
///
/// ```
/// let frame = mzstd_bindings::compress(b"data", None)?;
/// assert_eq!(&frame[..4], &[0x28, 0xb5, 0x2f, 0xfd]);
/// # Ok::<(), mzstd_bindings::MzstdError>(())
/// ```
pub fn compress(data: &[u8], level: Option<i32>) -> Result<Vec<u8>, MzstdError> {
    core::compress(data, core::level(level)?)
}

/// Compresses `data` into one frame that references `dictionary`.
///
/// `dictionary` is either a trained dictionary (one that starts with the
/// Zstandard dictionary magic number) or raw content to use as history.
/// Decompress the result with [`decompress_with_dictionary`] and the same
/// bytes.
///
/// # Errors
///
/// [`MzstdError::InvalidLevel`] for a level outside the accepted range,
/// [`MzstdError::Dictionary`] for a malformed trained dictionary and
/// [`MzstdError::Compression`] when the encoder fails.
///
/// # Examples
///
/// ```
/// let dictionary = b"common prefix shared by every record";
/// let frame = mzstd_bindings::compress_with_dictionary(b"common prefix: 1", dictionary, None)?;
/// let restored = mzstd_bindings::decompress_with_dictionary(&frame, dictionary, None)?;
/// assert_eq!(restored, b"common prefix: 1");
/// # Ok::<(), mzstd_bindings::MzstdError>(())
/// ```
pub fn compress_with_dictionary(
    data: &[u8],
    dictionary: &[u8],
    level: Option<i32>,
) -> Result<Vec<u8>, MzstdError> {
    core::compress_with_dictionary(data, dictionary, core::level(level)?)
}

/// Decompresses every frame in `data`, concatenating their content.
///
/// Skippable frames are ignored and empty input decodes to nothing.
/// `max_output_size` caps the decoded size; pass it for untrusted input so a
/// small frame cannot expand into an arbitrarily large allocation. Windows
/// up to 128 MiB are accepted, which covers every level of the reference
/// encoder without long-distance matching.
///
/// # Errors
///
/// [`MzstdError::Decompression`] for corrupt or truncated input, a window
/// over the limit, or output past `max_output_size`.
///
/// # Examples
///
/// ```
/// let frame = mzstd_bindings::compress(&[7; 1000], None)?;
/// assert_eq!(mzstd_bindings::decompress(&frame, None)?, [7; 1000]);
/// assert!(mzstd_bindings::decompress(&frame, Some(999)).is_err());
/// # Ok::<(), mzstd_bindings::MzstdError>(())
/// ```
pub fn decompress(data: &[u8], max_output_size: Option<usize>) -> Result<Vec<u8>, MzstdError> {
    core::decompress(data, max_output_size)
}

/// Decompresses frames written with `dictionary`.
///
/// Frames that name no dictionary decode as with [`decompress`].
///
/// # Errors
///
/// [`MzstdError::Dictionary`] for a malformed trained dictionary, and the
/// errors of [`decompress`].
///
/// # Examples
///
/// ```
/// let frame = mzstd_bindings::compress_with_dictionary(b"abcabc", b"abc", Some(1))?;
/// assert_eq!(mzstd_bindings::decompress_with_dictionary(&frame, b"abc", None)?, b"abcabc");
/// # Ok::<(), mzstd_bindings::MzstdError>(())
/// ```
pub fn decompress_with_dictionary(
    data: &[u8],
    dictionary: &[u8],
    max_output_size: Option<usize>,
) -> Result<Vec<u8>, MzstdError> {
    core::decompress_with_dictionary(data, dictionary, max_output_size)
}

/// The largest frame [`compress`] can produce for `input_size` bytes, as
/// zstd's `ZSTD_compressBound`.
///
/// # Errors
///
/// [`MzstdError::InputTooLarge`] when the bound does not fit in `usize`.
///
/// # Examples
///
/// ```
/// let frame = mzstd_bindings::compress(&[1, 2, 3], None)?;
/// assert!(frame.len() <= mzstd_bindings::compress_bound(3)?);
/// # Ok::<(), mzstd_bindings::MzstdError>(())
/// ```
pub fn compress_bound(input_size: usize) -> Result<usize, MzstdError> {
    core::compress_bound(input_size)
}
