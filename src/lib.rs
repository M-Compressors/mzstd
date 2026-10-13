//! Safe Rust Zstandard (RFC 8878) codecs.
//!
//! The encoder is a port of the reference implementation, zstd 1.5.7, and
//! writes the same bytes it does; the decoder reads every frame RFC 8878
//! defines apart from those that need a dictionary.
//!
//! `compression` and `decompression` are enabled by default. Disable default
//! features and select either codec alongside `std` or `no_std` (which needs
//! `alloc`):
//!
//! ```toml
//! mzstd = { version = "0.1", default-features = false, features = ["std", "decompression"] }
//! ```
//!
//! A disabled codec has no module and no re-exports. The `std::io` adapters
//! and parallel compression need `std`.
//!
//! # Byte identity
//!
//! For the same input and parameters, single-threaded compression produces
//! exactly the bytes of zstd 1.5.7: one-shot compression matches
//! `ZSTD_compress2`, and an encoder session matches `ZSTD_compressStream2`
//! driven with the same input chunks, output capacities and operations. The
//! contract covers compression levels -131072 to 22, the explicit
//! compression parameters (window, hash, chain and search logs, minimum
//! match, target length, strategy), long distance matching, the checksum and
//! content-size flags, declared input sizes and size hints.
//!
//! Parallel compression writes valid frames that decode to the input but are
//! not those of the reference's multithreaded mode. Dictionaries, the
//! magicless format, `targetCBlockSize` and external sequence producers are
//! not supported.
#![cfg_attr(
    feature = "compression",
    doc = r#"
# Compression

| I/O shape | API |
| --- | --- |
| Return a new `Vec<u8>` | [`Compressor::compress`] |
| Append to an existing `Vec` | [`Compressor::compress_into`] |
| Write into a caller-owned slice | [`Compressor::compress_to_slice`], [`Compressor::compress_to_uninit`] |
| Drive input and output incrementally | [`Compressor::start`] → [`EncoderSession`] |
| Same, with the session owning the compressor | [`Compressor::into_session`] → [`EncoderSessionOwned`] |

```
use mzstd::{Compressor, EncoderConfig, Level};

let mut encoder = Compressor::new(EncoderConfig::default().with_level(Level::try_from(19)?))?;
let payload = "the quick brown fox ".repeat(500);
let compressed = encoder.compress(payload.as_bytes())?;
assert!(compressed.len() < payload.len() / 50);
# Ok::<(), Box<dyn std::error::Error>>(())
```

[`EncoderConfig`] selects the level and frame flags, and
[`CompressionParams`] overrides individual parameters exactly as
`ZSTD_CCtx_setParameter` would.
"#
)]
#![cfg_attr(
    feature = "decompression",
    doc = r#"
# Decompression

| I/O shape | API |
| --- | --- |
| Return a new `Vec<u8>` | [`Decompressor::decompress`] |
| Append to an existing `Vec` | [`Decompressor::decompress_into`] |
| Write into a caller-owned slice | [`Decompressor::decompress_to_slice`] |
| Drive input and output incrementally | [`Decompressor::start`] → [`DecoderSession`] |
| Same, with the session owning the decompressor | [`Decompressor::into_session`] → [`DecoderSessionOwned`] |

[`DecodeLimits`] bound the window, input and output, so untrusted frames
cannot make the decoder allocate without limit.
"#
)]
#![cfg_attr(
    all(feature = "compression", feature = "decompression"),
    doc = r#"
```
use mzstd::{Compressor, Decompressor};

let payload = b"round trip, round trip, round trip";
let compressed = Compressor::default().compress(payload)?;
assert_eq!(Decompressor::default().decompress(&compressed)?, payload);
# Ok::<(), Box<dyn std::error::Error>>(())
```
"#
)]
#![cfg_attr(
    all(feature = "compression", not(feature = "no_std")),
    doc = r#"
# Streams and threads

[`Compressor::reader`] and [`Compressor::writer`] adapt the encoder to
`std::io`, and [`compressor::parallel::ParallelCompressor`] splits one
frame into segments compressed as independent tasks on your executor.
"#
)]
#![cfg_attr(
    all(feature = "decompression", not(feature = "no_std")),
    doc = r#"
[`Decompressor::reader`] and [`Decompressor::writer`] adapt the decoder to
`std::io`; the adapters live in [`io`].
"#
)]
#![cfg_attr(feature = "no_std", no_std)]
#![cfg_attr(not(test), forbid(unsafe_code))]
#![deny(missing_docs)]
#![deny(missing_debug_implementations)]
#![deny(rustdoc::broken_intra_doc_links)]

#[cfg(any(feature = "compression", feature = "decompression"))]
extern crate alloc;

#[cfg(any(feature = "compression", feature = "decompression"))]
mod retention;
#[cfg(any(feature = "compression", feature = "decompression"))]
mod shared;
#[cfg(any(feature = "compression", feature = "decompression"))]
pub use retention::RetentionPolicy;

#[cfg(all(
    any(feature = "compression", feature = "decompression"),
    not(feature = "no_std")
))]
mod finish_error;
#[cfg(all(
    any(feature = "compression", feature = "decompression"),
    not(feature = "no_std")
))]
pub mod io;

#[cfg(feature = "compression")]
pub mod compressor;
#[cfg(feature = "compression")]
pub use compressor::{
    CompressionParams, Compressor, CompressorBuilder, ConfigError, EncodeError, EncoderConfig,
    EncoderSession, EncoderSessionOwned, EncoderStatus, InputSize, LdmParams, Level,
    LongDistanceMatching, Operation, Parameter, Progress, SizeOverflow, Strategy, StreamConfig,
};

#[cfg(feature = "decompression")]
pub mod decompressor;

#[cfg(feature = "decompression")]
pub use decompressor::{
    DecodeConfigError, DecodeError, DecodeFailure, DecodeLimits, DecodeOperation, DecodeProgress,
    DecoderConfig, DecoderSession, DecoderSessionOwned, DecoderStatus, Decompressor,
    DecompressorBuilder, FrameMode, InvalidDataKind,
};
