# Parallel compression

`mzstd::compressor::parallel` divides a known input into fixed independent
segments and assembles them into one standard Zstandard frame. It supports
levels -131072–22. Dictionaries, magicless frames, and framing containers are
unsupported by this API. Output is deterministic for fixed input and segment
settings, but differs from serial compression and zstd's multithreaded mode.

The caller runs the tasks using scoped threads, Rayon, or another executor.
The library does not create a thread pool.

## Compressing a slice

```rust
use mzstd::compressor::parallel::{ParallelCompressor, ParallelConfig, TaskCount};
use mzstd::{Decompressor, EncoderConfig, Level};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = vec![b'a'; 8 << 20];
    let mut encoder = ParallelCompressor::new(
        EncoderConfig::default().with_level(Level::try_from(3)?),
        ParallelConfig::default(),
    )?;
    let mut batch = encoder.prepare_slice(&input, TaskCount::try_from(4)?);
    let tasks = batch.take_tasks();
    std::thread::scope(|scope| {
        for task in tasks {
            scope.spawn(move || task.run());
        }
    });
    let mut output = Vec::new();
    batch.finish_into(&mut output)?;
    let mut decoder = Decompressor::default();
    assert_eq!(decoder.decompress(&output)?, input);
    Ok(())
}
```

`take_tasks` transfers each task once. Every taken task must run or be dropped
so its completion can be reported. Join workers before finalization: it
collects already reported results, checks the source, and assembles the frame
in input order. A missing completion returns `InvalidState`.

## Running tasks with Rayon

Add `rayon = "1.12"` alongside `mzstd` in your dependencies. Create a pool
once and reuse it across requests. Task completion order does not affect assembly
order or the resulting bytes for fixed segment settings.

```rust
use mzstd::compressor::parallel::{ParallelCompressor, ParallelConfig, TaskCount};
use mzstd::{Decompressor, EncoderConfig, Level};
use rayon::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build()?;
    let mut encoder = ParallelCompressor::new(
        EncoderConfig::default().with_level(Level::try_from(3)?),
        ParallelConfig::default(),
    )?;
    let input = b"parallel compression with a reusable pool\n".repeat(220_000);
    let mut batch = encoder.prepare_slice(&input, TaskCount::try_from(4)?);
    let tasks = batch.take_tasks();
    pool.install(|| tasks.into_par_iter().for_each(|task| task.run()));

    let mut compressed = Vec::new();
    batch.finish_into(&mut compressed)?;
    let mut decoder = Decompressor::default();
    assert_eq!(decoder.decompress(&compressed)?, input);
    Ok(())
}
```

For many unrelated small inputs, another option is one ordinary `Compressor` per
worker. That preserves serial compression decisions for each input. The batch
API above splits **one** input into a single assembled stream.

## Streaming input

When input arrives in chunks, `start(stream, window)` returns a session and a task
queue. Each full segment the session accepts becomes a task on that queue;
clone the queue for each executor and run `for task in queue { task.run() }`.
The session delivers finished parts in input order while later segments are
still encoding, and closes the queue when the stream finishes, fails, or is
dropped.

```rust
use mzstd::compressor::parallel::{
    ParallelCompressor, ParallelConfig, ParallelEncoderStatus, TaskCount,
};
use mzstd::{EncoderConfig, Level, Operation};
use std::io::{Read, Write};

fn compress(
    mut reader: impl Read,
    mut writer: impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut encoder = ParallelCompressor::new(
        EncoderConfig::default().with_level(Level::try_from(3)?),
        ParallelConfig::default(),
    )?;
    std::thread::scope(|scope| {
        let (mut session, queue) = encoder.start(Default::default(), TaskCount::try_from(4)?);
        for _ in 0..4 {
            let queue = queue.clone();
            scope.spawn(move || queue.into_iter().for_each(|task| task.run()));
        }
        drop(queue);
        let mut chunk = vec![0; 1 << 16];
        let mut output = vec![0; 1 << 16];
        loop {
            let read = reader.read(&mut chunk)?;
            let operation = if read == 0 {
                Operation::End
            } else {
                Operation::Continue
            };
            let mut input = &chunk[..read];
            loop {
                let progress = session.process(input, &mut output, operation)?;
                input = &input[progress.consumed..];
                writer.write_all(&output[..progress.produced])?;
                match progress.status {
                    ParallelEncoderStatus::Finished => return Ok(()),
                    ParallelEncoderStatus::Pending => session.wait()?,
                    ParallelEncoderStatus::NeedsInput if input.is_empty() => break,
                    _ => {}
                }
            }
        }
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = b"incremental parallel input\n".repeat(350_000);
    let mut compressed = Vec::new();
    compress(input.as_slice(), &mut compressed)?;
    let mut decoder = mzstd::Decompressor::default();
    assert_eq!(decoder.decompress(&compressed)?, input);
    Ok(())
}
```

- `process` never blocks. `Pending` means the window is full or a flush or
  finish waits for queued tasks; call `wait`, which blocks until one completes.
- `window` bounds the segments queued, running or awaiting delivery, and so the
  session's memory. A full window stops input from being consumed.
- With the exact input size declared and no flushes, the output is byte-identical
  to `prepare_slice` over the same bytes, however the input was chunked. `Operation::Flush` ends the current
  segment early, which moves later segment boundaries.
- On a single thread, run `queue.try_recv()` tasks before `wait`; nothing else
  will. Dropping every queue handle fails the stream instead of hanging.
- `into_session(stream, window)` returns an owned session without a lifetime;
  `into_compressor` returns the compressor.

## Reader and writer adapters

`writer(sink, stream, window)` and `reader(source, stream, window)` wrap the same session in
`std::io::Write` and `std::io::Read` and return the task queue beside the
adapter, so the loop above becomes `std::io::copy`:

```rust
use mzstd::compressor::parallel::{ParallelCompressor, ParallelConfig, TaskCount};
use mzstd::io::FinishError;
use mzstd::{EncoderConfig, Level};
use std::io::{Read, Write};

fn compress<W: Write>(mut reader: impl Read, writer: W) -> Result<W, Box<dyn std::error::Error>> {
    let mut encoder = ParallelCompressor::new(
        EncoderConfig::default().with_level(Level::try_from(3)?),
        ParallelConfig::default(),
    )?;
    std::thread::scope(|scope| {
        let (mut sink, queue) = encoder.writer(writer, Default::default(), TaskCount::try_from(4)?);
        for _ in 0..4 {
            let queue = queue.clone();
            scope.spawn(move || queue.into_iter().for_each(|task| task.run()));
        }
        drop(queue);
        std::io::copy(&mut reader, &mut sink)?;
        Ok(sink.finish().map_err(FinishError::into_error)?)
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = b"stream through caller-owned workers\n".repeat(250_000);
    let compressed = compress(input.as_slice(), Vec::new())?;
    let mut decoder = mzstd::Decompressor::default();
    assert_eq!(decoder.decompress(&compressed)?, input);
    Ok(())
}
```

- Unlike `process`, the adapters block: a write waits while the window is
  full, a read waits for the next part, and `finish` waits for every part. Run
  the queue on threads other than the one using the adapter.
- With the exact input size declared and no flushes, output matches `prepare_slice`. The writer's
  `flush` is `Operation::Flush`: it makes everything written decodable and
  moves later segment boundaries.
- The writer keeps every compressed byte until the sink takes it, so short
  writes, `WouldBlock` and sink errors lose nothing, and `finish` hands the
  adapter back inside `FinishError` for a retry, as `EncoderWriter` does.
  `reader.into_parts()` returns source bytes read ahead of the session.
- Session failures arrive as `std::io::Error` with the `ParallelEncodeError`
  as their source. Dropping an adapter abandons the stream and closes the
  queue.

## Segments and compressed size

Segment size defaults to 4 MiB and accepts 64 KiB–256 MiB. Effective task
count is at most the number of segments. Task grouping and completion order do
not change output bytes.

Segments are primed with preceding input: an eighth of the window for most
strategies, or the full window for `BtUltra` and `BtUltra2`. The overlap is not
capped by segment size. Long distance matching runs within each
segment. Compression can be worse than serial compression on data whose useful
history exceeds that overlap; compare compressed size as well as elapsed time.
An input that fits in one segment uses serial encoding and matches `Compressor`.

## Sources and staging

| Input | Entry point |
| --- | --- |
| Borrowed slice | `prepare_slice(input, tasks)` |
| Regular file | `prepare_source(FileSource::open(path)?, tasks)` |
| Owned seekable reader | `prepare_source(SeekSource::from(reader), tasks)` |
| Custom random-access source | `prepare_source(source, tasks)` with `RandomAccessSource` |

`tasks` is a validated `TaskCount`. `FileSource` uses positional reads on Unix
and Windows. `SeekSource` accepts `Read + Seek + Send`, serializes seek/read
operations, and lets compression run concurrently. Sources must remain immutable
for the batch lifetime; identity checks cannot detect every in-place mutation.

Owned sources and shared handles use the same generic entry point. For an
existing `Arc<FileSource>`, type inference can require
`prepare_source::<FileSource, _>(shared_file, tasks)`.

Batch output is staged in memory. Use streaming sessions and their bounded
window when the complete input or compressed output should not be retained.

## Output and cancellation

`finish` returns a new vector. `finish_into` appends a frame and restores the
vector's original length on failure. Write the completed frame to a sink or
use the streaming writer; the caller controls flushing, durability, and publication.

Every taken task must run or be dropped, and workers must finish before batch
finalization. The batch does not wait for detached tasks. Dropping a batch abandons assembly without joining detached workers.
Worker storage can be reused by the parallel owner. See
[parallel mechanics](../architecture/parallel-compression.md) for lifecycle,
source checks, memory, cancellation, and error propagation.
