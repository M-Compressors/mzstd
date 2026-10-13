#![cfg(feature = "compression")]
#![cfg(not(feature = "no_std"))]
//! Parallel frames decode, and do not depend on scheduling.

use mzstd::compressor::parallel::{
    ParallelCompressor, ParallelConfig, ParallelEncodeError, SegmentSize, TaskCount,
};
use mzstd::{Compressor, ConfigError, EncoderConfig, Level, Parameter};
use std::sync::Arc;
use zstd_ffi::corpus;

fn compressor(level: i32, checksum: bool) -> ParallelCompressor {
    let config = EncoderConfig::default()
        .with_level(Level::try_from(level).unwrap())
        .with_checksum(checksum);
    let parallel =
        ParallelConfig::default().with_segment_size(SegmentSize::try_from(256 << 10).unwrap());
    ParallelCompressor::new(config, parallel).unwrap()
}

fn threaded(compressor: &mut ParallelCompressor, input: &[u8], tasks: usize) -> Vec<u8> {
    let mut batch = compressor.prepare_slice(input, TaskCount::try_from(tasks).unwrap());
    let tasks = batch.take_tasks();
    std::thread::scope(|scope| {
        for task in tasks.into_iter().rev() {
            scope.spawn(move || task.run());
        }
    });
    batch.finish().unwrap()
}

#[test]
fn frames_are_independent_of_task_count_and_decode() {
    let input = corpus::mixed(2_000_000, 1);
    for level in [-3, 1, 3, 9, 16] {
        let mut compressor = compressor(level, level % 2 != 0);
        let reference = threaded(&mut compressor, &input, 1);
        for tasks in [2, 3, 8, 64] {
            assert_eq!(
                threaded(&mut compressor, &input, tasks),
                reference,
                "level {level}, {tasks} tasks"
            );
        }
        assert_eq!(
            zstd_ffi::oracle::decompress(&reference, input.len()).unwrap(),
            input
        );
    }
}

#[test]
fn inputs_within_one_segment_match_the_serial_compressor() {
    let input = corpus::text(200_000, 4);
    let mut parallel = compressor(7, false);
    let mut serial = Compressor::new(*parallel.config()).unwrap();
    assert_eq!(
        parallel.compress(&input).unwrap(),
        serial.compress(&input).unwrap()
    );
    assert_eq!(
        parallel.compress(&[]).unwrap(),
        serial.compress(&[]).unwrap()
    );
    assert!(parallel.retained_bytes() > 0);
    parallel.release();
    assert_eq!(parallel.retained_bytes(), 0);
}

#[test]
fn the_compressor_reports_its_parallel_configuration() {
    let segment = SegmentSize::try_from(1 << 20).unwrap();
    let parallel = ParallelConfig::default().with_segment_size(segment);
    assert_eq!(parallel.segment_size(), segment);
    let compressor = ParallelCompressor::new(EncoderConfig::default(), parallel).unwrap();
    assert_eq!(compressor.parallel_config(), &parallel);
    assert_eq!(
        ParallelConfig::default().segment_size(),
        SegmentSize::DEFAULT
    );
}

#[test]
fn the_segment_size_selects_where_segments_are_cut() {
    let input = corpus::text(1_000_000, 5);
    let mut small = compressor(3, false);
    let mut large = ParallelCompressor::new(
        *small.config(),
        ParallelConfig::default().with_segment_size(SegmentSize::try_from(512 << 10).unwrap()),
    )
    .unwrap();
    let small_frame = small.compress(&input).unwrap();
    let large_frame = large.compress(&input).unwrap();
    assert_ne!(small_frame, large_frame);
    for frame in [small_frame, large_frame] {
        assert_eq!(
            zstd_ffi::oracle::decompress(&frame, input.len()).unwrap(),
            input
        );
    }
}

#[test]
fn task_counts_default_to_one() {
    assert_eq!(TaskCount::default(), TaskCount::ONE);
    assert_eq!(TaskCount::default().get(), 1);
}

#[test]
fn task_counts_accept_one_through_the_maximum() {
    assert_eq!(TaskCount::try_from(1).unwrap(), TaskCount::ONE);
    assert_eq!(TaskCount::try_from(1 << 16).unwrap(), TaskCount::MAX);
}

#[test]
fn task_counts_outside_their_range_are_rejected() {
    for value in [0, (1 << 16) + 1] {
        let error = TaskCount::try_from(value).unwrap_err();
        assert_eq!(
            error,
            ConfigError::OutOfRange {
                parameter: Parameter::TaskCount,
                value: value as i64,
                min: 1,
                max: 1 << 16,
            }
        );
    }
    assert_eq!(
        TaskCount::try_from(0).unwrap_err().to_string(),
        "task count 0 is outside 1..=65536"
    );
}

#[test]
fn segment_sizes_outside_their_range_are_rejected() {
    let error = SegmentSize::try_from((64 << 10) - 1).unwrap_err();
    assert_eq!(
        error.to_string(),
        "segment size 65535 is outside 65536..=268435456"
    );
    assert!(SegmentSize::try_from((256 << 20) + 1).is_err());
    assert_eq!(SegmentSize::default(), SegmentSize::DEFAULT);
}

#[test]
fn batches_run_untaken_tasks_on_the_calling_thread() {
    let input = corpus::text(600_000, 11);
    let mut compressor = compressor(1, false);
    let reference = threaded(&mut compressor, &input, 3);
    let batch = compressor.prepare_slice(&input, TaskCount::try_from(3).unwrap());
    let mut out = b"prefix".to_vec();
    batch.finish_into(&mut out).unwrap();
    assert_eq!(&out[..6], b"prefix");
    assert_eq!(out[6..], reference[..]);
}

#[test]
fn tasks_are_capped_at_one_per_segment() {
    let input = corpus::text(2 * (256 << 10), 12);
    let mut compressor = compressor(1, false);
    let mut batch = compressor.prepare_slice(&input, TaskCount::MAX);
    assert_eq!(batch.take_tasks().len(), 2);
}

#[test]
fn a_single_segment_batch_is_one_serial_task() {
    let input = corpus::text(100_000, 13);
    let mut compressor = compressor(4, true);
    let mut serial = Compressor::new(*compressor.config()).unwrap();
    let mut batch = compressor.prepare_slice(&input, TaskCount::try_from(8).unwrap());
    let tasks = batch.take_tasks();
    assert_eq!(tasks.len(), 1);
    std::thread::scope(|scope| {
        for task in tasks {
            scope.spawn(move || task.run());
        }
    });
    assert_eq!(batch.finish().unwrap(), serial.compress(&input).unwrap());
}

#[test]
fn batches_with_dropped_tasks_report_their_abandonment() {
    let input = corpus::text(600_000, 10);
    let mut compressor = compressor(1, false);
    let mut batch = compressor.prepare_slice(&input, TaskCount::try_from(2).unwrap());
    let mut tasks = batch.take_tasks();
    assert_eq!(tasks.len(), 2);
    tasks.remove(0).run();
    drop(tasks);
    let mut out = b"kept".to_vec();
    let error = batch.finish_into(&mut out).unwrap_err();
    // Three 256 KiB segments: the first task has two, the second one.
    assert_eq!(
        error,
        ParallelEncodeError::BatchFailed(Arc::new(ParallelEncodeError::TaskAbandoned {
            segment: 2
        }))
    );
    assert_eq!(out, b"kept");
    assert_eq!(error.to_string(), "the parallel batch failed");
    let cause = std::error::Error::source(&error).unwrap();
    assert_eq!(
        cause.to_string(),
        "the task for segment 2 was dropped without running"
    );
}

#[test]
fn a_dropped_serial_task_fails_its_batch() {
    let mut compressor = compressor(1, false);
    let mut batch = compressor.prepare_slice(b"short", TaskCount::ONE);
    drop(batch.take_tasks());
    assert_eq!(
        batch.finish(),
        Err(ParallelEncodeError::BatchFailed(Arc::new(
            ParallelEncodeError::TaskAbandoned { segment: 0 }
        )))
    );
}

#[test]
fn batches_with_leaked_tasks_report_invalid_state() {
    let input = corpus::text(600_000, 14);
    let mut compressor = compressor(1, false);
    let mut batch = compressor.prepare_slice(&input, TaskCount::try_from(2).unwrap());
    std::mem::forget(batch.take_tasks());
    let error = batch.finish().unwrap_err();
    assert_eq!(error, ParallelEncodeError::InvalidState);
    assert_eq!(
        error.to_string(),
        "the batch cannot be assembled: a task never reported"
    );
}
