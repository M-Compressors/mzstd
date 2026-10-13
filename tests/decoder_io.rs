#![cfg(feature = "decompression")]
//! The `std::io` adapters: pull decoding through `Read`, push decoding
//! through `Write`, and their error and retry behavior.

#![cfg(not(feature = "no_std"))]

mod decoder_support;

use std::io::{self, Cursor, ErrorKind, Read, Write};

use decoder_support::{compress, params, text};
use mzstd::{DecodeError, Decompressor, FrameMode};

/// A source that hands out at most `step` bytes per read and fails with
/// `Interrupted` and then a retryable error before every other read.
struct Flaky {
    data: Vec<u8>,
    position: usize,
    step: usize,
    calls: usize,
}

impl Read for Flaky {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.calls += 1;
        match self.calls % 4 {
            1 => return Err(ErrorKind::Interrupted.into()),
            2 => return Err(ErrorKind::WouldBlock.into()),
            _ => {}
        }
        let count = self
            .step
            .min(buffer.len())
            .min(self.data.len() - self.position);
        buffer[..count].copy_from_slice(&self.data[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}

/// A sink that accepts a few bytes per write and refuses every other call.
struct Stubborn {
    written: Vec<u8>,
    calls: usize,
    zero: bool,
}

impl Write for Stubborn {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.zero {
            return Ok(0);
        }
        match self.calls % 3 {
            0 => Err(ErrorKind::WouldBlock.into()),
            1 => Err(ErrorKind::Interrupted.into()),
            _ => {
                let count = buffer.len().min(1000);
                self.written.extend_from_slice(&buffer[..count]);
                Ok(count)
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn decode_error(error: &io::Error) -> Option<&DecodeError> {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<DecodeError>())
}

#[test]
fn readers_decode_through_small_reads_and_retry_source_errors() {
    let input = text(120_000, 1);
    let frame = compress(&input, &params(3, true, true));
    let mut decoder = Decompressor::default();
    let mut reader = decoder.reader(Flaky {
        data: frame.clone(),
        position: 0,
        step: 333,
        calls: 0,
    });
    let mut decoded = Vec::new();
    let mut buffer = [0u8; 97];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => decoded.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => panic!("unexpected {error}"),
        }
    }
    assert_eq!(decoded, input);
    assert!(reader.is_finished());
    assert_eq!(reader.get_ref().position, frame.len());
    reader.get_mut().step = 1;
    assert_eq!(reader.read(&mut []).expect("empty read"), 0);
    assert_eq!(reader.read(&mut buffer).expect("at the end"), 0);
}

#[test]
fn readers_return_output_before_a_deferred_error() {
    let input = text(30_000, 2);
    let mut frame = compress(&input, &params(3, true, true));
    let last = frame.len() - 1;
    frame[last] ^= 0xff;
    let mut decoder = Decompressor::default();
    let mut reader = decoder.reader(&frame[..]);
    let mut decoded = Vec::new();
    let error = reader.read_to_end(&mut decoded).expect_err("bad checksum");
    assert!(matches!(
        decode_error(&error),
        Some(DecodeError::ChecksumMismatch { .. })
    ));
    assert_eq!(error.kind(), ErrorKind::InvalidData);
    assert_eq!(decoded, input);
    assert!(!reader.is_finished());
    let error = reader.read(&mut [0u8; 4]).expect_err("failed");
    assert!(matches!(
        decode_error(&error),
        Some(DecodeError::InvalidState)
    ));

    // An error found before any output is returned at once.
    let mut reader = decoder.reader(&b"garbage!"[..]);
    let error = reader.read(&mut [0u8; 4]).expect_err("bad magic");
    assert!(matches!(
        decode_error(&error),
        Some(DecodeError::InvalidData { .. })
    ));
    // A deferred error is kept by `into_parts`.
    let small = compress(b"tiny", &params(1, true, true));
    let mut broken = small.clone();
    let last = broken.len() - 1;
    broken[last] ^= 1;
    let mut reader = decoder.reader(&broken[..]);
    let mut buffer = [0u8; 16];
    assert_eq!(reader.read(&mut buffer).expect("output first"), 4);
    let parts = reader.into_parts();
    assert!(!parts.finished);
    assert!(parts.pending_error.is_some());
    // Truncated input reports an unexpected end.
    let mut reader = decoder.reader(&small[..small.len() - 2]);
    let error = reader.read_to_end(&mut Vec::new()).expect_err("truncated");
    assert_eq!(error.kind(), ErrorKind::UnexpectedEof);
}

#[test]
fn single_frame_readers_hand_back_the_unread_suffix() {
    let frame = compress(b"payload", &params(1, false, true));
    let mut input = frame.clone();
    input.extend_from_slice(b"protocol suffix");
    let mut decoder = Decompressor::new(FrameMode::Single.into());
    let mut reader = decoder.reader(Cursor::new(input));
    let mut decoded = Vec::new();
    reader.read_to_end(&mut decoded).expect("valid");
    assert_eq!(decoded, b"payload");
    let parts = reader.into_parts();
    assert!(parts.finished);
    assert!(parts.pending_error.is_none());
    let mut rest = Vec::new();
    Cursor::new(parts.unread_input)
        .chain(parts.source)
        .read_to_end(&mut rest)
        .expect("readable");
    assert_eq!(rest, b"protocol suffix");
}

#[test]
fn writers_decode_and_survive_sink_errors() {
    let input = text(100_000, 3);
    let frame = compress(&input, &params(5, true, true));
    let mut decoder = Decompressor::default();
    let mut writer = decoder.writer(Stubborn {
        written: Vec::new(),
        calls: 0,
        zero: false,
    });
    let mut position = 0;
    while position < frame.len() {
        let end = (position + 4_000).min(frame.len());
        match writer.write(&frame[position..end]) {
            Ok(count) => position += count,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => panic!("unexpected {error}"),
        }
    }
    assert!(!writer.is_finished());
    let mut retries = 0;
    let sink = loop {
        match writer.finish() {
            Ok(sink) => break sink,
            Err(failure) => {
                assert_eq!(failure.error().kind(), ErrorKind::WouldBlock);
                writer = failure.into_inner();
                // While finishing, `flush` retries the finish too.
                retries += 1;
                if retries % 2 == 0 {
                    let _ = writer.flush();
                }
            }
        }
    };
    assert!(retries > 0);
    assert_eq!(sink.written, input);
}

#[test]
fn writers_report_truncation_trailing_data_and_dead_sinks() {
    let frame = compress(b"0123456789", &params(1, true, true));
    let mut decoder = Decompressor::default();
    let mut writer = decoder.writer(Vec::new());
    writer
        .write_all(&frame[..frame.len() - 1])
        .expect("accepted");
    let error = writer.try_finish().expect_err("truncated");
    assert!(matches!(
        decode_error(&error),
        Some(DecodeError::UnexpectedEndOfInput)
    ));
    let error = writer.write(b"more").expect_err("failed");
    assert!(matches!(
        decode_error(&error),
        Some(DecodeError::InvalidState)
    ));
    drop(writer);

    // Single mode: bytes after the frame are trailing data.
    let mut single = Decompressor::new(FrameMode::Single.into());
    let mut writer = single.writer(Vec::new());
    let mut input = frame.clone();
    input.extend_from_slice(b"xx");
    let accepted = writer.write(&input).expect("the frame");
    assert_eq!(accepted, frame.len());
    let error = writer.write(b"xx").expect_err("trailing");
    assert!(matches!(
        decode_error(&error),
        Some(DecodeError::TrailingData { .. })
    ));
    writer.try_finish().expect("finished");
    writer.try_finish().expect("idempotent");
    assert!(writer.is_finished());
    assert_eq!(writer.get_ref().as_slice(), b"0123456789");
    writer.get_mut().clear();
    let error = writer.write(b"x").expect_err("finished");
    assert!(matches!(
        decode_error(&error),
        Some(DecodeError::InvalidState)
    ));
    assert_eq!(writer.write(&[]).expect("empty"), 0);
    drop(writer);

    // A sink that accepts nothing.
    let mut writer = decoder.writer(Stubborn {
        written: Vec::new(),
        calls: 0,
        zero: true,
    });
    // The input is accepted; the sink's failure surfaces on the next call.
    writer
        .write_all(&frame)
        .expect("accepted before the sink fails");
    let failure = writer.finish().map(|_| ()).expect_err("dead sink");
    assert_eq!(failure.into_error().kind(), ErrorKind::WriteZero);

    // `flush` delivers what was decoded without ending the input.
    let input = text(100_000, 4);
    let large = compress(&input, &params(3, false, true));
    let mut writer = decoder.writer(Vec::new());
    writer.write_all(&large).expect("accepted");
    assert!(writer.get_ref().len() < input.len());
    writer.flush().expect("flushes");
    assert_eq!(writer.get_ref(), &input);
    assert!(!writer.is_finished());
    drop(writer);

    // Flushing after `try_finish` started completes the finish.
    let mut writer = decoder.writer(Vec::new());
    writer.write_all(&frame).expect("accepted");
    writer.try_finish().expect("finished");
    writer.flush().expect("flushes");
    let content = writer
        .finish()
        .map_err(mzstd::io::FinishError::into_error)
        .expect("done");
    assert_eq!(content, b"0123456789");
}
