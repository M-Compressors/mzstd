#![cfg(feature = "compression")]
#![cfg(not(feature = "no_std"))]
//! The `std::io` compression adapters: pull encoding through `Read`, push
//! encoding through `Write`, and recovery from failing sinks through
//! `FinishError`.

use std::cell::Cell;
use std::error::Error as _;
use std::io::{self, ErrorKind, Read, Write};
use std::rc::Rc;

use mzstd::io::FinishError;
use mzstd::{Compressor, EncodeError, EncoderConfig, InputSize, Level};
use zstd_ffi::corpus;
use zstd_ffi::oracle::{self, Directive, Params};

/// How many bytes the reader takes from its source at a time.
const READER_CHUNK: usize = 128 * 1024;

/// A source that is interrupted before every read and hands out at most
/// `step` bytes per read.
struct Interrupted {
    data: Vec<u8>,
    position: usize,
    step: usize,
    interrupt: bool,
}

impl Read for Interrupted {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.interrupt = !self.interrupt;
        if self.interrupt {
            return Err(ErrorKind::Interrupted.into());
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

/// A source that always fails.
struct Broken;

impl Read for Broken {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(ErrorKind::BrokenPipe, "source is gone"))
    }
}

/// A sink that refuses its next `refusals` writes with `kind`, then accepts
/// at most `step` bytes per write. The refusal count is shared so a test can
/// arm it while a writer owns the sink.
struct Sink {
    written: Vec<u8>,
    refusals: Rc<Cell<usize>>,
    kind: ErrorKind,
    step: usize,
    flushes: usize,
}

impl Sink {
    fn healthy() -> Self {
        Self::refusing(0, ErrorKind::WouldBlock)
    }

    fn refusing(refusals: usize, kind: ErrorKind) -> Self {
        Self {
            written: Vec::new(),
            refusals: Rc::new(Cell::new(refusals)),
            kind,
            step: usize::MAX,
            flushes: 0,
        }
    }
}

impl Write for Sink {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.refusals.get() > 0 {
            self.refusals.set(self.refusals.get() - 1);
            return Err(io::Error::new(self.kind, "not yet"));
        }
        let count = buffer.len().min(self.step);
        self.written.extend_from_slice(&buffer[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

/// A sink that accepts nothing.
struct Full;

impl Write for Full {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Ok(0)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The reference's frame for `chunks` handed over as `Continue` steps.
fn reference_stream(params: &Params, chunks: &[&[u8]]) -> Vec<u8> {
    let steps: Vec<(&[u8], Directive)> = chunks
        .iter()
        .map(|&chunk| (chunk, Directive::Continue))
        .collect();
    oracle::compress_stream(params, &steps).expect("reference")
}

fn encode_error(error: &io::Error) -> Option<&EncodeError> {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<EncodeError>())
}

/// Writes `input` in `chunk`-byte pieces into a writer over `sink` and
/// returns the adapter's first `finish` result.
fn write_and_finish(
    encoder: &mut Compressor,
    sink: Sink,
    input: &[u8],
    chunk: usize,
) -> Result<Sink, io::Error> {
    let mut writer = encoder.writer(sink, Default::default());
    for piece in input.chunks(chunk) {
        writer.write_all(piece)?;
    }
    writer.finish().map_err(FinishError::into_error)
}

#[test]
fn writers_match_the_reference_stream() {
    let input = corpus::text(400_000, 1);
    let mut encoder = Compressor::default();
    let sink = write_and_finish(&mut encoder, Sink::healthy(), &input, 50_000).expect("finishes");
    let chunks: Vec<&[u8]> = input.chunks(50_000).collect();
    assert_eq!(sink.written, reference_stream(&Params::level(3), &chunks));
    assert!(sink.flushes > 0);
}

#[test]
fn writers_with_a_declared_size_write_the_one_shot_frame() {
    let input = corpus::text(60_000, 2);
    let mut encoder = Compressor::default();
    let expected = encoder.compress(&input).expect("compresses");
    let mut writer = encoder.writer(Vec::new(), InputSize::from(input.len() as u64).into());
    writer.write_all(&input).expect("accepts");
    let frame = writer
        .finish()
        .map_err(FinishError::into_error)
        .expect("finishes");
    assert_eq!(frame, expected);
}

#[test]
fn writer_flushes_match_reference_flushes() {
    let input = corpus::text(120_000, 3);
    let (head, tail) = input.split_at(70_000);
    let mut encoder = Compressor::default();
    let mut writer = encoder.writer(Vec::new(), Default::default());
    writer.write_all(head).expect("accepts");
    writer.flush().expect("flushes");
    let flushed = writer.get_ref().len();
    assert!(flushed > 0);
    writer.write_all(tail).expect("accepts");
    let frame = writer
        .finish()
        .map_err(FinishError::into_error)
        .expect("finishes");

    let steps: [(&[u8], Directive); 3] = [
        (head, Directive::Continue),
        (&[], Directive::Flush),
        (tail, Directive::Continue),
    ];
    let reference = oracle::compress_stream(&Params::level(3), &steps).expect("reference");
    assert_eq!(frame, reference);
    let decoded = oracle::decompress_stream(&frame[..flushed], 4096, None);
    assert!(decoded.is_err(), "a flush does not end the frame");
}

#[test]
fn writers_deliver_through_sinks_that_take_a_few_bytes_at_a_time() {
    let input = corpus::text(50_000, 4);
    let mut sink = Sink::healthy();
    sink.step = 7;
    let mut encoder = Compressor::default();
    let written = write_and_finish(&mut encoder, sink, &input, 10_000).expect("finishes");
    let chunks: Vec<&[u8]> = input.chunks(10_000).collect();
    assert_eq!(
        written.written,
        reference_stream(&Params::level(3), &chunks)
    );
}

#[test]
fn writers_retry_interrupted_sinks() {
    let input = corpus::text(20_000, 5);
    let mut encoder = Compressor::default();
    let sink = write_and_finish(
        &mut encoder,
        Sink::refusing(3, ErrorKind::Interrupted),
        &input,
        20_000,
    )
    .expect("interruptions are retried");
    let reference = reference_stream(&Params::level(3), &[&input]);
    assert_eq!(sink.written, reference);
}

#[test]
fn writers_reject_writes_after_the_frame_ended() {
    let mut encoder = Compressor::default();
    let mut writer = encoder.writer(Vec::new(), Default::default());
    writer.write_all(b"payload").expect("accepts");
    writer.try_finish().expect("finishes");
    let frame = writer.get_ref().clone();
    let error = writer.write(b"late").expect_err("finished");
    assert_eq!(error.kind(), ErrorKind::Other);
    assert_eq!(error.to_string(), "the frame has already been finished");
    writer.try_finish().expect("finishing again is a no-op");
    writer
        .flush()
        .expect("flushing a finished frame is a no-op");
    assert_eq!(writer.get_ref(), &frame);
    assert_eq!(oracle::decompress(&frame, 64).expect("decodes"), b"payload");
}

#[test]
fn writers_report_a_sink_that_accepts_nothing() {
    let mut encoder = Compressor::default();
    let mut writer = encoder.writer(Full, Default::default());
    writer
        .write_all(b"payload")
        .expect("buffered by the session");
    let failure = writer.finish().map(|_| ()).expect_err("full sink");
    assert_eq!(failure.error().kind(), ErrorKind::WriteZero);
    assert_eq!(failure.error().to_string(), "the sink accepted no bytes");
}

#[test]
fn writers_report_input_short_of_the_declared_size() {
    let mut encoder = Compressor::default();
    let mut writer = encoder.writer(Vec::new(), InputSize::Exact(100).into());
    writer.write_all(b"short").expect("accepts");
    let failure = writer.finish().map(|_| ()).expect_err("too short");
    assert_eq!(
        encode_error(failure.error()),
        Some(&EncodeError::InputSizeMismatch { declared: 100 })
    );
}

#[test]
fn writers_debug_output_names_the_adapter() {
    let mut encoder = Compressor::default();
    let writer = encoder.writer(Vec::new(), Default::default());
    let debug = format!("{writer:?}");
    assert!(debug.starts_with("EncoderWriter"), "{debug}");
    assert!(debug.contains("pending: 0"), "{debug}");
}

/// A writer over a healthy sink holding `input`, and the handle that makes
/// the sink refuse its next writes.
fn armed_writer<'c>(
    encoder: &'c mut Compressor,
    input: &[u8],
) -> (mzstd::io::EncoderWriter<'c, Sink>, Rc<Cell<usize>>) {
    let sink = Sink::healthy();
    let refusals = Rc::clone(&sink.refusals);
    let mut writer = encoder.writer(sink, Default::default());
    writer.write_all(input).expect("accepts");
    (writer, refusals)
}

#[test]
fn a_failed_finish_hands_back_a_writer_that_completes_the_frame() {
    let input = corpus::text(30_000, 6);
    let mut encoder = Compressor::default();
    let (mut writer, refusals) = armed_writer(&mut encoder, &input);
    refusals.set(2);
    let mut failures = 0;
    let sink = loop {
        match writer.finish() {
            Ok(sink) => break sink,
            Err(failure) => {
                assert_eq!(failure.error().kind(), ErrorKind::WouldBlock);
                failures += 1;
                writer = failure.into_inner();
            }
        }
    };
    assert_eq!(failures, 2);
    assert_eq!(sink.written, reference_stream(&Params::level(3), &[&input]));
}

#[test]
fn try_finish_can_be_retried_after_a_sink_failure() {
    let input = corpus::text(30_000, 7);
    let mut encoder = Compressor::default();
    let (mut writer, refusals) = armed_writer(&mut encoder, &input);
    refusals.set(1);
    let error = writer.try_finish().expect_err("refused");
    assert_eq!(error.kind(), ErrorKind::WouldBlock);
    assert!(writer.get_ref().written.is_empty());
    writer.try_finish().expect("retried");
    assert_eq!(
        writer.get_ref().written,
        reference_stream(&Params::level(3), &[&input])
    );
}

#[test]
fn finish_errors_split_into_the_failure_and_the_writer() {
    let input = corpus::text(30_000, 8);
    let mut encoder = Compressor::default();
    let (writer, refusals) = armed_writer(&mut encoder, &input);
    refusals.set(1);
    let failure = writer.finish().map(|_| ()).expect_err("refused");
    let (error, writer) = failure.into_parts();
    assert_eq!(error.kind(), ErrorKind::WouldBlock);
    assert_eq!(error.to_string(), "not yet");
    let sink = writer
        .finish()
        .map_err(FinishError::into_error)
        .expect("retried");
    assert_eq!(sink.written, reference_stream(&Params::level(3), &[&input]));
}

#[test]
fn finish_errors_display_and_expose_the_sink_failure() {
    let mut encoder = Compressor::default();
    let (writer, refusals) = armed_writer(&mut encoder, b"payload");
    refusals.set(1);
    let failure = writer.finish().map(|_| ()).expect_err("refused");
    assert_eq!(
        failure.to_string(),
        "the stream could not be finished: not yet"
    );
    let source = failure.source().expect("the sink failure");
    let source = source.downcast_ref::<io::Error>().expect("an io::Error");
    assert_eq!(source.kind(), ErrorKind::WouldBlock);
    let debug = format!("{failure:?}");
    assert!(debug.starts_with("FinishError"), "{debug}");
    assert!(debug.contains("WouldBlock"), "{debug}");
}

#[test]
fn finish_errors_convert_into_the_sink_failure() {
    let mut encoder = Compressor::default();
    let (writer, refusals) = armed_writer(&mut encoder, b"payload");
    refusals.set(1);
    let failure = writer.finish().map(|_| ()).expect_err("refused");
    let error = io::Error::from(failure);
    assert_eq!(error.kind(), ErrorKind::WouldBlock);
    assert_eq!(error.to_string(), "not yet");
}

#[test]
fn readers_match_the_reference_stream() {
    let input = corpus::text(400_000, 9);
    let mut encoder = Compressor::default();
    let mut frame = Vec::new();
    encoder
        .reader(input.as_slice(), Default::default())
        .read_to_end(&mut frame)
        .expect("reads");
    let chunks: Vec<&[u8]> = input.chunks(READER_CHUNK).collect();
    assert_eq!(frame, reference_stream(&Params::level(3), &chunks));
}

#[test]
fn readers_with_a_declared_size_write_the_one_shot_frame() {
    let input = corpus::text(60_000, 10);
    let config = EncoderConfig::default().with_level(Level::try_from(9).expect("valid"));
    let mut encoder = Compressor::new(config).expect("valid");
    let expected = encoder.compress(&input).expect("compresses");
    let mut frame = Vec::new();
    encoder
        .reader(input.as_slice(), InputSize::from(input.len() as u64).into())
        .read_to_end(&mut frame)
        .expect("reads");
    assert_eq!(frame, expected);
}

#[test]
fn readers_deliver_through_small_reads() {
    let input = corpus::text(100_000, 11);
    let mut encoder = Compressor::default();
    let mut reader = encoder.reader(input.as_slice(), Default::default());
    let mut frame = Vec::new();
    let mut buffer = [0u8; 13];
    loop {
        let read = reader.read(&mut buffer).expect("reads");
        if read == 0 {
            break;
        }
        frame.extend_from_slice(&buffer[..read]);
    }
    assert_eq!(frame, reference_stream(&Params::level(3), &[&input]));
}

#[test]
fn readers_retry_interrupted_sources() {
    let input = corpus::text(50_000, 12);
    let source = Interrupted {
        data: input.clone(),
        position: 0,
        step: 20_000,
        interrupt: false,
    };
    let mut encoder = Compressor::default();
    let mut frame = Vec::new();
    encoder
        .reader(source, Default::default())
        .read_to_end(&mut frame)
        .expect("interruptions are retried");
    let chunks: Vec<&[u8]> = input.chunks(20_000).collect();
    assert_eq!(frame, reference_stream(&Params::level(3), &chunks));
}

#[test]
fn readers_report_source_failures() {
    let mut encoder = Compressor::default();
    let mut reader = encoder.reader(Broken, Default::default());
    let error = reader.read(&mut [0u8; 64]).expect_err("broken source");
    assert_eq!(error.kind(), ErrorKind::BrokenPipe);
    assert_eq!(error.to_string(), "source is gone");
}

#[test]
fn readers_report_input_beyond_the_declared_size() {
    let mut encoder = Compressor::default();
    let mut reader = encoder.reader(&b"more than four"[..], InputSize::Exact(4).into());
    let error = reader.read_to_end(&mut Vec::new()).expect_err("too long");
    assert_eq!(
        encode_error(&error),
        Some(&EncodeError::InputSizeMismatch { declared: 4 })
    );
}

#[test]
fn readers_return_nothing_for_an_empty_buffer_or_after_the_frame() {
    let mut encoder = Compressor::default();
    let mut reader = encoder.reader(&b"payload"[..], Default::default());
    assert_eq!(reader.read(&mut []).expect("empty buffer"), 0);
    let mut frame = Vec::new();
    reader.read_to_end(&mut frame).expect("reads");
    assert_eq!(reader.read(&mut [0u8; 64]).expect("finished"), 0);
    assert_eq!(oracle::decompress(&frame, 64).expect("decodes"), b"payload");
}

#[test]
fn readers_expose_their_source() {
    let mut encoder = Compressor::default();
    let source: &[u8] = b"payload";
    let mut reader = encoder.reader(source, Default::default());
    assert_eq!(*reader.get_ref(), b"payload");
    reader.read_to_end(&mut Vec::new()).expect("reads");
    assert!(reader.get_ref().is_empty());
}

#[test]
fn reader_parts_return_the_unaccepted_read_ahead() {
    let input = corpus::binary(1 << 20, 21);
    let mut encoder = Compressor::default();
    let mut reader = encoder.reader(io::Cursor::new(input.clone()), Default::default());
    // Undelivered output stops the session from taking the second chunk.
    let mut byte = [0u8; 1];
    assert_eq!(reader.read(&mut byte).expect("reads"), 1);
    assert_eq!(reader.read(&mut byte).expect("reads"), 1);
    let position = reader.get_ref().position() as usize;
    let parts = reader.into_parts();
    assert!(!parts.unread_input.is_empty());
    let accepted = position - parts.unread_input.len();
    assert_eq!(parts.unread_input, input[accepted..position]);
    let mut rest = Vec::new();
    io::Cursor::new(parts.unread_input)
        .chain(parts.source)
        .read_to_end(&mut rest)
        .expect("reads");
    assert_eq!(rest, input[accepted..]);
}

#[test]
fn readers_debug_output_names_the_adapter() {
    let mut encoder = Compressor::default();
    let reader = encoder.reader(&b""[..], Default::default());
    let debug = format!("{reader:?}");
    assert!(debug.starts_with("EncoderReader"), "{debug}");
    assert!(debug.contains("source_done: false"), "{debug}");
}
