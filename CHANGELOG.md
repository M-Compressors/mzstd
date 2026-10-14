# Changelog

## [v0.2.3](https://github.com/M-Compressors/mzstd/releases/tag/v0.2.3) - 2026-10-15

- **Faster fast and double-fast match finding (levels -131072 to 4).**
  Candidate validity is a select between the candidate's bytes and a
  stand-in word that can never match (the reference's
  `ZSTD_match4Found_cmov`), so only equality branches; this removed most of
  the finders' branch mispredictions. Blocks that cannot read below the
  prefix take a specialization with prefix-relative positions, hash slots
  are held as cell references, and exit tests share the bounds checks of the
  loads they guard. Frames remain byte-identical to zstd 1.5.7.

- **Faster lazy finders (greedy to btlazy2).** Row tables are viewed from
  a 64-byte boundary as the reference's workspace aligns them (half of the
  tag rows used to straddle cache lines); search state is built once per
  block; the row update checks one range per run and steps through a
  constant table; collected candidates are prefetched as in the reference;
  btlazy2's tree walk counts short runs inline, hands long ones to the
  vector scan, and picks its side without a branch.

- **Faster optimal parser (btopt, btultra, btultra2).** A per-block
  `Finder` precomputes the search's parameters, the tree update runs out of
  line, the walk exits early when the first candidate is outside the window,
  hash cells are prefetched four positions ahead, long literal runs refresh
  all literal weights at once, and the position and match tables are sized
  to what a block can use. Levels 16-22 now run faster than zstd 1.5.7 on
  every dataset of the comparison corpus.

- **Cheaper sequence coding.** `fill_codes` builds the codes, extra-bit
  counts and the three code histograms in one pass, `sequences::encode`
  adds each sequence's extra bits with one write when they number fewer
  than 31, and the bit writers and FSE table build drop their bounds checks.
  The sequence encoder now executes fewer instructions than the reference's.

- Updated [benchmarks](docs/benchmarks/README.md) for this working tree
  (2026-10-14): compression median 1.092× zstd 1.5.7 (was 1.000×), faster in
  158 of 192 dataset/level pairs (was 96); decompression median 1.022×, with Alice 0.92× (was 0.89×) and cyclic
  Alice 0.85× (was 0.82×). All frames remain byte-identical to zstd 1.5.7.
  Text at the fastest compression levels and compressible-text decoding
  remain slower than C.

- **Faster literal and sequence decoding.** Huffman pair tables are built
  from one series per code length with one add per row (3.4x fewer
  instructions), sequence bit counts carry their range so reads need no
  masking, the first chunk of a long-offset match copy and the Huffman fast
  loops' refills need no bounds checks.

## [v0.2.2](https://github.com/M-Compressors/mzstd/releases/tag/v0.2.2) - 2026-10-14

- **A session holds a first `End` call to its declared size.** With
  `InputSize::Exact(n)`, a session whose whole input arrived in one
  `Operation::End` call of any other length used to write a complete frame
  for that length, as `ZSTD_compressStream2` does when it replaces the
  pledged size. It now fails with `EncodeError::InputSizeMismatch`, like
  every other contradiction of a declared size, so a truncated or overlong
  input never yields a frame. Frames whose input matches its declaration
  are unchanged and still byte-identical to zstd 1.5.7.

- **`process_uninit` and `compress_to_uninit` write in place.** Both used to
  allocate and zero a temporary buffer as large as the destination on every
  call, then copy the result over. The encoder now writes into the caller's
  `MaybeUninit` memory directly, zeroing it only just ahead of each block it
  writes there (tracked with `borrowed-buf`, without `unsafe`), and copies
  buffered output in. Bytes, counts, statuses and errors are those of
  `process` and `compress_to_slice`. As with those, bytes past the reported
  output may now hold zeros or scratch bytes; only the reported prefix is
  meaningful. The `encoder_session` and `output_capacity` fuzz targets cover
  both paths, and `benches/compress.rs` times them: with 128 KiB outputs
  over 1 MiB of mixed data in 16 KiB chunks, `process_uninit` at level 1
  takes 1.25 ms (was 1.35 ms; `process` 1.26 ms), and with 4 KiB outputs
  1.22 ms (was 1.28 ms). `compress_to_uninit` of 1 MiB of text is 1-2%
  faster (level 3: 1.334 ms, was 1.350-1.363 ms). Measured with
  `cargo bench --bench compress -- 'stream-output|compress_to_uninit'` on
  Apple Silicon (aarch64-apple-darwin); output is identical.

## [v0.2.1](https://github.com/M-Compressors/mzstd/releases/tag/v0.2.1) - 2026-10-14

- **Reuse the Huffman tree during depth selection.** Optimal-depth trials
  restore leaf lengths from the original internal nodes instead of sorting
  and rebuilding the same tree for each depth. Table selection and encoded
  bytes remain identical to zstd 1.5.7.

- **Less work on incompressible blocks.** After a failed Huffman attempt,
  raw literals that already exceed the block's compression threshold cause
  an early raw-block decision. This avoids copying a literals section and
  encoding sequences that would be discarded. Short output buffers retain
  their existing error behavior, and frames remain byte-identical to C.

- **Simpler fast/dfast candidate checks.** Bounded reads from the searchable
  history reject stale indices and guard the comparison word with one range
  check. The search order and minimum-match specializations are unchanged.

- **Wider match comparisons.** Fast, double-fast and lazy search share a
  word-first, 128-byte scan in the selected SIMD backend. On AVX2 it tests
  four vectors together, then resolves the first differing word. Short
  matches and bounded tails keep their exact bytewise semantics.

- **Less output initialization in cold compression.** `compress` and
  `compress_into` reserve the same capacity but initialize the output of
  inputs larger than 128 KiB one block at a time, reusing unwritten room.
  The RLE check uses bulk slice comparison. Frames remain byte-identical
  to zstd 1.5.7; caller-owned slices keep their exact capacity semantics.

- **Faster empty and tiny inputs at levels 11-22.** The optimal parser's
  position and match tables are sized per block on first use
  (`min(block + 3, ZSTD_OPT_SIZE)` entries) instead of being filled to
  4099 entries at every frame start. Frames are unchanged.

- Updated [compression benchmarks](docs/benchmarks/encoder-comparison.md)
  for the final working tree: 384 measurements, median speed relative to C
  1.000×, with lower mean latency in 96 of 192 dataset/level pairs. All
  frames remain byte-identical to zstd 1.5.7. Large text remains slower;
  the aggregate does not establish an overall lead.

- **Faster `Decompressor::decompress` and `decompress_into`.** Raw and RLE
  blocks are appended in one pass instead of zeroing room and copying over
  it; frames that declare a content size are decoded into a vector cut at
  their end, so neither slack nor spare capacity is kept (a declared frame's
  vector now has exactly its length as capacity); capacity doubles, bounded
  by the frame, and a declared size is reserved up front up to 32 bytes per
  unread input byte. 1 MiB of random data decodes in 20.1 µs (was 43.8 µs;
  zstd 1.5.7 20.6 µs), 64 KiB in 1.01 µs (was 1.82 µs; zstd 1.79 µs). Errors
  are unchanged: a frame that overruns its declared size is re-decoded uncut
  to report the same `ContentSizeMismatch`.
- New cold comparison against zstd 1.5.7 in `benchmarks/comparison`
  (its own workspace): mbrotli's eight datasets, including Alice in
  Wonderland (`testdata/alice29.txt`), at levels -5, -1 and 1-22, with
  byte-identity preflight, CSV export, and generated per-level pages,
  per-dataset charts and run reports in `docs/benchmarks/`, laid out as
  mbrotli's benchmark documentation. See `docs/benchmarking.md`.

## [v0.2.0](https://github.com/M-Compressors/mzstd/releases/tag/v0.2.0) - 2026-10-13

- **Breaking:** parallel streaming sessions are rebuilt around a window
  and a non-blocking `process`. `ParallelCompressor::start(stream, window)`
  returns a `ParallelEncoderSession` and its `ParallelTaskQueue`;
  `into_session(stream, window)` returns the owned
  `ParallelEncoderSessionOwned` (with `into_compressor`). Sessions have
  `process`, `flush` and `finish`, which report the new
  `ParallelEncoderStatus` (`NeedsInput`, `NeedsOutput`, `Pending`,
  `Finished`) in a `ParallelProgress`, and `wait`, which blocks for one task
  completion. `window` bounds the segments queued, running or awaiting
  delivery, and so the session's memory; a full window reports `Pending`.
  Each full segment becomes a task on the queue, which executors drain with
  `for task in queue { task.run() }` (`recv` blocks; `next_task` does not).
  With a declared size and without a flush, the frame is byte-identical to
  `prepare_slice` for any chunking, window and execution order; an input
  that fits in one segment takes the batch's serial fallback with or without
  a declared size. Finishing, failing or dropping the session closes the
  queue, so executor loops end; dropping every queue handle fails the stream
  with the new `ParallelEncodeError::TaskQueueClosed` or `TaskAbandoned`
  instead of hanging. Stream failures are the new `StreamFailed` with the
  cause as its source, repeated on every later call. `push`, `drain_into`,
  `finish_into`, `run_queued` and `queue()` are removed.
- `ParallelEncoderSession` and `ParallelEncoderSessionOwned` have
  `process_uninit`, which delivers into `&mut [MaybeUninit<u8>]` such as
  `Vec::spare_capacity_mut` without zeroing it first. It produces the same
  bytes, counts, statuses and errors as `process`, writes nothing past
  `produced`, and may be mixed with `process` on one stream. The AFL
  `parallel` target alternates the two.
- Parallel streams have `std::io` adapters.
  `ParallelCompressor::writer(sink, stream, window)` returns a
  `ParallelEncoderWriter` and `ParallelCompressor::reader(source, stream,
  window)` a `ParallelEncoderReader`, each beside the `ParallelTaskQueue`
  their segment tasks go to, as `start` does. Both drive the parallel session
  and produce its bytes, so with a declared size and without a flush the
  output is byte-identical to `prepare_slice` however writes or source reads
  are split. Unlike `process`, they block in `wait` while the window is full
  or a part is pending, so the queue must run on other threads. The writer
  keeps `EncoderWriter`'s transactional guarantees: nothing is lost to short
  writes or sink errors, `Write::flush` is `Operation::Flush`, and `finish`
  returns the adapter inside `FinishError` for a retry. `reader.into_parts()`
  returns read-ahead source bytes as the new `EncoderReaderParts`
  (`source`, `unread_input`), which `EncoderReader::into_parts` now returns
  too. `ParallelEncodeError` now converts into `std::io::Error`, taking the
  kind of its root cause through `StreamFailed` and `BatchFailed` and never
  `Interrupted` or `WouldBlock`. The AFL `parallel` target also drives both
  adapters.
- Batch tasks report exactly once: a task dropped unrun is
  `TaskAbandoned { segment }` and one that unwinds `TaskPanicked { segment }`
  (the panic still propagates and its engine is discarded). A failed batch
  returns the new `ParallelEncodeError::BatchFailed` with the first failing
  task's cause as its source, and `finish_into` leaves its output untouched;
  `InvalidState` now only reports a task leaked with `mem::forget`. Batch
  frames are unchanged.
- A new `parallel` Criterion bench (`benches/parallel.rs`) compresses one
  16 MiB input in 1 MiB segments on four executor threads through the batch, the session and the
  writer, beside zstd's single-threaded `ZSTD_compress2`. Measured with
  `cargo bench --bench parallel -- --warm-up-time 1
  --measurement-time 3` (Apple M5 Pro): all three write the same frame, and
  the session and writer run within noise of the batch (level 1: 9.99,
  10.08 and 10.02 ms; level 3: 17.62, 17.27 and 17.12 ms). zstd serial takes
  18.09 and 30.37 ms; the parallel frames are 0.05% and 0.36% larger
  (6286456 vs 6283433 and 5712713 vs 5692172 bytes).
- `mzstd` and `mzstd-ffi` are now dual-licensed under MIT or
  Apache-2.0, at your option. The MIT text moved from `LICENSE` to
  `LICENSE-MIT`, and `LICENSE-APACHE` was added.
- The README and crate docs have one "Choose your API" table covering
  both codecs, the `std::io` adapters and `ParallelCompressor`
  (`prepare_slice`, `start`). In the crate docs it renders for every feature
  set; APIs a build leaves out link to the feature-selection section.
- The `CI Fuzz` workflow archives AFL's findings before uploading them:
  `actions/upload-artifact` rejects AFL's file names, which contain `:`.

- Added `Backend`, a public, host-validated SIMD level
  (`fearless_simd`): `Backend::available`, `Backend::name`,
  `Display`/`Debug`, `CompressorBuilder::with_backend` /
  `DecompressorBuilder::with_backend`, and `Compressor::backend` /
  `Decompressor::backend`. The default is detected once when a codec is
  built; every backend produces identical output. `compression` and
  `decompression` now depend on `fearless_simd`, and `compression` on
  `branches` for safe prefetch hints.
- Decompression is 2.5-9x faster, now at 0.93-1.18x of zstd 1.5.7's time
  on `cargo bench --bench decompress` (i7-13700KF, AVX2, one core),
  with identical output and the same verdicts on malformed input:
  - Huffman and sequence bitstreams use a 64-bit container refilled where
    the reference refills. Four-stream literals decode interleaved with
    the reference's sentinel fast loops, with a pair (X2) table when the
    reference's timing model prefers it.
  - Sequence tables carry baselines and extra-bit counts per state;
    predefined tables are kept between blocks; tables and buffers are
    reused.
  - Sequences execute with wildcopies, including a byte-shuffle kernel for
    overlapping short-offset matches on SSSE3, AVX2, AVX-512, NEON and
    WASM SIMD.
  - `decompress_to_slice` decodes straight into the destination when every
    frame declares a content size that fits.
- Compression is 1.3-3.4x faster. On `cargo bench --bench compress` it now
  runs at 0.88-1.07x of zstd 1.5.7's time on small inputs and at levels
  16-19, 1.03-1.17x on 1 MiB of text, and 1.1-1.3x on binary input and
  in streaming. Frames stay byte-identical to zstd 1.5.7 on every backend:
  - The fast and dfast finders are specialized per minimum match length,
    read through fixed-width windows over the block's prefix, and mask
    their table indices; extDict variants work the same way.
  - The greedy, lazy, lazy2 and btlazy2 finders compare row tags with one
    `u8x16`/`u8x32`/`u8x64` compare (`ZSTD_row_getMatchMask`), prefetch
    rows 8 positions ahead as the reference does, and are specialized per
    search, depth, minimum match, row size and extDict mode.
  - The optimal parser (btopt, btultra, btultra2) and LDM are specialized
    per backend, optimization level and extDict mode; one binary-tree walk
    serves insertion and match collection; prices come from tables
    rebuilt at each rescale with a vectorized weight form.
  - Match lengths are compared a word at a time, then with native vectors.
  - Entropy coding follows the reference's BMI2 builds: packed Huffman
    codes with its two-container loop, interleaved histograms, the fast FSE
    spread, and 16-byte literal copies.
  - Per call: block states swap instead of copying, and tables are reused
    whenever indices continue, as `ZSTD_reset_matchState` does.
- XXH64 stays scalar: an AVX2 kernel measured 1.8x slower (no 64-bit
  vector multiply); the scalar code matches the reference's throughput.

## [v0.1.0](https://github.com/M-Compressors/mzstd/releases/tag/v0.1.0) - 2026-10-13

First version: a safe-Rust Zstandard encoder byte-identical to zstd 1.5.7,
an independent RFC 8878 decoder, parallel compression, and a C ABI.

### Compression

- Added `Compressor` with `compress`, `compress_into`, `compress_to_slice`
  and `compress_to_uninit`. One-shot output is byte-identical to
  `ZSTD_compress2` with the same parameters.
- Added incremental sessions: `EncoderSession`, and `EncoderSessionOwned`
  with `reinit` and `into_compressor`. A session matches
  `ZSTD_compressStream2` driven with the same chunks, output capacities and
  `Continue`/`Flush`/`End` operations, including the reference's direct
  compression of a final `End` and the content size it then records.
- Configuration in `EncoderConfig`:
  - levels -131072..=22 (`Level`), the explicit `CompressionParams` (window,
    chain, hash and search logs, minimum match, target length, `Strategy`);
  - long distance matching (`LongDistanceMatching`, `LdmParams`);
  - checksum and content-size flags, and a source size hint.
  - Out-of-range values fail with `ConfigError::OutOfRange`, which names the
    `Parameter`. `Compressor::default()` uses level 3.
- Added `std::io` adapters (`EncoderReader`, `EncoderWriter`, `FinishError`)
  and memory retention control (`RetentionPolicy`, `trim`,
  `retained_bytes`).
- The core ports zstd 1.5.7's encoder:
  - all nine strategies with their extDict variants, the row match finder
    and binary trees;
  - the optimal parser, including btultra2's first-block pass;
  - long distance matching;
  - the pre- and post-block splitters;
  - FSE and Huffman table selection;
  - streaming buffers and window wrap-around;
  - index overflow correction.

### Decompression

- Added `Decompressor` with `decompress`, `decompress_into` and
  `decompress_to_slice`; `DecoderSession` and `DecoderSessionOwned`; the
  `DecoderReader` and `DecoderWriter` adapters. Concatenated and skippable
  frames are supported.
- `DecodeLimits` bound the window (default 2^27, up to 2^31), the input and
  the output. Failures are typed (`DecodeError`, `InvalidDataKind`).
- It is stricter than the reference in two places: Huffman streams must be
  consumed exactly, and offsets beyond the window are rejected in every API.
  Frames that need a dictionary fail with `DictionaryUnsupported`.

### Parallel compression (`std`)

- Added `compressor::parallel`:
  - `ParallelCompressor`, with `prepare_slice` batches of caller-run tasks
    and streaming `ParallelEncoderSession`s with a `ParallelTaskQueue`;
  - segments that reach back into an overlap of the previous input;
  - frames independent of the task count and execution order;
  - panic-safe task completion.
- Parallel frames decode anywhere but are not byte-identical to the serial
  encoder or to zstd's multithreaded mode.

### Tooling

- Added `mzstd-ffi`: `mzstd_compress` (byte-identical to `ZSTD_compress`),
  `mzstd_decompress`, `mzstd_compress_bound`, and `include/mzstd.h`.
- Added `zstd-ffi`, a test-only crate that builds the vendored zstd v1.5.7
  and provides safe oracle wrappers and corpora.
- Added AFL++ targets in `fuzz/afl`: `differential_c`, `output_capacity`,
  `encoder_session`, `compressor_lifecycle`, `parallel`,
  `parameter_parsing`, `decompress`, `decode_streaming` and
  `decode_roundtrip`. Each has committed regressions, a seed builder over
  upstream's test data, and a campaign script.
- Added Criterion benchmarks against zstd (`compress`, `decompress`, and
  `mzstd-ffi`'s `one_shot`), CI workflows and architecture specifications.
- Measured with `cargo bench --bench compress -- 'compress/(text-1MiB|binary-1MiB)/'`
  and `cargo bench --bench decompress -- 'decompress/(text-4MiB|binary-4MiB)/level-3'`
  (`--warm-up-time 1 --measurement-time 3`, Apple M5 Pro, rustc 1.99).
  Output sizes are identical on both sides. Compression runs at 0.5–0.7×
  zstd's speed (text 1 MiB, level 3: 418 vs 775 MiB/s; level 19: 3.8 vs
  5.5 MiB/s). Decompression runs at 0.2–0.35× (text 4 MiB: 716 MiB/s vs
  2.19 GiB/s). Neither codec has SIMD yet.
