//! The reusable compressor.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::mem::MaybeUninit;
use core::ops::Range;

use super::config::{ConfigError, EncoderConfig, SizeOverflow};
use super::core::cctx::{Engine, RequestedParams, compress_bound};
use super::core::stream::Stream;
use super::error::EncodeError;
use super::internal::requested_params;
use super::session::{EncoderSession, EncoderSessionOwned, StreamConfig};
use crate::{Backend, RetentionPolicy};

/// A Zstandard compressor that keeps its tables between frames.
///
/// Every operation produces a complete frame. One-shot operations
/// ([`compress`](Self::compress) and friends) produce the same bytes as the
/// reference's `ZSTD_compress2` with the same parameters; sessions produce
/// the bytes of `ZSTD_compressStream2` driven with the same calls.
///
/// # Examples
///
/// ```
/// use mzstd::{Compressor, EncoderConfig, Level};
///
/// let mut encoder = Compressor::new(EncoderConfig::default().with_level(Level::try_from(7)?))?;
/// let mut output = Vec::new();
/// for payload in [b"first payload".as_slice(), b"second payload".as_slice()] {
///     output.clear();
///     let written = encoder.compress_into(payload, &mut output)?;
///     assert_eq!(written, 0..output.len());
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct Compressor {
    config: EncoderConfig,
    pub(crate) requested: RequestedParams,
    retention: RetentionPolicy,
    backend: Backend,
    pub(crate) engine: Box<Engine>,
    pub(crate) stream: Stream,
    next_input_id: u64,
}

impl Default for Compressor {
    /// A compressor for [`EncoderConfig::default`], level 3.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut encoder = mzstd::Compressor::default();
    /// assert_eq!(encoder.config().level(), mzstd::Level::DEFAULT);
    /// assert!(!encoder.compress(b"payload")?.is_empty());
    /// # Ok::<(), mzstd::EncodeError>(())
    /// ```
    fn default() -> Self {
        // The default configuration holds no explicit parameter, so there
        // is nothing to validate.
        Self::validated(
            EncoderConfig::default(),
            RetentionPolicy::Aggressive,
            Backend::default(),
        )
    }
}

impl Compressor {
    /// A compressor for a configuration that passed validation.
    fn validated(config: EncoderConfig, retention: RetentionPolicy, backend: Backend) -> Self {
        Self {
            config,
            requested: requested_params(&config),
            retention,
            backend,
            engine: Box::new(Engine::with_backend(backend)),
            stream: Stream::new(),
            next_input_id: 0,
        }
    }

    /// Creates a compressor for `config`.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] when a parameter is out of range.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut encoder = mzstd::Compressor::new(Default::default())?;
    /// assert!(!encoder.compress(b"payload")?.is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(config: EncoderConfig) -> Result<Self, ConfigError> {
        Self::builder(config).build()
    }

    /// Starts building a compressor with non-default options.
    ///
    /// # Examples
    ///
    /// ```
    /// use mzstd::{Compressor, RetentionPolicy};
    ///
    /// let encoder = Compressor::builder(Default::default())
    ///     .with_retention(RetentionPolicy::ReleaseAll)
    ///     .build()?;
    /// assert_eq!(encoder.retention(), RetentionPolicy::ReleaseAll);
    /// # Ok::<(), mzstd::ConfigError>(())
    /// ```
    #[must_use]
    pub const fn builder(config: EncoderConfig) -> CompressorBuilder {
        CompressorBuilder {
            config,
            retention: RetentionPolicy::Aggressive,
            backend: None,
        }
    }

    /// The configuration frames are compressed with.
    #[must_use]
    pub const fn config(&self) -> &EncoderConfig {
        &self.config
    }

    /// The SIMD backend the compressor runs on.
    ///
    /// # Examples
    ///
    /// ```
    /// use mzstd::{Backend, Compressor};
    ///
    /// assert_eq!(Compressor::default().backend(), Backend::default());
    /// ```
    #[must_use]
    pub const fn backend(&self) -> Backend {
        self.backend
    }

    /// The retention policy.
    #[must_use]
    pub const fn retention(&self) -> RetentionPolicy {
        self.retention
    }

    /// Replaces the configuration; buffers are kept for reuse.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] when a parameter is out of range; the compressor is
    /// unchanged then.
    ///
    /// # Examples
    ///
    /// ```
    /// use mzstd::{Compressor, EncoderConfig, Level};
    ///
    /// let mut encoder = Compressor::new(Default::default())?;
    /// encoder.reconfigure(EncoderConfig::default().with_level(Level::MAX))?;
    /// assert_eq!(encoder.config().level(), Level::MAX);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn reconfigure(&mut self, config: EncoderConfig) -> Result<(), ConfigError> {
        config.validate()?;
        self.config = config;
        self.requested = requested_params(&config);
        self.stream.reset_session();
        Ok(())
    }

    /// The largest frame a one-shot compression of `input_size` bytes can
    /// produce (`ZSTD_compressBound`).
    ///
    /// # Errors
    ///
    /// [`SizeOverflow`] when the bound does not fit in a `usize`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(mzstd::Compressor::max_compressed_size(0)?, 64);
    /// # Ok::<(), mzstd::SizeOverflow>(())
    /// ```
    pub const fn max_compressed_size(input_size: usize) -> Result<usize, SizeOverflow> {
        match compress_bound(input_size) {
            Some(bound) => Ok(bound),
            None => Err(SizeOverflow),
        }
    }

    fn next_id(&mut self) -> u64 {
        self.next_input_id += 1;
        self.next_input_id
    }

    /// Compresses `src` into a new vector.
    ///
    /// # Errors
    ///
    /// [`EncodeError::Bound`] for inputs whose bound overflows.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut encoder = mzstd::Compressor::new(Default::default())?;
    /// let frame = encoder.compress(b"hello hello hello hello")?;
    /// assert_eq!(&frame[..4], &[0x28, 0xB5, 0x2F, 0xFD]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn compress(&mut self, src: &[u8]) -> Result<Vec<u8>, EncodeError> {
        let mut out = Vec::new();
        self.compress_into(src, &mut out)?;
        Ok(out)
    }

    /// Appends the frame for `src` to `dst` and returns where it landed.
    ///
    /// # Errors
    ///
    /// As [`Self::compress`].
    ///
    /// # Examples
    ///
    /// ```
    /// let mut encoder = mzstd::Compressor::new(Default::default())?;
    /// let mut out = b"prefix".to_vec();
    /// let range = encoder.compress_into(b"payload", &mut out)?;
    /// assert_eq!(range.start, 6);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn compress_into(
        &mut self,
        src: &[u8],
        dst: &mut Vec<u8>,
    ) -> Result<Range<usize>, EncodeError> {
        let bound = Self::max_compressed_size(src.len())?;
        let start = dst.len();
        let limit = start.checked_add(bound).ok_or(SizeOverflow)?;
        let result = if src.len() <= crate::shared::format::BLOCK_SIZE_MAX {
            // A single block needs its full bound anyway. Keep the direct
            // slice path here, without another call for empty/tiny frames.
            dst.resize(limit, 0);
            self.compress_to_slice(src, &mut dst[start..])
        } else {
            self.stream.reset_session();
            let id = self.next_id();
            let result = super::core::compress_once_append(
                &mut self.engine,
                &self.requested,
                src,
                id,
                dst,
                bound,
            )
            .map_err(|error| EncodeError::from_core(error, bound, Some(src.len() as u64)));
            self.apply_retention();
            result
        };
        match result {
            Ok(written) => {
                dst.truncate(start + written);
                Ok(start..start + written)
            }
            Err(error) => {
                dst.truncate(start);
                Err(error)
            }
        }
    }

    /// Writes the frame for `src` into `dst` and returns its size.
    ///
    /// `dst` plays the reference's destination buffer: a destination smaller
    /// than [`Self::max_compressed_size`] is used as is and fails only when the
    /// frame does not fit.
    ///
    /// # Errors
    ///
    /// [`EncodeError::OutputTooSmall`] when the frame does not fit.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut encoder = mzstd::Compressor::new(Default::default())?;
    /// let mut buffer = [0u8; 128];
    /// let written = encoder.compress_to_slice(b"payload", &mut buffer)?;
    /// assert!(written > 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn compress_to_slice(&mut self, src: &[u8], dst: &mut [u8]) -> Result<usize, EncodeError> {
        self.stream.reset_session();
        let id = self.next_id();
        let result =
            super::core::compress_once_into(&mut self.engine, &self.requested, src, id, dst)
                .map_err(|error| EncodeError::from_core(error, dst.len(), Some(src.len() as u64)));
        self.apply_retention();
        result
    }

    /// Writes the frame for `src` into uninitialized memory.
    ///
    /// On success exactly `dst[..written]` is initialized.
    ///
    /// # Errors
    ///
    /// As [`Self::compress_to_slice`].
    ///
    /// # Examples
    ///
    /// ```
    /// let mut encoder = mzstd::Compressor::new(Default::default())?;
    /// let mut out: Vec<u8> = Vec::with_capacity(64);
    /// let written = encoder.compress_to_uninit(b"payload", out.spare_capacity_mut())?;
    /// // SAFETY: the first `written` bytes were initialized.
    /// unsafe { out.set_len(written) };
    /// assert_eq!(out.len(), written);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn compress_to_uninit(
        &mut self,
        src: &[u8],
        dst: &mut [MaybeUninit<u8>],
    ) -> Result<usize, EncodeError> {
        let mut scratch = vec![0u8; dst.len()];
        let written = self.compress_to_slice(src, &mut scratch)?;
        for (slot, &byte) in dst.iter_mut().zip(&scratch[..written]) {
            slot.write(byte);
        }
        Ok(written)
    }

    /// Starts an incremental frame borrowing this compressor.
    ///
    /// Any frame left unfinished by an earlier session is abandoned.
    ///
    /// # Examples
    ///
    /// ```
    /// use mzstd::{Compressor, EncoderStatus, InputSize, Operation};
    ///
    /// let mut encoder = Compressor::new(Default::default())?;
    /// let payload = b"streamed payload";
    /// let mut session = encoder.start(InputSize::Exact(payload.len() as u64).into());
    /// let mut out = [0u8; 256];
    /// let progress = session.process(payload, &mut out, Operation::End)?;
    /// assert_eq!(progress.status, EncoderStatus::Finished);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn start(&mut self, stream: StreamConfig) -> EncoderSession<'_> {
        self.begin_stream(stream);
        EncoderSession::new(self, stream)
    }

    /// Starts an incremental frame that owns this compressor.
    ///
    /// # Examples
    ///
    /// ```
    /// use mzstd::{Compressor, Operation};
    ///
    /// let mut session = Compressor::new(Default::default())?.into_session(Default::default());
    /// let mut out = [0u8; 64];
    /// session.process(b"owned", &mut out, Operation::End)?;
    /// let _compressor = session.into_compressor();
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn into_session(mut self, stream: StreamConfig) -> EncoderSessionOwned {
        self.begin_stream(stream);
        EncoderSessionOwned::new(self, stream)
    }

    pub(crate) fn begin_stream(&mut self, stream: StreamConfig) {
        self.stream.reset_session();
        if let super::session::InputSize::Exact(size) = stream.input_size() {
            // A fresh session is always in its initial stage.
            let _ = self.stream.set_pledged_src_size(size);
        }
    }

    /// Bytes retained between operations.
    ///
    /// # Examples
    ///
    /// ```
    /// let encoder = mzstd::Compressor::new(Default::default())?;
    /// assert_eq!(encoder.retained_bytes(), encoder.retained_bytes());
    /// # Ok::<(), mzstd::ConfigError>(())
    /// ```
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.engine.retained_bytes() + self.stream.retained_bytes()
    }

    /// Releases memory according to `policy`.
    ///
    /// # Examples
    ///
    /// ```
    /// use mzstd::{Compressor, RetentionPolicy};
    ///
    /// let mut encoder = Compressor::new(Default::default())?;
    /// encoder.compress(b"warm up")?;
    /// encoder.trim(RetentionPolicy::ReleaseAll);
    /// assert_eq!(encoder.retained_bytes(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn trim(&mut self, policy: RetentionPolicy) {
        if !policy.allows(self.retained_bytes()) {
            *self.engine = Engine::with_backend(self.backend);
            self.stream.release();
        }
    }

    pub(crate) fn apply_retention(&mut self) {
        self.trim(self.retention);
    }
}

/// Builds a [`Compressor`] with non-default options.
///
/// # Examples
///
/// ```
/// let encoder = mzstd::Compressor::builder(Default::default()).build()?;
/// assert_eq!(encoder.config(), &mzstd::EncoderConfig::default());
/// # Ok::<(), mzstd::ConfigError>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct CompressorBuilder {
    config: EncoderConfig,
    retention: RetentionPolicy,
    backend: Option<Backend>,
}

impl CompressorBuilder {
    /// Sets what is kept allocated between operations.
    #[must_use]
    pub const fn with_retention(mut self, retention: RetentionPolicy) -> Self {
        self.retention = retention;
        self
    }

    /// Selects the SIMD backend instead of the best one the host supports.
    ///
    /// Every backend writes identical frames; this exists for
    /// reproducible measurements and differential tests.
    ///
    /// # Examples
    ///
    /// ```
    /// use mzstd::{Backend, Compressor};
    ///
    /// let mut detected = Compressor::default();
    /// for backend in Backend::available() {
    ///     let mut selected = Compressor::builder(Default::default())
    ///         .with_backend(backend)
    ///         .build()?;
    ///     assert_eq!(selected.backend(), backend);
    ///     assert_eq!(selected.compress(b"identical bytes")?, detected.compress(b"identical bytes")?);
    /// }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn with_backend(mut self, backend: Backend) -> Self {
        self.backend = Some(backend);
        self
    }

    /// Builds the compressor.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] when a parameter is out of range.
    pub fn build(self) -> Result<Compressor, ConfigError> {
        self.config.validate()?;
        Ok(Compressor::validated(
            self.config,
            self.retention,
            self.backend.unwrap_or_default(),
        ))
    }
}
