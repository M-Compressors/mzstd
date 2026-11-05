//! Codec plumbing behind the public facade: argument conversion, codec
//! construction and the loops that drive incremental sessions.

use mzstd::dictionary::{DecodeDictionary, DictionaryBuilder, PreparedDictionary};
use mzstd::{
    Compressor, ConfigError, DecodeLimits, DecodeOperation, DecoderConfig, DecoderSessionOwned,
    DecoderStatus, Decompressor, EncoderConfig, EncoderSessionOwned, EncoderStatus, Level,
    Operation,
};

use crate::MzstdError;

/// Output space added per session step before the buffer doubles.
const FIRST_CHUNK: usize = 32 * 1024;

/// The widest chunk a session step offers, so one call never zeroes more
/// than this ahead of the data.
const MAX_CHUNK: usize = 1024 * 1024;

/// Validates an optional level; `None` is zstd's default.
///
/// # Errors
///
/// [`MzstdError::InvalidLevel`] outside the accepted range.
pub(crate) fn level(level: Option<i32>) -> Result<Level, MzstdError> {
    let value = level.unwrap_or(crate::DEFAULT_LEVEL);
    Level::try_from(value).map_err(invalid_level(value))
}

/// Reports `level` as rejected by `source`.
fn invalid_level(level: i32) -> impl FnOnce(ConfigError) -> MzstdError {
    move |source| MzstdError::InvalidLevel { level, source }
}

/// zstd's defaults at `level`: content size recorded, no checksum.
fn config(level: Level) -> EncoderConfig {
    EncoderConfig::default().with_level(level)
}

/// A compressor for `config`.
///
/// # Errors
///
/// [`MzstdError::InvalidLevel`] if the configuration is rejected; it holds
/// only an already validated level, so this reports a validation bug rather
/// than panicking.
fn compressor(config: EncoderConfig) -> Result<Compressor, MzstdError> {
    Compressor::new(config).map_err(invalid_level(config.level().get()))
}

/// A dictionary prepared for compressors built from `config`.
///
/// # Errors
///
/// [`MzstdError::Dictionary`] for a malformed trained dictionary.
fn prepared(dictionary: &[u8], config: EncoderConfig) -> Result<PreparedDictionary, MzstdError> {
    Ok(DictionaryBuilder::new(dictionary)
        .with_config(config)
        .build()?)
}

/// Decoder limits: the default 128 MiB window and the caller's output cap.
const fn limits(max_output_size: Option<usize>) -> DecodeLimits {
    let max_output_bytes = match max_output_size {
        // `usize` is at most 64 bits on every supported target.
        Some(size) => Some(size as u64),
        None => None,
    };
    DecodeLimits::DEFAULT.with_max_output_bytes(max_output_bytes)
}

/// A decompressor enforcing [`limits`].
fn decompressor(max_output_size: Option<usize>) -> Decompressor {
    Decompressor::from(DecoderConfig::DEFAULT.with_limits(limits(max_output_size)))
}

/// One frame for `data` at `level`.
///
/// # Errors
///
/// As [`crate::compress`].
pub(crate) fn compress(data: &[u8], level: Level) -> Result<Vec<u8>, MzstdError> {
    Ok(compressor(config(level))?.compress(data)?)
}

/// One frame for `data` at `level` referencing `dictionary`.
///
/// # Errors
///
/// As [`crate::compress_with_dictionary`].
pub(crate) fn compress_with_dictionary(
    data: &[u8],
    dictionary: &[u8],
    level: Level,
) -> Result<Vec<u8>, MzstdError> {
    let config = config(level);
    let dictionary = prepared(dictionary, config)?;
    Ok(compressor(config)?.compress_with_dictionary(&dictionary, data)?)
}

/// Every frame of `data`, decoded; empty input decodes to nothing.
///
/// # Errors
///
/// As [`crate::decompress`].
pub(crate) fn decompress(
    data: &[u8],
    max_output_size: Option<usize>,
) -> Result<Vec<u8>, MzstdError> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    Ok(decompressor(max_output_size).decompress(data)?)
}

/// Every frame of `data`, decoded with `dictionary`.
///
/// # Errors
///
/// As [`crate::decompress_with_dictionary`].
pub(crate) fn decompress_with_dictionary(
    data: &[u8],
    dictionary: &[u8],
    max_output_size: Option<usize>,
) -> Result<Vec<u8>, MzstdError> {
    let dictionary = DecodeDictionary::try_from(dictionary)?;
    if data.is_empty() {
        return Ok(Vec::new());
    }
    Ok(decompressor(max_output_size).decompress_with_dictionary(&dictionary, data)?)
}

/// zstd's `ZSTD_compressBound` for `input_size`.
///
/// # Errors
///
/// [`MzstdError::InputTooLarge`] when the bound overflows.
pub(crate) fn compress_bound(input_size: usize) -> Result<usize, MzstdError> {
    Compressor::max_compressed_size(input_size)
        .map_err(|_| MzstdError::InputTooLarge { input_size })
}

/// An encoder session at `level`, with `dictionary` when given.
///
/// # Errors
///
/// [`MzstdError::InvalidLevel`] and [`MzstdError::Dictionary`] as for
/// one-shot compression, [`MzstdError::Compression`] when the session cannot
/// take the dictionary.
pub(crate) fn encoder(
    level: Level,
    dictionary: Option<&[u8]>,
) -> Result<EncoderSessionOwned, MzstdError> {
    let config = config(level);
    let compressor = compressor(config)?;
    match dictionary {
        None => Ok(compressor.into_session(Default::default())),
        Some(bytes) => {
            let dictionary = prepared(bytes, config)?;
            Ok(compressor.into_session_with_dictionary(dictionary, Default::default())?)
        }
    }
}

/// A decoder session with the caller's output cap, and `dictionary` when
/// given.
///
/// # Errors
///
/// [`MzstdError::Dictionary`] for a malformed trained dictionary.
pub(crate) fn decoder(
    dictionary: Option<&[u8]>,
    max_output_size: Option<usize>,
) -> Result<DecoderSessionOwned, MzstdError> {
    let decompressor = decompressor(max_output_size);
    match dictionary {
        None => Ok(decompressor.into_session()),
        Some(bytes) => {
            Ok(decompressor.into_session_with_dictionary(DecodeDictionary::try_from(bytes)?))
        }
    }
}

/// What one session step reported, in the terms the pump needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Step {
    /// Input bytes accepted.
    pub(crate) consumed: usize,
    /// Output bytes written.
    pub(crate) produced: usize,
    /// The step stopped because the output was full.
    pub(crate) needs_output: bool,
    /// The operation is complete.
    pub(crate) finished: bool,
}

/// Runs `step` until `input` is consumed and the session no longer waits for
/// output space, or until it finishes when `until_finished` is set, and
/// returns everything it wrote.
///
/// The output grows by a chunk that starts at [`FIRST_CHUNK`] and doubles up
/// to [`MAX_CHUNK`] each time a step fills it.
///
/// # Errors
///
/// The first error `step` returns.
pub(crate) fn pump<E>(
    mut input: &[u8],
    until_finished: bool,
    mut step: impl FnMut(&[u8], &mut [u8]) -> Result<Step, E>,
) -> Result<Vec<u8>, E> {
    let mut output = Vec::new();
    let mut chunk = FIRST_CHUNK;
    loop {
        let start = output.len();
        output.resize(start + chunk, 0);
        let progress = step(input, &mut output[start..])?;
        output.truncate(start + progress.produced);
        input = input.get(progress.consumed..).unwrap_or_default();
        if progress.needs_output {
            chunk = (chunk * 2).min(MAX_CHUNK);
            continue;
        }
        if progress.finished || (!until_finished && input.is_empty()) {
            return Ok(output);
        }
    }
}

/// One encoder step under `operation`.
///
/// # Errors
///
/// [`MzstdError::Compression`] when the encoder fails.
pub(crate) fn encode_step(
    session: &mut EncoderSessionOwned,
    input: &[u8],
    output: &mut [u8],
    operation: Operation,
) -> Result<Step, MzstdError> {
    let progress = session.process(input, output, operation)?;
    Ok(Step {
        consumed: progress.consumed,
        produced: progress.produced,
        needs_output: progress.status == EncoderStatus::NeedsOutput,
        finished: progress.status == EncoderStatus::Finished,
    })
}

/// One decoder step under `operation`.
///
/// # Errors
///
/// [`MzstdError::Decompression`] for corrupt input or a broken limit.
pub(crate) fn decode_step(
    session: &mut DecoderSessionOwned,
    input: &[u8],
    output: &mut [u8],
    operation: DecodeOperation,
) -> Result<Step, MzstdError> {
    let progress = session
        .process(input, output, operation)
        .map_err(|failure| MzstdError::Decompression(failure.into_error()))?;
    Ok(Step {
        consumed: progress.consumed,
        produced: progress.produced,
        needs_output: progress.status == DecoderStatus::NeedsOutput,
        finished: progress.status == DecoderStatus::Finished,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake step that copies at most `limit` bytes per call and finishes
    /// once `total` bytes went through.
    fn copier(total: usize, limit: usize) -> impl FnMut(&[u8], &mut [u8]) -> Result<Step, ()> {
        let mut seen = 0;
        move |input, output| {
            let n = input.len().min(output.len()).min(limit);
            output[..n].copy_from_slice(&input[..n]);
            seen += n;
            Ok(Step {
                consumed: n,
                produced: n,
                needs_output: n == output.len() && n < input.len(),
                finished: seen == total,
            })
        }
    }

    #[test]
    fn pump_copies_input_larger_than_every_chunk() {
        let input: Vec<u8> = (0..3 * MAX_CHUNK).map(|i| i as u8).collect();
        let output = pump(&input, false, copier(input.len(), usize::MAX));
        assert_eq!(output, Ok(input));
    }

    #[test]
    fn pump_keeps_stepping_until_input_is_consumed() {
        let input = vec![9u8; 1000];
        let output = pump(&input, false, copier(usize::MAX, 7));
        assert_eq!(output, Ok(input));
    }

    #[test]
    fn pump_until_finished_ignores_empty_input() {
        let mut calls = 0;
        let output = pump(&[], true, |_, _| {
            calls += 1;
            Ok::<_, ()>(Step {
                consumed: 0,
                produced: 0,
                needs_output: false,
                finished: calls == 3,
            })
        });
        assert_eq!(output, Ok(Vec::new()));
        assert_eq!(calls, 3);
    }

    #[test]
    fn pump_returns_the_first_error() {
        let output = pump(b"abc", false, |_, _| Err::<Step, _>("boom"));
        assert_eq!(output, Err("boom"));
    }

    #[test]
    fn level_defaults_and_rejects_out_of_range() {
        assert_eq!(level(None).ok(), Some(Level::DEFAULT));
        assert_eq!(level(Some(0)).ok(), Some(Level::DEFAULT));
        assert!(matches!(
            level(Some(crate::MAX_LEVEL + 1)),
            Err(MzstdError::InvalidLevel { .. })
        ));
    }

    #[test]
    fn compress_bound_matches_compressor_and_rejects_overflow() {
        assert_eq!(
            compress_bound(1000).ok(),
            Compressor::max_compressed_size(1000).ok()
        );
        assert!(matches!(
            compress_bound(usize::MAX),
            Err(MzstdError::InputTooLarge {
                input_size: usize::MAX
            })
        ));
    }
}
