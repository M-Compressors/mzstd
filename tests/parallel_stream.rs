#![cfg(feature = "compression")]
#![cfg(not(feature = "no_std"))]
//! Parallel sessions: windows, ordered delivery, failures and the owned
//! session.

use std::mem::MaybeUninit;
use std::sync::Arc;

use mzstd::compressor::parallel::{
    ParallelCompressor, ParallelConfig, ParallelEncodeError, ParallelEncoderSession,
    ParallelEncoderStatus, ParallelTaskQueue, SegmentSize, TaskCount,
};
use mzstd::{Compressor, EncoderConfig, InputSize, Level, Operation, StreamConfig};
use zstd_ffi::corpus;

const SEGMENT: usize = 64 << 10;

fn compressor(level: i32, checksum: bool) -> ParallelCompressor {
    let config = EncoderConfig::default()
        .with_level(Level::try_from(level).unwrap())
        .with_checksum(checksum);
    let parallel = ParallelConfig::default().with_segment_size(SegmentSize::MIN);
    ParallelCompressor::new(config, parallel).unwrap()
}

fn window(count: usize) -> TaskCount {
    TaskCount::try_from(count).unwrap()
}

fn exact(input: &[u8]) -> StreamConfig {
    InputSize::Exact(input.len() as u64).into()
}

/// Runs every queued task on this thread, last queued first.
fn run_backwards(queue: &ParallelTaskQueue) {
    let tasks: Vec<_> = std::iter::from_fn(|| queue.next_task()).collect();
    for task in tasks.into_iter().rev() {
        task.run();
    }
}

/// Drives a session single-threaded: `chunk` input bytes and `out` output
/// bytes per call, the last chunk with `last`. Returns once an `End` has
/// finished the frame or a `Flush` has delivered everything.
fn drive(
    session: &mut ParallelEncoderSession<'_>,
    queue: &ParallelTaskQueue,
    input: &[u8],
    chunk: usize,
    out: usize,
    last: Operation,
) -> Vec<u8> {
    let mut frame = Vec::new();
    let mut buffer = vec![0u8; out];
    let mut position = 0;
    loop {
        let end = (position + chunk).min(input.len());
        let operation = if end == input.len() {
            last
        } else {
            Operation::Continue
        };
        let progress = session
            .process(&input[position..end], &mut buffer, operation)
            .unwrap();
        assert!(progress.consumed <= end - position);
        assert!(progress.produced <= out);
        position += progress.consumed;
        frame.extend_from_slice(&buffer[..progress.produced]);
        match progress.status {
            ParallelEncoderStatus::Finished => return frame,
            ParallelEncoderStatus::Pending => {
                run_backwards(queue);
                session.wait().unwrap();
            }
            ParallelEncoderStatus::NeedsInput if operation == Operation::Flush => return frame,
            ParallelEncoderStatus::NeedsInput => assert_eq!(position, end),
            ParallelEncoderStatus::NeedsOutput => {}
        }
    }
}

/// What a decoder makes of a frame that may stop after a flush.
fn decode_prefix(frame: &[u8]) -> Vec<u8> {
    let mut decoder = mzstd::Decompressor::default();
    let mut session = decoder.start();
    let mut content = Vec::new();
    let mut buffer = vec![0u8; 1 << 16];
    let mut input = frame;
    loop {
        let progress = session
            .process(input, &mut buffer, mzstd::DecodeOperation::Process)
            .unwrap();
        input = &input[progress.consumed..];
        content.extend_from_slice(&buffer[..progress.produced]);
        if progress.status != mzstd::DecoderStatus::NeedsOutput {
            return content;
        }
    }
}

fn stream(
    compressor: &mut ParallelCompressor,
    stream: StreamConfig,
    input: &[u8],
    chunk: usize,
    window_size: usize,
) -> Vec<u8> {
    let (mut session, queue) = compressor.start(stream, window(window_size));
    drive(&mut session, &queue, input, chunk, 1000, Operation::End)
}

#[test]
fn declared_streams_match_the_batch_for_any_chunking_and_window() {
    let input = corpus::mixed(5 * SEGMENT + 1234, 1);
    for level in [1, 3, 19] {
        let mut compressor = compressor(level, level != 3);
        let batch = compressor.compress(&input).unwrap();
        for (chunk, window_size) in [(SEGMENT, 1), (7_777, 2), (100_003, 8), (input.len(), 3)] {
            assert_eq!(
                stream(&mut compressor, exact(&input), &input, chunk, window_size),
                batch,
                "level {level}, chunk {chunk}, window {window_size}"
            );
        }
    }
}

#[test]
fn unknown_size_streams_decode_with_the_reference() {
    let input = corpus::binary(4 * SEGMENT + 77, 2);
    let mut compressor = compressor(3, true);
    let frame = stream(&mut compressor, StreamConfig::default(), &input, 9_000, 2);
    assert_ne!(frame, compressor.compress(&input).unwrap());
    assert_eq!(
        zstd_ffi::oracle::decompress_stream(&frame, 4096, None).unwrap(),
        input
    );
}

#[test]
fn inputs_within_one_segment_take_the_serial_fallback_without_a_declared_size() {
    let mut compressor = compressor(5, true);
    let mut serial = Compressor::new(*compressor.config()).unwrap();
    for len in [0, 1, 1000, SEGMENT - 1, SEGMENT] {
        let input = corpus::text(len, 3);
        for chunk in [1000, SEGMENT] {
            assert_eq!(
                stream(&mut compressor, StreamConfig::default(), &input, chunk, 1),
                serial.compress(&input).unwrap(),
                "{len} bytes"
            );
        }
    }
}

#[test]
fn a_full_first_segment_waits_for_the_byte_that_decides_the_fallback() {
    let input = corpus::text(SEGMENT + 1, 4);
    let mut compressor = compressor(3, false);
    let (mut session, queue) = compressor.start(exact(&input), window(2));
    let progress = session
        .process(&input[..SEGMENT], &mut [], Operation::Continue)
        .unwrap();
    assert_eq!(progress.consumed, SEGMENT);
    assert_eq!(queue.queued(), 0);
    let progress = session
        .process(&input[SEGMENT..], &mut [], Operation::Continue)
        .unwrap();
    assert_eq!(progress.consumed, 1);
    assert_eq!(queue.queued(), 1);
    let frame = drive(&mut session, &queue, &[], 1, 64, Operation::End);
    drop(session);
    assert_eq!(frame, compressor.compress(&input).unwrap());
}

#[test]
fn a_full_window_reports_pending_and_takes_no_more_input() {
    let input = corpus::text(4 * SEGMENT, 5);
    let mut compressor = compressor(1, false);
    let (mut session, queue) = compressor.start(StreamConfig::default(), window(2));
    let mut out = vec![0u8; 1 << 20];
    let progress = session
        .process(&input, &mut out, Operation::Continue)
        .unwrap();
    // Two segments are queued, the third is gathered, and the window is
    // full; only the frame header is ready.
    assert_eq!(progress.consumed, 3 * SEGMENT);
    assert!(progress.produced > 0);
    assert_eq!(progress.status, ParallelEncoderStatus::Pending);
    assert_eq!(queue.queued(), 2);
    queue.next_task().unwrap().run();
    session.wait().unwrap();
    let progress = session
        .process(&input[3 * SEGMENT..], &mut out, Operation::Continue)
        .unwrap();
    // The first segment's blocks make room for the third and fourth.
    assert!(progress.produced > 0);
    assert_eq!(progress.consumed, SEGMENT);
    assert_eq!(progress.status, ParallelEncoderStatus::NeedsInput);
    assert_eq!(queue.queued(), 2);
}

#[test]
fn parts_are_delivered_in_input_order_whatever_order_tasks_ran_in() {
    let input = corpus::mixed(4 * SEGMENT, 6);
    let mut compressor = compressor(3, true);
    let batch = compressor.compress(&input).unwrap();
    let (mut session, queue) = compressor.start(exact(&input), window(8));
    let mut frame = vec![0u8; batch.len() + 64];
    let progress = session.process(&input, &mut [], Operation::End).unwrap();
    assert_eq!(progress.status, ParallelEncoderStatus::NeedsOutput);
    let mut tasks: Vec<_> = std::iter::from_fn(|| queue.next_task()).collect();
    assert_eq!(tasks.len(), 4);
    // Only the header is ready while the first segment is outstanding.
    tasks.pop().unwrap().run();
    let header = session.process(&[], &mut frame, Operation::End).unwrap();
    assert!(header.produced > 0);
    assert_eq!(header.status, ParallelEncoderStatus::Pending);
    for task in tasks.into_iter().rev() {
        task.run();
    }
    session.wait().unwrap();
    let rest = session
        .process(&[], &mut frame[header.produced..], Operation::End)
        .unwrap();
    assert_eq!(rest.status, ParallelEncoderStatus::Finished);
    frame.truncate(header.produced + rest.produced);
    assert_eq!(frame, batch);
}

#[test]
fn tasks_run_on_executor_threads() {
    let input = corpus::text(6 * SEGMENT + 5, 7);
    let mut compressor = compressor(5, false);
    let batch = compressor.compress(&input).unwrap();
    let frame = std::thread::scope(|scope| {
        let (mut session, queue) = compressor.start(exact(&input), window(3));
        for _ in 0..3 {
            let queue = queue.clone();
            scope.spawn(move || queue.into_iter().for_each(|task| task.run()));
        }
        drop(queue);
        let mut frame = Vec::new();
        let mut buffer = [0u8; 4096];
        let mut rest = input.as_slice();
        loop {
            let progress = session.process(rest, &mut buffer, Operation::End).unwrap();
            rest = &rest[progress.consumed..];
            frame.extend_from_slice(&buffer[..progress.produced]);
            match progress.status {
                ParallelEncoderStatus::Finished => break frame,
                ParallelEncoderStatus::Pending => session.wait().unwrap(),
                _ => {}
            }
        }
    });
    assert_eq!(frame, batch);
}

#[test]
fn every_queue_handle_shares_the_same_tasks() {
    let mut compressor = compressor(1, false);
    let (mut session, first) = compressor.start(StreamConfig::default(), window(8));
    let second = first.clone();
    session
        .process(&corpus::text(3 * SEGMENT, 8), &mut [], Operation::Continue)
        .unwrap();
    assert_eq!(second.queued(), 3);
    first.next_task().unwrap().run();
    assert_eq!(second.queued(), 2);
    second.recv().unwrap().run();
    assert_eq!(first.queued(), 1);
    first.next_task().unwrap().run();
    assert!(second.next_task().is_none());
}

#[test]
fn the_queue_ends_when_the_session_finishes() {
    let mut compressor = compressor(1, false);
    let (mut session, queue) = compressor.start(StreamConfig::default(), window(1));
    let progress = session.finish(&mut [0u8; 64]).unwrap();
    assert_eq!(progress.status, ParallelEncoderStatus::Finished);
    assert!(queue.recv().is_none());
    assert_eq!(queue.into_iter().count(), 0);
}

#[test]
fn finished_sessions_report_finished_without_moving_bytes() {
    let mut compressor = compressor(1, false);
    let (mut session, queue) = compressor.start(StreamConfig::default(), window(1));
    let frame = drive(&mut session, &queue, b"payload", 3, 64, Operation::End);
    assert!(session.is_finished());
    let progress = session
        .process(b"more", &mut [0u8; 8], Operation::Continue)
        .unwrap();
    assert_eq!((progress.consumed, progress.produced), (0, 0));
    assert_eq!(progress.status, ParallelEncoderStatus::Finished);
    session.wait().unwrap();
    assert_eq!(
        zstd_ffi::oracle::decompress_stream(&frame, 4096, None).unwrap(),
        b"payload"
    );
}

#[test]
fn input_beyond_the_declared_size_fails_the_stream() {
    let mut compressor = compressor(1, false);
    let (mut session, _queue) = compressor.start(InputSize::Exact(10).into(), window(1));
    session
        .process(b"0123456789", &mut [], Operation::Continue)
        .unwrap();
    let error = session
        .process(b"x", &mut [], Operation::Continue)
        .unwrap_err();
    let expected =
        ParallelEncodeError::StreamFailed(Arc::new(ParallelEncodeError::InputSizeMismatch {
            declared: 10,
            actual: 11,
        }));
    assert_eq!(error, expected);
    assert_eq!(error.to_string(), "the parallel stream failed");
    assert_eq!(
        session.process(b"", &mut [], Operation::End),
        Err(expected.clone())
    );
    assert_eq!(session.wait(), Err(expected));
}

#[test]
fn input_short_of_the_declared_size_fails_at_the_end() {
    let mut compressor = compressor(1, false);
    let (mut session, _queue) = compressor.start(InputSize::Exact(10).into(), window(1));
    session
        .process(b"01234", &mut [], Operation::Continue)
        .unwrap();
    let error = session.finish(&mut [0u8; 64]).unwrap_err();
    let ParallelEncodeError::StreamFailed(cause) = &error else {
        panic!("{error:?}");
    };
    assert_eq!(
        cause.to_string(),
        "the stream was declared as 10 bytes but received 5"
    );
}

#[test]
fn a_flush_makes_everything_accepted_decodable() {
    let input = corpus::text(3 * SEGMENT, 9);
    let mut compressor = compressor(3, true);
    let (mut session, queue) = compressor.start(StreamConfig::default(), window(2));
    let mut out = vec![0u8; 1 << 20];
    // Nothing accepted: nothing to flush.
    let progress = session.flush(&mut out).unwrap();
    assert_eq!(progress.produced, 0);
    assert_eq!(progress.status, ParallelEncoderStatus::NeedsInput);
    let head = &input[..SEGMENT + 100];
    let flushed = drive(&mut session, &queue, head, 10_000, 333, Operation::Flush);
    // Everything accepted so far decodes from what was delivered.
    assert_eq!(decode_prefix(&flushed), head);
    let rest = drive(
        &mut session,
        &queue,
        &input[head.len()..],
        10_000,
        333,
        Operation::End,
    );
    let frame = [flushed, rest].concat();
    assert_eq!(
        zstd_ffi::oracle::decompress_stream(&frame, 4096, None).unwrap(),
        input
    );
}

#[test]
fn a_flush_with_a_full_window_is_pending_until_a_slot_frees() {
    let input = corpus::text(2 * SEGMENT + 10, 10);
    let mut compressor = compressor(1, false);
    let (mut session, queue) = compressor.start(StreamConfig::default(), window(1));
    let progress = session
        .process(&input, &mut [], Operation::Continue)
        .unwrap();
    assert_eq!(progress.consumed, 2 * SEGMENT);
    let header = session.flush(&mut [0u8; 64]).unwrap();
    assert!(header.produced > 0);
    assert_eq!(header.status, ParallelEncoderStatus::Pending);
    assert_eq!(queue.queued(), 1);
    let rest = drive(
        &mut session,
        &queue,
        &input[2 * SEGMENT..],
        64,
        50,
        Operation::Flush,
    );
    assert!(!rest.is_empty());
}

#[test]
fn an_end_with_a_full_window_queues_the_last_part_when_repeated() {
    let input = corpus::text(2 * SEGMENT + 10, 11);
    let mut compressor = compressor(1, false);
    let batch = compressor.compress(&input).unwrap();
    let (mut session, queue) = compressor.start(exact(&input), window(2));
    let mut frame = vec![0u8; 1 << 20];
    let progress = session.process(&input, &mut frame, Operation::End).unwrap();
    assert_eq!(progress.consumed, input.len());
    assert_eq!(progress.status, ParallelEncoderStatus::Pending);
    assert_eq!(queue.queued(), 2);
    frame.truncate(progress.produced);
    frame.extend(drive(&mut session, &queue, &[], 1, 64, Operation::End));
    assert_eq!(frame, batch);
}

#[test]
fn dropping_every_queue_handle_abandons_queued_tasks() {
    let input = corpus::text(3 * SEGMENT, 12);
    let mut compressor = compressor(1, false);
    let (mut session, queue) = compressor.start(StreamConfig::default(), window(1));
    session
        .process(&input[..SEGMENT + 1], &mut [], Operation::Continue)
        .unwrap();
    drop(queue);
    let error = session.wait().unwrap_err();
    assert_eq!(
        error,
        ParallelEncodeError::StreamFailed(Arc::new(ParallelEncodeError::TaskAbandoned {
            segment: 0
        }))
    );
}

#[test]
fn a_closed_queue_fails_the_next_dispatch() {
    let input = corpus::text(3 * SEGMENT, 13);
    let mut compressor = compressor(1, false);
    let (mut session, queue) = compressor.start(StreamConfig::default(), window(4));
    drop(queue);
    let error = session
        .process(&input, &mut [], Operation::Continue)
        .unwrap_err();
    assert_eq!(
        error,
        ParallelEncodeError::StreamFailed(Arc::new(ParallelEncodeError::TaskQueueClosed))
    );
}

#[test]
fn a_dropped_task_fails_the_stream_with_its_segment() {
    let input = corpus::text(2 * SEGMENT + 1, 14);
    let mut compressor = compressor(1, false);
    let (mut session, queue) = compressor.start(StreamConfig::default(), window(4));
    session
        .process(&input, &mut [], Operation::Continue)
        .unwrap();
    let first = queue.next_task().unwrap();
    drop(queue.next_task().unwrap());
    first.run();
    let error = session.finish(&mut [0u8; 1 << 20]).unwrap_err();
    assert_eq!(
        error,
        ParallelEncodeError::StreamFailed(Arc::new(ParallelEncodeError::TaskAbandoned {
            segment: 1
        }))
    );
}

#[test]
fn tasks_taken_after_the_session_drops_skip_their_work() {
    let input = corpus::text(2 * SEGMENT, 15);
    let mut compressor = compressor(1, false);
    let (mut session, queue) = compressor.start(StreamConfig::default(), window(2));
    session
        .process(&input, &mut [], Operation::Continue)
        .unwrap();
    let task = queue.next_task().unwrap();
    drop(session);
    task.run();
    assert!(queue.recv().is_none());
    assert_eq!(compressor.retained_bytes(), 0);
}

/// Bytes `process_uninit` must leave alone.
const SENTINEL: u8 = 0xA5;

#[test]
fn process_uninit_matches_process_and_writes_nothing_past_produced() {
    let input = corpus::mixed(3 * SEGMENT + 999, 16);
    let mut compressor = compressor(3, true);
    let batch = compressor.compress(&input).unwrap();
    let (mut session, queue) = compressor.start(exact(&input), window(2));
    let mut frame = Vec::new();
    let mut initialized = [0u8; 97];
    let mut uninit = [MaybeUninit::new(SENTINEL); 97];
    let mut position = 0;
    for call in 0usize.. {
        let end = (position + 5_000).min(input.len());
        let operation = if end == input.len() {
            Operation::End
        } else {
            Operation::Continue
        };
        let progress = if call % 2 == 0 {
            let progress = session
                .process(&input[position..end], &mut initialized, operation)
                .unwrap();
            frame.extend_from_slice(&initialized[..progress.produced]);
            progress
        } else {
            let progress = session
                .process_uninit(&input[position..end], &mut uninit, operation)
                .unwrap();
            // SAFETY: the array was initialized with the sentinel and
            // `process_uninit` only writes initialized bytes.
            let written = uninit.map(|slot| unsafe { slot.assume_init() });
            frame.extend_from_slice(&written[..progress.produced]);
            assert!(written[progress.produced..].iter().all(|&b| b == SENTINEL));
            uninit = [MaybeUninit::new(SENTINEL); 97];
            progress
        };
        position += progress.consumed;
        match progress.status {
            ParallelEncoderStatus::Finished => break,
            ParallelEncoderStatus::Pending => {
                run_backwards(&queue);
                session.wait().unwrap();
            }
            _ => {}
        }
    }
    assert_eq!(frame, batch);
}

#[test]
fn process_uninit_reports_failures_as_process_does() {
    let mut compressor = compressor(1, false);
    let (mut session, _queue) = compressor.start(InputSize::Exact(1).into(), window(1));
    let mut uninit = [MaybeUninit::<u8>::uninit(); 8];
    let error = session
        .process_uninit(b"ab", &mut uninit, Operation::Continue)
        .unwrap_err();
    assert!(matches!(error, ParallelEncodeError::StreamFailed(_)));
}

#[test]
fn owned_sessions_produce_the_bytes_of_borrowed_ones() {
    let input = corpus::text(3 * SEGMENT + 17, 17);
    let mut compressor = compressor(4, true);
    let borrowed = stream(&mut compressor, exact(&input), &input, 20_000, 2);
    let (mut session, queue) = compressor.into_session(exact(&input), window(2));
    assert!(!session.is_finished());
    let mut frame = Vec::new();
    let mut buffer = [0u8; 1000];
    let mut uninit = [MaybeUninit::<u8>::uninit(); 1000];
    let mut rest = input.as_slice();
    for call in 0usize.. {
        let progress = if call % 2 == 0 {
            let progress = session.process(rest, &mut buffer, Operation::End).unwrap();
            frame.extend_from_slice(&buffer[..progress.produced]);
            progress
        } else {
            let progress = session
                .process_uninit(rest, &mut uninit, Operation::End)
                .unwrap();
            // SAFETY: `process_uninit` initialized the first `produced` bytes.
            frame.extend(
                uninit[..progress.produced]
                    .iter()
                    .map(|b| unsafe { b.assume_init() }),
            );
            progress
        };
        rest = &rest[progress.consumed..];
        match progress.status {
            ParallelEncoderStatus::Finished => break,
            ParallelEncoderStatus::Pending => {
                run_backwards(&queue);
                session.wait().unwrap();
            }
            _ => {}
        }
    }
    assert!(session.is_finished());
    assert_eq!(frame, borrowed);
    let compressor = session.into_compressor();
    assert!(compressor.retained_bytes() > 0);
}

#[test]
fn owned_sessions_flush_and_finish() {
    let compressor = compressor(1, false);
    let (mut session, queue) = compressor.into_session(StreamConfig::default(), window(1));
    let mut out = [0u8; 4096];
    session
        .process(b"owned", &mut out, Operation::Continue)
        .unwrap();
    let header = session.flush(&mut out).unwrap();
    assert_eq!(header.status, ParallelEncoderStatus::Pending);
    queue.next_task().unwrap().run();
    session.wait().unwrap();
    let blocks = session.flush(&mut out[header.produced..]).unwrap();
    assert_eq!(blocks.status, ParallelEncoderStatus::NeedsInput);
    let end = session
        .finish(&mut out[header.produced + blocks.produced..])
        .unwrap();
    assert_eq!(end.status, ParallelEncoderStatus::Finished);
    let frame = &out[..header.produced + blocks.produced + end.produced];
    assert_eq!(
        zstd_ffi::oracle::decompress_stream(frame, 4096, None).unwrap(),
        b"owned"
    );
}

#[test]
fn an_unfinished_owned_session_returns_its_compressor() {
    let compressor = compressor(1, false);
    let (mut session, queue) = compressor.into_session(StreamConfig::default(), window(1));
    session
        .process(&corpus::text(SEGMENT + 1, 18), &mut [], Operation::Continue)
        .unwrap();
    let mut compressor = session.into_compressor();
    assert!(queue.recv().is_none());
    assert_eq!(
        compressor.compress(b"again").unwrap(),
        Compressor::new(*compressor.config())
            .unwrap()
            .compress(b"again")
            .unwrap()
    );
}

#[test]
fn sessions_and_queues_describe_themselves() {
    let mut compressor = compressor(1, false);
    let (session, queue) = compressor.start(StreamConfig::default(), window(1));
    assert!(format!("{session:?}").starts_with("ParallelEncoderSession"));
    assert!(format!("{queue:?}").starts_with("ParallelTaskQueue"));
    assert!(format!("{:?}", queue.clone().into_iter()).starts_with("ParallelTasks"));
}
