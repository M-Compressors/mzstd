#![cfg(feature = "compression")]
#![cfg(not(feature = "no_std"))]
//! The `std::io` adapters over parallel sessions: byte identity with the
//! batch, blocking on executors, transactional sinks and read-ahead.

use std::io::{self, Cursor, ErrorKind, Read, Write};
use std::sync::Arc;

use mzstd::compressor::parallel::{
    ParallelCompressor, ParallelConfig, ParallelEncodeError, ParallelTaskQueue, SegmentSize,
    TaskCount,
};
use mzstd::io::FinishError;
use mzstd::{EncoderConfig, InputSize, Level, StreamConfig};
use zstd_ffi::corpus;

const SEGMENT: usize = 64 << 10;

fn compressor(level: i32) -> ParallelCompressor {
    let config = EncoderConfig::default()
        .with_level(Level::try_from(level).unwrap())
        .with_checksum(true);
    let parallel = ParallelConfig::default().with_segment_size(SegmentSize::MIN);
    ParallelCompressor::new(config, parallel).unwrap()
}

fn window(count: usize) -> TaskCount {
    TaskCount::try_from(count).unwrap()
}

fn exact(input: &[u8]) -> StreamConfig {
    InputSize::Exact(input.len() as u64).into()
}

/// Runs `executors` threads over `queue` for the duration of `body`.
fn with_executors<T>(queue: ParallelTaskQueue, executors: usize, body: impl FnOnce() -> T) -> T {
    std::thread::scope(|scope| {
        for _ in 0..executors {
            let queue = queue.clone();
            scope.spawn(move || queue.into_iter().for_each(|task| task.run()));
        }
        drop(queue);
        body()
    })
}

fn decode(frame: &[u8]) -> Vec<u8> {
    zstd_ffi::oracle::decompress_stream(frame, 4096, None).unwrap()
}

/// A sink that refuses every other write with `kind` and takes at most
/// `step` bytes per write.
struct Flaky {
    written: Vec<u8>,
    refuse: bool,
    kind: ErrorKind,
    step: usize,
    flushes: usize,
}

impl Flaky {
    fn new(kind: ErrorKind, step: usize) -> Self {
        Self {
            written: Vec::new(),
            refuse: false,
            kind,
            step,
            flushes: 0,
        }
    }
}

impl Write for Flaky {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.refuse = !self.refuse;
        if self.refuse {
            return Err(self.kind.into());
        }
        let count = buf.len().min(self.step);
        self.written.extend_from_slice(&buf[..count]);
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

/// A source interrupted before every read, failing once with `BrokenPipe`
/// after `fail_at` bytes, and handing out at most `step` bytes per read.
struct Unreliable {
    data: Vec<u8>,
    position: usize,
    step: usize,
    interrupt: bool,
    fail_at: Option<usize>,
}

impl Unreliable {
    fn new(data: Vec<u8>, step: usize) -> Self {
        Self {
            data,
            position: 0,
            step,
            interrupt: false,
            fail_at: None,
        }
    }
}

impl Read for Unreliable {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.interrupt = !self.interrupt;
        if self.interrupt {
            return Err(ErrorKind::Interrupted.into());
        }
        if self.fail_at.is_some_and(|at| self.position >= at) {
            self.fail_at = None;
            return Err(io::Error::new(ErrorKind::BrokenPipe, "source hiccup"));
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

fn parallel_error(error: &io::Error) -> &ParallelEncodeError {
    error
        .get_ref()
        .and_then(|error| error.downcast_ref::<ParallelEncodeError>())
        .unwrap()
}

#[test]
fn writers_with_a_declared_size_write_the_batch_frame_however_writes_split() {
    let input = corpus::mixed(5 * SEGMENT + 321, 1);
    let mut compressor = compressor(3);
    let batch = compressor.compress(&input).unwrap();
    for (piece, window_size) in [(1000, 1), (SEGMENT + 7, 2), (input.len(), 8)] {
        let (mut sink, queue) = compressor.writer(Vec::new(), exact(&input), window(window_size));
        let frame = with_executors(queue, 2, || {
            for chunk in input.chunks(piece) {
                sink.write_all(chunk).unwrap();
            }
            sink.finish().map_err(FinishError::into_error).unwrap()
        });
        assert_eq!(frame, batch, "pieces of {piece}, window {window_size}");
    }
}

#[test]
fn writers_without_a_declared_size_decode() {
    let input = corpus::text(3 * SEGMENT + 5, 2);
    let mut compressor = compressor(1);
    let (mut sink, queue) = compressor.writer(Vec::new(), StreamConfig::default(), window(2));
    let frame = with_executors(queue, 1, || {
        sink.write_all(&input).unwrap();
        sink.finish().map_err(FinishError::into_error).unwrap()
    });
    assert_eq!(decode(&frame), input);
}

#[test]
fn writer_flushes_make_everything_written_decodable() {
    let input = corpus::text(2 * SEGMENT, 3);
    let mut compressor = compressor(3);
    let (mut sink, queue) = compressor.writer(Vec::new(), StreamConfig::default(), window(2));
    with_executors(queue, 1, || {
        sink.write_all(&input[..1000]).unwrap();
        sink.flush().unwrap();
        let flushed = sink.get_ref().len();
        // Flushing again with nothing written emits nothing.
        sink.flush().unwrap();
        assert_eq!(sink.get_ref().len(), flushed);
        sink.write_all(&input[1000..]).unwrap();
        sink.try_finish().unwrap();
    });
    assert!(sink.is_finished());
    assert_eq!(decode(sink.get_ref()), input);
}

#[test]
fn writers_lose_nothing_to_short_writes_and_refusals() {
    let input = corpus::mixed(3 * SEGMENT + 99, 4);
    let mut compressor = compressor(3);
    let batch = compressor.compress(&input).unwrap();
    let (mut sink, queue) = compressor.writer(
        Flaky::new(ErrorKind::Interrupted, 1000),
        exact(&input),
        window(2),
    );
    with_executors(queue, 2, || {
        for chunk in input.chunks(20_000) {
            sink.write_all(chunk).unwrap();
        }
        // After the last write, a flush moves no segment boundary.
        sink.flush().unwrap();
        sink.try_finish().unwrap();
    });
    assert_eq!(sink.get_ref().written, batch);
    assert!(sink.get_ref().flushes >= 2);
}

#[test]
fn a_failed_finish_hands_back_a_writer_that_completes_the_frame() {
    let input = corpus::text(2 * SEGMENT + 1, 5);
    let mut compressor = compressor(1);
    let batch = compressor.compress(&input).unwrap();
    let (mut sink, queue) = compressor.writer(
        Flaky::new(ErrorKind::WouldBlock, usize::MAX),
        exact(&input),
        window(2),
    );
    let frame = with_executors(queue, 1, || {
        // The first write is accepted; the refused delivery waits.
        assert_eq!(sink.write(&input).unwrap(), 2 * SEGMENT + 1);
        let mut attempts = 0;
        let mut sink = sink;
        let written = loop {
            attempts += 1;
            match sink.finish() {
                Ok(inner) => break inner.written,
                Err(failure) => {
                    assert_eq!(failure.error().kind(), ErrorKind::WouldBlock);
                    sink = failure.into_inner();
                }
            }
        };
        assert!(attempts > 1);
        written
    });
    assert_eq!(frame, batch);
}

#[test]
fn writers_reject_writes_after_the_frame_ended() {
    let mut compressor = compressor(1);
    let serial = mzstd::Compressor::new(*compressor.config())
        .unwrap()
        .compress(b"");
    let (mut sink, _queue) = compressor.writer(Vec::new(), StreamConfig::default(), window(1));
    // An empty frame is compressed inline, so no executor is needed.
    sink.try_finish().unwrap();
    sink.try_finish().unwrap();
    sink.flush().unwrap();
    let error = sink.write(b"late").unwrap_err();
    assert_eq!(error.to_string(), "the frame has already been finished");
    assert_eq!(sink.write(b"").unwrap_err().kind(), ErrorKind::Other);
    assert_eq!(sink.get_ref(), &serial.unwrap());
}

#[test]
fn writers_accept_empty_writes_without_a_task() {
    let mut compressor = compressor(1);
    let (mut sink, _queue) = compressor.writer(Vec::new(), StreamConfig::default(), window(1));
    assert_eq!(sink.write(b"").unwrap(), 0);
    sink.get_mut().extend_from_slice(b"");
    assert!(!sink.is_finished());
}

#[test]
fn writers_report_a_sink_that_accepts_nothing() {
    let mut compressor = compressor(1);
    let (mut sink, _queue) = compressor.writer(Full, StreamConfig::default(), window(1));
    let error = sink.try_finish().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::WriteZero);
}

#[test]
fn writers_fail_instead_of_blocking_when_every_queue_handle_is_gone() {
    let input = corpus::text(3 * SEGMENT, 6);
    let mut compressor = compressor(1);
    let (mut sink, queue) = compressor.writer(Vec::new(), StreamConfig::default(), window(1));
    drop(queue);
    let error = sink.write_all(&input).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Other);
    let ParallelEncodeError::StreamFailed(cause) = parallel_error(&error) else {
        panic!("{error:?}");
    };
    assert_eq!(**cause, ParallelEncodeError::TaskQueueClosed);
}

#[test]
fn writer_flushes_and_finishes_wait_for_abandoned_tasks_to_report() {
    let mut compressor = compressor(1);
    let (mut sink, queue) = compressor.writer(Vec::new(), StreamConfig::default(), window(1));
    sink.write_all(b"queued, never run").unwrap();
    let held = queue.clone();
    drop(queue);
    std::thread::scope(|scope| {
        scope.spawn(move || {
            // Dropping the task unrun abandons it.
            drop(held.recv());
        });
        let error = sink.flush().unwrap_err();
        let ParallelEncodeError::StreamFailed(cause) = parallel_error(&error) else {
            panic!("{error:?}");
        };
        assert_eq!(**cause, ParallelEncodeError::TaskAbandoned { segment: 0 });
    });
    let failure = sink.finish().unwrap_err();
    assert!(matches!(
        parallel_error(failure.error()),
        ParallelEncodeError::StreamFailed(_)
    ));
}

#[test]
fn writers_report_input_short_of_the_declared_size() {
    let mut compressor = compressor(1);
    let (mut sink, _queue) = compressor.writer(Vec::new(), InputSize::Exact(9).into(), window(1));
    sink.write_all(b"short").unwrap();
    let error = sink.try_finish().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidInput);
    assert!(error.get_ref().unwrap().source().is_some());
}

#[test]
fn writers_describe_themselves() {
    let mut compressor = compressor(1);
    let (sink, _queue) = compressor.writer(Vec::new(), StreamConfig::default(), window(1));
    let debug = format!("{sink:?}");
    assert!(debug.starts_with("ParallelEncoderWriter"));
    assert!(debug.contains("undelivered: 0"));
}

#[test]
fn readers_with_a_declared_size_read_the_batch_frame_however_the_source_splits() {
    let input = corpus::mixed(4 * SEGMENT + 4321, 7);
    let mut compressor = compressor(5);
    let batch = compressor.compress(&input).unwrap();
    for (step, buffer, window_size) in [(999, 77, 1), (SEGMENT, 4096, 3), (usize::MAX, 1 << 20, 8)]
    {
        let source = Unreliable::new(input.clone(), step);
        let (mut reader, queue) = compressor.reader(source, exact(&input), window(window_size));
        let frame = with_executors(queue, 2, || {
            let mut frame = Vec::new();
            let mut chunk = vec![0u8; buffer];
            loop {
                let n = reader.read(&mut chunk).unwrap();
                if n == 0 {
                    break frame;
                }
                frame.extend_from_slice(&chunk[..n]);
            }
        });
        assert!(reader.is_finished());
        assert_eq!(frame, batch, "steps of {step}, buffer {buffer}");
    }
}

#[test]
fn readers_carry_on_after_a_source_failure() {
    let input = corpus::text(2 * SEGMENT + 3, 8);
    let mut compressor = compressor(1);
    let batch = compressor.compress(&input).unwrap();
    let mut source = Unreliable::new(input.clone(), 5000);
    source.fail_at = Some(SEGMENT);
    let (mut reader, queue) = compressor.reader(source, exact(&input), window(2));
    let frame = with_executors(queue, 1, || {
        let mut frame = Vec::new();
        let error = reader.read_to_end(&mut frame).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::BrokenPipe);
        reader.read_to_end(&mut frame).unwrap();
        frame
    });
    assert_eq!(frame, batch);
}

#[test]
fn readers_return_nothing_for_an_empty_buffer_or_after_the_frame() {
    let mut compressor = compressor(1);
    let (mut reader, _queue) = compressor.reader(&b""[..], StreamConfig::default(), window(1));
    assert_eq!(reader.read(&mut []).unwrap(), 0);
    assert!(!reader.is_finished());
    // An empty source is compressed inline, so no executor is needed.
    let mut frame = Vec::new();
    reader.read_to_end(&mut frame).unwrap();
    assert!(reader.is_finished());
    assert_eq!(reader.read(&mut [0u8; 8]).unwrap(), 0);
    drop(reader);
    let serial = mzstd::Compressor::new(*compressor.config())
        .unwrap()
        .compress(b"");
    assert_eq!(frame, serial.unwrap());
}

#[test]
fn readers_report_input_beyond_the_declared_size() {
    let mut compressor = compressor(1);
    let (mut reader, _queue) = compressor.reader(
        &b"longer than declared"[..],
        InputSize::Exact(3).into(),
        window(1),
    );
    let error = reader.read_to_end(&mut Vec::new()).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidInput);
    let ParallelEncodeError::StreamFailed(cause) = parallel_error(&error) else {
        panic!("{error:?}");
    };
    assert_eq!(
        **cause,
        ParallelEncodeError::InputSizeMismatch {
            declared: 3,
            actual: 20
        }
    );
}

#[test]
fn readers_fail_instead_of_blocking_when_every_queue_handle_is_gone() {
    let input = corpus::text(SEGMENT + 10, 9);
    let mut compressor = compressor(1);
    let (mut reader, queue) =
        compressor.reader(input.as_slice(), StreamConfig::default(), window(1));
    drop(queue);
    let error = reader.read_to_end(&mut Vec::new()).unwrap_err();
    assert_eq!(
        parallel_error(&error),
        &ParallelEncodeError::StreamFailed(Arc::new(ParallelEncodeError::TaskQueueClosed))
    );
}

#[test]
fn reader_parts_return_the_unaccepted_read_ahead() {
    let input = corpus::text(3 * SEGMENT, 10);
    let mut compressor = compressor(1);
    let (parts, position) = std::thread::scope(|scope| {
        let (mut reader, queue) = compressor.reader(
            Cursor::new(input.clone()),
            StreamConfig::default(),
            window(1),
        );
        scope.spawn(move || queue.into_iter().for_each(|task| task.run()));
        // With one part in the window, the second read stops short of the
        // second source chunk.
        let mut first = [0u8; 16];
        assert!(reader.read(&mut first).unwrap() > 0);
        assert!(reader.read(&mut first).unwrap() > 0);
        let position = reader.get_mut().position() as usize;
        // Dropping the session closes the queue, which ends the executor.
        (reader.into_parts(), position)
    });
    assert!(!parts.unread_input.is_empty());
    let accepted = position - parts.unread_input.len();
    assert_eq!(parts.unread_input, input[accepted..position]);
    let mut rest = Vec::new();
    Cursor::new(parts.unread_input)
        .chain(parts.source)
        .read_to_end(&mut rest)
        .unwrap();
    assert_eq!(rest, input[accepted..]);
}

#[test]
fn readers_describe_themselves() {
    let mut compressor = compressor(1);
    let (reader, _queue) = compressor.reader(&b"x"[..], StreamConfig::default(), window(1));
    assert_eq!(*reader.get_ref(), b"x");
    let debug = format!("{reader:?}");
    assert!(debug.starts_with("ParallelEncoderReader"));
    assert!(debug.contains("eof: false"));
}
