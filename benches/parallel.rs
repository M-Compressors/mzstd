//! Parallel compression against zstd 1.5.7's single-threaded encoder.
//!
//! One 16 MiB input is compressed in 1 MiB segments with four executor
//! threads through a batch, a session and the parallel writer, after
//! checking that all three write the same frame and that it decodes. zstd's
//! `ZSTD_compress2` on the same bytes is timed for reference; the parallel
//! frames are larger, and both sizes are printed before timing.

#[cfg(not(feature = "no_std"))]
mod parallel {
    use criterion::{BenchmarkId, Criterion, Throughput};
    use mzstd::compressor::parallel::{
        ParallelCompressor, ParallelConfig, ParallelEncoderStatus, ParallelTaskQueue, SegmentSize,
        TaskCount,
    };
    use mzstd::{Compressor, EncoderConfig, InputSize, Level, Operation};
    use std::hint::black_box;
    use zstd_ffi::corpus;
    use zstd_ffi::oracle::{self, Params};

    /// Runs `body` while `executors` threads drain `queue`.
    fn with_executors<T>(
        queue: ParallelTaskQueue,
        executors: usize,
        body: impl FnOnce() -> T,
    ) -> T {
        std::thread::scope(|scope| {
            for _ in 0..executors {
                let queue = queue.clone();
                scope.spawn(move || queue.into_iter().for_each(|task| task.run()));
            }
            drop(queue);
            body()
        })
    }

    /// A parallel session over `input` in 64 KiB calls with a 64 KiB buffer.
    fn parallel_session(compressor: &mut ParallelCompressor, input: &[u8], out: &mut Vec<u8>) {
        out.clear();
        let stream = InputSize::Exact(input.len() as u64).into();
        let window = TaskCount::try_from(PARALLEL_TASKS).expect("a window");
        let (mut session, queue) = compressor.start(stream, window);
        with_executors(queue, PARALLEL_TASKS, || {
            let mut buffer = vec![0u8; 64 << 10];
            let mut position = 0;
            loop {
                let end = (position + (64 << 10)).min(input.len());
                let operation = if end == input.len() {
                    Operation::End
                } else {
                    Operation::Continue
                };
                let progress = session
                    .process(&input[position..end], &mut buffer, operation)
                    .expect("compresses");
                position += progress.consumed;
                out.extend_from_slice(&buffer[..progress.produced]);
                match progress.status {
                    ParallelEncoderStatus::Finished => break,
                    ParallelEncoderStatus::Pending => session.wait().expect("waits"),
                    _ => {}
                }
            }
        });
    }

    /// The parallel writer over `input` in 64 KiB writes.
    fn parallel_writer(compressor: &mut ParallelCompressor, input: &[u8], out: &mut Vec<u8>) {
        use std::io::Write;
        out.clear();
        let stream = InputSize::Exact(input.len() as u64).into();
        let window = TaskCount::try_from(PARALLEL_TASKS).expect("a window");
        let (mut sink, queue) = compressor.writer(out, stream, window);
        with_executors(queue, PARALLEL_TASKS, || {
            for piece in input.chunks(64 << 10) {
                sink.write_all(piece).expect("writes");
            }
            sink.try_finish().expect("finishes");
        });
    }

    /// A batch of `PARALLEL_TASKS` tasks on as many threads.
    fn parallel_batch(compressor: &mut ParallelCompressor, input: &[u8], out: &mut Vec<u8>) {
        out.clear();
        let tasks = TaskCount::try_from(PARALLEL_TASKS).expect("a task count");
        let mut batch = compressor.prepare_slice(input, tasks);
        std::thread::scope(|scope| {
            for task in batch.take_tasks() {
                scope.spawn(move || task.run());
            }
        });
        batch.finish_into(out).expect("compresses");
    }

    /// Executor threads of the parallel group.
    const PARALLEL_TASKS: usize = 4;

    pub fn parallel(c: &mut Criterion) {
        let input = corpus::mixed(16 << 20, 11);
        let mut group = c.benchmark_group("parallel/mixed-16MiB");
        group.throughput(Throughput::Bytes(input.len() as u64));
        for level in [1, 3] {
            let config =
                EncoderConfig::default().with_level(Level::try_from(level).expect("level"));
            let segments = ParallelConfig::default()
                .with_segment_size(SegmentSize::try_from(1 << 20).expect("a segment size"));
            let mut compressor =
                ParallelCompressor::new(config, segments).expect("a configuration");
            let mut batch = Vec::new();
            parallel_batch(&mut compressor, &input, &mut batch);
            let mut out = Vec::new();
            parallel_session(&mut compressor, &input, &mut out);
            assert_eq!(out, batch, "session, level {level}");
            parallel_writer(&mut compressor, &input, &mut out);
            assert_eq!(out, batch, "writer, level {level}");
            assert_eq!(
                oracle::decompress(&batch, input.len()).expect("decodes"),
                input
            );
            let params = Params::level(level);
            let theirs = oracle::compress2(&input, &params).expect("reference");
            println!(
                "parallel level {level}: {} -> {} bytes ({:.4}); zstd serial {} bytes ({:.4})",
                input.len(),
                batch.len(),
                batch.len() as f64 / input.len() as f64,
                theirs.len(),
                theirs.len() as f64 / input.len() as f64
            );
            group.bench_function(BenchmarkId::new("mzstd-batch", level), |b| {
                b.iter(|| parallel_batch(&mut compressor, black_box(&input), &mut out));
            });
            group.bench_function(BenchmarkId::new("mzstd-session", level), |b| {
                b.iter(|| parallel_session(&mut compressor, black_box(&input), &mut out));
            });
            group.bench_function(BenchmarkId::new("mzstd-writer", level), |b| {
                b.iter(|| parallel_writer(&mut compressor, black_box(&input), &mut out));
            });
            let mut cctx = oracle::Encoder::new(&params);
            let mut dst = vec![0u8; Compressor::max_compressed_size(input.len()).expect("a bound")];
            group.bench_function(BenchmarkId::new("zstd-serial", level), |b| {
                b.iter(|| cctx.compress_into(black_box(&input), &mut dst));
            });
        }
        group.finish();
    }
}

#[cfg(not(feature = "no_std"))]
criterion::criterion_group! {
    name = benches;
    config = criterion::Criterion::default().sample_size(10);
    targets = parallel::parallel
}
#[cfg(not(feature = "no_std"))]
criterion::criterion_main!(benches);

/// Parallel compression needs `std`.
#[cfg(feature = "no_std")]
fn main() {}
