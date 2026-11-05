//! Incremental compression and decompression for data that arrives in
//! pieces.

use mzstd::{DecodeOperation, DecoderSessionOwned, EncoderSessionOwned, Operation};

use crate::MzstdError;
use crate::core;

/// Compresses data supplied in pieces into one Zstandard frame.
///
/// Feed input with [`compress`](Self::compress), optionally force buffered
/// data out with [`flush`](Self::flush), and end the frame with
/// [`finish`](Self::finish). Each call returns the compressed bytes it
/// produced, which may be empty; concatenated in order they form the frame,
/// identical to what zstd 1.5.7's `ZSTD_compressStream2` writes for the same
/// calls. The frame records no content size, since it is unknown up front.
///
/// After `finish` or a failed call the stream is closed and every further
/// call fails with [`MzstdError::StreamClosed`].
///
/// # Examples
///
/// ```
/// use mzstd_bindings::StreamCompressor;
///
/// let mut stream = StreamCompressor::new(None)?;
/// let mut frame = stream.compress(b"hello, ")?;
/// frame.extend(stream.compress(b"world")?);
/// frame.extend(stream.finish()?);
/// assert_eq!(mzstd_bindings::decompress(&frame, None)?, b"hello, world");
/// # Ok::<(), mzstd_bindings::MzstdError>(())
/// ```
#[derive(Debug)]
pub struct StreamCompressor {
    /// The open session, or `None` once finished or failed.
    session: Option<EncoderSessionOwned>,
}

impl StreamCompressor {
    /// A stream at `level`, zstd's default when `None`.
    ///
    /// # Errors
    ///
    /// [`MzstdError::InvalidLevel`] for a level outside the accepted range.
    pub fn new(level: Option<i32>) -> Result<Self, MzstdError> {
        Self::open(level, None)
    }

    /// A stream whose frame references `dictionary`, as
    /// [`compress_with_dictionary`](crate::compress_with_dictionary).
    ///
    /// # Errors
    ///
    /// [`MzstdError::InvalidLevel`] for a level outside the accepted range
    /// and [`MzstdError::Dictionary`] for a malformed trained dictionary.
    ///
    /// # Examples
    ///
    /// ```
    /// use mzstd_bindings::StreamCompressor;
    ///
    /// let mut stream = StreamCompressor::with_dictionary(b"shared words", None)?;
    /// let mut frame = stream.compress(b"shared words again")?;
    /// frame.extend(stream.finish()?);
    /// let restored = mzstd_bindings::decompress_with_dictionary(&frame, b"shared words", None)?;
    /// assert_eq!(restored, b"shared words again");
    /// # Ok::<(), mzstd_bindings::MzstdError>(())
    /// ```
    pub fn with_dictionary(dictionary: &[u8], level: Option<i32>) -> Result<Self, MzstdError> {
        Self::open(level, Some(dictionary))
    }

    fn open(level: Option<i32>, dictionary: Option<&[u8]>) -> Result<Self, MzstdError> {
        let session = core::encoder(core::level(level)?, dictionary)?;
        Ok(Self {
            session: Some(session),
        })
    }

    /// Adds `data` to the frame and returns the compressed bytes that are
    /// ready, often none.
    ///
    /// # Errors
    ///
    /// [`MzstdError::StreamClosed`] after `finish` or a failure, and
    /// [`MzstdError::Compression`] when the encoder fails.
    pub fn compress(&mut self, data: &[u8]) -> Result<Vec<u8>, MzstdError> {
        self.run(data, Operation::Continue)
    }

    /// Returns every byte buffered so far, so a reader of the output can
    /// decode all input given up to now; the frame stays open.
    ///
    /// # Errors
    ///
    /// As [`compress`](Self::compress).
    pub fn flush(&mut self) -> Result<Vec<u8>, MzstdError> {
        self.run(&[], Operation::Flush)
    }

    /// Ends the frame and returns its remaining bytes, closing the stream.
    ///
    /// # Errors
    ///
    /// As [`compress`](Self::compress).
    pub fn finish(&mut self) -> Result<Vec<u8>, MzstdError> {
        let output = self.run(&[], Operation::End);
        self.session = None;
        output
    }

    /// Whether the stream still accepts calls.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut stream = mzstd_bindings::StreamCompressor::new(None)?;
    /// assert!(stream.is_open());
    /// stream.finish()?;
    /// assert!(!stream.is_open());
    /// # Ok::<(), mzstd_bindings::MzstdError>(())
    /// ```
    pub fn is_open(&self) -> bool {
        self.session.is_some()
    }

    fn run(&mut self, data: &[u8], operation: Operation) -> Result<Vec<u8>, MzstdError> {
        let session = self.session.as_mut().ok_or(MzstdError::StreamClosed)?;
        let until_finished = operation != Operation::Continue;
        let output = core::pump(data, until_finished, |input, output| {
            let step = core::encode_step(session, input, output, operation)?;
            // A flush is complete once the encoder stops asking for space.
            Ok(core::Step {
                finished: step.finished || (operation == Operation::Flush && !step.needs_output),
                ..step
            })
        });
        if output.is_err() {
            self.session = None;
        }
        output
    }
}

/// Decompresses Zstandard data supplied in pieces.
///
/// Feed input with [`decompress`](Self::decompress), which returns the bytes
/// decoded so far, and call [`finish`](Self::finish) once the input ends to
/// check that the last frame was complete. The input may hold several
/// frames; their content is concatenated.
///
/// After `finish` or a failed call the stream is closed and every further
/// call fails with [`MzstdError::StreamClosed`].
///
/// # Examples
///
/// ```
/// use mzstd_bindings::StreamDecompressor;
///
/// let frame = mzstd_bindings::compress(b"split across calls", None)?;
/// let mut stream = StreamDecompressor::new(None)?;
/// let (head, tail) = frame.split_at(5);
/// let mut restored = stream.decompress(head)?;
/// restored.extend(stream.decompress(tail)?);
/// restored.extend(stream.finish()?);
/// assert_eq!(restored, b"split across calls");
/// # Ok::<(), mzstd_bindings::MzstdError>(())
/// ```
#[derive(Debug)]
pub struct StreamDecompressor {
    /// The open session, or `None` once finished or failed.
    session: Option<DecoderSessionOwned>,
}

impl StreamDecompressor {
    /// A stream that fails once the decoded total exceeds `max_output_size`.
    ///
    /// # Errors
    ///
    /// None today; the `Result` leaves room for limits that need
    /// validation.
    pub fn new(max_output_size: Option<usize>) -> Result<Self, MzstdError> {
        Self::open(None, max_output_size)
    }

    /// A stream for frames written with `dictionary`.
    ///
    /// # Errors
    ///
    /// [`MzstdError::Dictionary`] for a malformed trained dictionary.
    ///
    /// # Examples
    ///
    /// ```
    /// use mzstd_bindings::StreamDecompressor;
    ///
    /// let frame = mzstd_bindings::compress_with_dictionary(b"abcabc", b"abc", None)?;
    /// let mut stream = StreamDecompressor::with_dictionary(b"abc", None)?;
    /// let mut restored = stream.decompress(&frame)?;
    /// restored.extend(stream.finish()?);
    /// assert_eq!(restored, b"abcabc");
    /// # Ok::<(), mzstd_bindings::MzstdError>(())
    /// ```
    pub fn with_dictionary(
        dictionary: &[u8],
        max_output_size: Option<usize>,
    ) -> Result<Self, MzstdError> {
        Self::open(Some(dictionary), max_output_size)
    }

    fn open(dictionary: Option<&[u8]>, max_output_size: Option<usize>) -> Result<Self, MzstdError> {
        Ok(Self {
            session: Some(core::decoder(dictionary, max_output_size)?),
        })
    }

    /// Decodes `data` and returns the bytes that are ready.
    ///
    /// # Errors
    ///
    /// [`MzstdError::StreamClosed`] after `finish` or a failure, and
    /// [`MzstdError::Decompression`] for corrupt input or output past the
    /// limit.
    pub fn decompress(&mut self, data: &[u8]) -> Result<Vec<u8>, MzstdError> {
        self.run(data, DecodeOperation::Process)
    }

    /// Declares the end of the input, returns the remaining bytes and closes
    /// the stream.
    ///
    /// # Errors
    ///
    /// [`MzstdError::Decompression`] when the input stopped inside a frame,
    /// and the errors of [`decompress`](Self::decompress).
    pub fn finish(&mut self) -> Result<Vec<u8>, MzstdError> {
        let output = self.run(&[], DecodeOperation::Finish);
        self.session = None;
        output
    }

    /// Whether the stream still accepts calls.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut stream = mzstd_bindings::StreamDecompressor::new(None)?;
    /// assert!(stream.is_open());
    /// assert!(stream.finish().is_err()); // no frame was given
    /// assert!(!stream.is_open());
    /// # Ok::<(), mzstd_bindings::MzstdError>(())
    /// ```
    pub fn is_open(&self) -> bool {
        self.session.is_some()
    }

    fn run(&mut self, data: &[u8], operation: DecodeOperation) -> Result<Vec<u8>, MzstdError> {
        let session = self.session.as_mut().ok_or(MzstdError::StreamClosed)?;
        let until_finished = operation == DecodeOperation::Finish;
        let output = core::pump(data, until_finished, |input, output| {
            core::decode_step(session, input, output, operation)
        });
        if output.is_err() {
            self.session = None;
        }
        output
    }
}
