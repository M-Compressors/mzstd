//! A [`Read`] adapter over an encoder session.

use std::io::{ErrorKind, Read, Result};

use crate::compressor::session::{EncoderSession, EncoderStatus, Operation};

/// Size of the input staging buffer.
const INPUT_CHUNK: usize = 128 * 1024;

/// Yields the compressed frame of everything read from an inner reader.
///
/// # Examples
///
/// ```
/// use mzstd::Compressor;
/// use std::io::Read;
///
/// let mut encoder = Compressor::new(Default::default())?;
/// let mut reader = encoder.reader(&b"pulled payload pulled payload"[..], Default::default());
/// let mut frame = Vec::new();
/// reader.read_to_end(&mut frame)?;
/// assert!(!frame.is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct EncoderReader<'c, R: Read> {
    session: EncoderSession<'c>,
    source: R,
    input: Vec<u8>,
    /// Bytes of `input` not yet consumed by the session.
    available: core::ops::Range<usize>,
    source_done: bool,
}

impl<R: Read> std::fmt::Debug for EncoderReader<'_, R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncoderReader")
            .field("session", &self.session)
            .field("source_done", &self.source_done)
            .finish_non_exhaustive()
    }
}

impl<'c, R: Read> EncoderReader<'c, R> {
    pub(crate) fn new(session: EncoderSession<'c>, source: R) -> Self {
        Self {
            session,
            source,
            input: vec![0; INPUT_CHUNK],
            available: 0..0,
            source_done: false,
        }
    }

    /// The inner reader.
    #[must_use]
    pub const fn get_ref(&self) -> &R {
        &self.source
    }
}

impl<R: Read> Read for EncoderReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if buf.is_empty() || self.session.is_finished() {
            return Ok(0);
        }
        loop {
            if self.available.is_empty() && !self.source_done {
                match self.source.read(&mut self.input) {
                    Ok(0) => self.source_done = true,
                    Ok(n) => self.available = 0..n,
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                }
            }
            let operation = if self.source_done {
                Operation::End
            } else {
                Operation::Continue
            };
            let progress = self
                .session
                .process(&self.input[self.available.clone()], buf, operation)
                .map_err(std::io::Error::other)?;
            self.available.start += progress.consumed;
            if progress.produced > 0 || progress.status == EncoderStatus::Finished {
                return Ok(progress.produced);
            }
        }
    }
}
