# Changelog

## Unreleased

- **hotpath Cloud with Callgrind**, ported from mbrotli. New opt-in
  `hotpath-callgrind` and `hotpath-cloud` profiling features, and the
  optional `hotpath` dependency is updated to 0.29. The new
  `profile_compressor` example drives the encoder at levels -5 to 22 on
  repeated text, on a 17-byte input and on an optional corpus. The
  `hotpath benchmark` workflow runs it under Callgrind on Alice for every
  push to `main` and pull request and uploads the report to hotpath Cloud
  (pull requests through `hotpath-relay.yml`), judged by
  `hotpath/policy.toml`. The published crate is unchanged apart from the
  example.

- **Continuous fuzzing with ClusterFuzzLite**, ported from mbrotli.
  `fuzz/libfuzzer` wraps all 14 AFL target bodies (11 stable, 3 framed) as
  libFuzzer targets, which `.clusterfuzzlite/build.sh` builds with
  AddressSanitizer, debug assertions and overflow checks and seeds from the
  AFL corpora and committed regressions. Pull requests that touch code get a
  ten-minute run on the changed code (`ci-cflite-pr.yml`). A daily workflow
  runs 5.5 hours of batch fuzzing, then corpus pruning and a coverage report
  (`ci-cflite-cron.yml`). `ci.yml` lints the adapters. The published crate
  is unchanged.

- **Miri and AddressSanitizer workflows**, ported from mbrotli and run on
  every `v*` tag or on demand. `ci-miri.yml` runs `scripts/miri.sh`, which
  interprets only the tests that reach `unsafe` in dependencies: the
  `fearless_simd` vector kernels (`opt_count` match-length scans, the row
  finder's tag masks, the decoder's shuffle wildcopy and sequence execution
  around it) on the scalar, SSE2, SSE4.2 and AVX2 backends, plus AVX-512
  when the runner supports it (`MIRI_TARGET_CPU=native` in CI), and the
  `borrowed-buf` uninitialized output of both codecs. Tests are named
  exactly and each run checks its pass count. `ci-sanitizer.yml` runs every
  integration test with `framed`, then the same kernel modules natively,
  under `-Zsanitizer=address` with LeakSanitizer. Tests that leaked their
  inputs or `&'static` dictionaries through `Vec::leak`/`Box::leak` now
  borrow locals or a `static OnceLock`; the two tests that forget an
  owning guard on purpose run in a separate step without leak detection.
  Under `cfg!(miri)` the `opt_count` window
  test keeps only lengths next to 16-byte boundaries and
  `compress_to_uninit_writes_the_one_shot_frame` compresses 2 KB; native
  runs check a superset of the previous window cases.

## [v0.2.8](https://github.com/M-Compressors/mzstd/releases/tag/v0.2.8) - 2026-10-20

- **Framed containers (`framed` feature).** `mzstd::framing` ports
  mbrotli's structured framing API one to one — `FramedCompressor` with
  borrowed one-shot input, native and owned sessions, resource guards,
  `FramedWriter`/`ResourceWriter` and `FramedEncoderReader`;
  `FramedDecompressor` with owned, appended and sliced one-shot output,
  event sessions, `FramedReader` and the random-access `FramedSeekReader`;
  resources, metadata (resource, footer, global, repeated), padding,
  hidden resources, explicit dictionary references, a central directory and
  a footer. In place of RFC 9841's Brotli wire format the container is made
  only of Zstandard frames: visible resource chunks are regular frames,
  byte-identical to `ZSTD_compress2` of each chunk, so `zstd -d` prints the
  visible resources; headers, metadata, hidden resources and the directory
  live in skippable frames; and the container ends with a seek table in
  zstd's `contrib/seekable_format` layout. Tests check every container
  against `ZSTD_decompress` and the contrib seekable decoder, which
  `zstd-ffi` now builds as an oracle (along with the contrib seekable
  compressor, the benchmarks' baseline). The fuzz package's opt-in
  `experimental` feature is replaced by `framed`, which gates the
  `framed_encode`, `framed_decode` and `framed_roundtrip` targets. The
  format is specified in `architecture/framing.md`; the API may change in
  patch releases while the feature is opt-in.

- **Clarified `ParallelTaskQueue` documentation.** Stopped executors can
  leave `wait`, the blocking reader and writer and the writer's finishing
  waiting indefinitely; dropping queue handles or tasks reports closure or
  abandonment when the session observes it. Tasks executors already hold
  must still be run or dropped.
- **Parallel batches over random-access sources.**
  `ParallelCompressor::prepare_source` plans a batch over a
  `RandomAccessSource` instead of a borrowed slice, as mbrotli's does, and
  returns an `OwnedParallelBatch` whose tasks are `'static`, so they run on
  detached threads or any executor. Each task reads its segments and their
  overlap into one reused buffer; the frame is byte-identical to
  `prepare_slice` over the same bytes. `ArcBytesSource` (shared `Arc<[u8]>`),
  `SeekSource` (any `Read + Seek + Send` reader behind a mutex) and
  `FileSource` (positional reads of a regular file) implement the trait,
  and `SourceIdentity` lets a source carry a version token. `finish` checks
  that the source's length and identity are unchanged
  (`ParallelEncodeError::SourceChanged`) and, for a frame with a checksum,
  reads the source once more to hash it. Source failures are
  `SourceMetadata` and `SourceRead { segment }`, carrying the `io::Error`
  in a `SourceIoError` so the error stays `Clone + Eq`.

## [v0.2.7](https://github.com/M-Compressors/mzstd/releases/tag/v0.2.7) - 2026-10-18

- **Faster Huffman tables for small inputs.** The literal table's sort
  (`HUF_sort`) no longer places symbols that do not occur: the reference
  sorts them into a last bucket of their own, so the other symbols' order
  is unchanged, and counting them was the longest chain of increments to
  one bucket. One array replaces the reference's start and cursor per
  bucket, since a bucket ends where the next lower one starts. The
  quicksort for buckets from count 164 up (`HUF_simpleQuickSort`) keeps
  waiting ranges on a fixed 28-entry stack of byte pairs instead of
  recursing, and its insertion sort stays hand-written: the standard
  library's stable sort, tried for it, linked about 1,200 instructions of
  driftsort for `Node` that runs of at most nine nodes never reach. The
  sort's stack frame shrank from 1,248 to 496 bytes. On histograms of
  256-byte and 1 KiB slices of the test corpora the sort took 0.12-0.25 µs
  instead of 0.16-0.77 µs, and 0.19-0.78 µs instead of 0.32-0.82 µs on
  16 KiB and 128 KiB ones (an ad hoc timing loop, not checked in).
  Compressing zstd's 1189-byte `http` file took 8.5% less time at level 1,
  6.2% at level 3, 2.4% at level 9 and 7.3% at level 19 (Criterion,
  `benches/compress.rs`, against the previous commit in back-to-back runs
  on an Apple M5 Pro); 1 MiB inputs are unchanged. Frames are
  byte-identical: new tests compare the sort with a verbatim port of
  `HUF_sort` and the quicksort with the recursive one on heavily tied
  counts, and a rebuilt table drops symbols that no longer occur.
- **Faster fast and dfast levels (-131072 to 4).** The fast search tests a
  candidate on a filling table by clamping its offset into the history and
  loading its bytes there, which needs neither a branch nor a stand-in
  word; a clamped offset never matches, so the reference's matches are
  found. The select it replaces rebuilt a stand-in's address at every
  position. Both searches now prefetch the input when their step grows,
  as the reference does. Against the previous commit, level -5 took 2-18% less
  time, level 1 0.3-15%, level 2 0.3-14% and level 3 1-11% on 1 KiB to
  1 MiB of text, binary and long-literal data (1 MiB of text at level 3
  unchanged), with the largest gains on small inputs; level 4 moved by -3% to +1% (one reused context per input,
  interleaved runs of both builds on an Apple M5 Pro). 1 MiB of binary data
  at level 2 measured 5% slower, as it does with dfast untouched: code
  placement, not the loop. Frames are byte-identical. Clamping dfast's
  candidates too was up to 18% faster on single small blocks but up to 16%
  slower on multi-block inputs, and was not kept.
- **Fixed an x86 slowdown in the fast and dfast levels.** On x86 the
  compiler split the clamped candidate test above back into two branches,
  the range test mispredicting on text while the table fills: Alice and
  1 MiB of text took 5-17% longer at levels -5 to 2 than before the
  change (i7-13700KF, interleaved process pairs). The test now folds both
  conditions into one value, which restores the previous time on text and
  keeps a 1-5% gain on 64 KiB of binary and random data. dfast's inlined
  match-length compare, also measured 3-6% slower on text at levels 2 to
  4 on x86, was reverted. Frames are byte-identical.
- **NEON tag mask for the row match finder.** 16-slot rows (levels 5 to 9
  on inputs whose window exceeds 16 KiB) narrow their tag compare to four
  bits per slot with `vshrn`, the reference's `ZSTD_row_getNEONMask`,
  instead of emulating a byte-to-bit mask NEON lacks. The intrinsics run
  in a `fearless_simd::kernel!`; mzstd's source stays free of `unsafe`.
  Levels 5 and 7 took 1-6% less time on 64 KiB and 128 KiB of text and
  binary data on an Apple M5 Pro. The reference's 32- and 64-slot NEON
  masks and a NEON match-length kernel measured no faster than the
  portable code and were not kept. x86 is unchanged.
- **The Huffman encoder no longer zeroes its code table.** The 2 KiB of
  packed codes are built with `array::from_fn`, so each entry is written
  once and the `memset` in front of the fill is gone.
- **Compiled-in table widths for small inputs at the fast levels.** The
  `fast` search compiles in the hash widths of the reference's defaults
  for inputs up to 128 and 256 KiB (minimum match and hash log (5,12),
  (5,13) and (6,14), levels -5, -1 and 1), together with the window's
  first index, for blocks whose prefix starts the window (one-shot frames
  and a fresh context's first frame); other prefixes keep the generic
  loop. Before, only the large-input widths were compiled in, so Alice
  and every 64 KiB input ran the generic loop. Its short literal runs are
  appended straight from the input as one 16-byte chunk, without a stack
  copy or an overflow test (`store_seq_wild`). Cold one-shot calls on
  Alice take 3-5% less time at levels -5, -1 and 1, and 64 KiB inputs
  1-6% less (i7-13700KF, 10 interleaved process pairs per case); the fast
  kernel executes 9% fewer instructions. About 30 KB of code.
- **dfast's table widths are compiled in, one function per width.** The
  six (minimum match, hash log, chain log) sets the default parameters
  give on inputs up to 64 KiB, 256 KiB and above, at levels 2 to 4, each
  run in a separate `#[inline(never)]` search function, so their shifts
  are immediates and their table bounds constants. Inlined side by side,
  LLVM had merged the instances back into one loop with run-time widths,
  which is why the compiled-in widths once measured no faster. The first
  word of a match length is compared inline again, now neutral to 3%
  faster inside the separate functions. After a match, dfast loads the
  word and long-table slot where the match ends and prefetches their
  candidates before the backward extension and the sequence store, so the
  next search no longer waits for them (the kernel is bound by the latency
  after each mispredicted match, not by its instruction count), and short
  literal runs are appended straight from the input as fast's are.
  Together, Alice takes 5% less time at level 2 and 7% at level 3, 1 MiB
  of text 5% at levels 3 and 4 and random data 3% (12 interleaved pairs).
  About 160 KB of code over the four x86 SIMD levels; the 44-byte text at
  level 3 is 1.5-3% slower (0.5 µs calls).
- **The row match finder is chosen once per block.** greedy, lazy and
  lazy2 with the row finder picked among nine (minimum match, row size)
  searches at every searched position, and each parser inlined all nine.
  The search is now picked when a block starts, so each parser inlines
  one. Levels 8 to 12 take 1-2.5% less time on Alice and 1 MiB of text,
  with 6-8% fewer mispredicted branches; greedy and lazy are unchanged.
  The dictMatchState parsers keep the per-search choice. About 570 KB of
  code.
- **Cold calls no longer zero tables they may never use.** The Huffman
  table, the three sequence tables and the optimal parser's state are
  allocated on their first write (a private `LazyBox`: an unwritten table
  reads as one shared all-zero value, as the reference leaves its
  workspace untouched until a block writes it). The engine is built once
  instead of copied, and is 2 KB smaller. Empty input compresses in half
  the time (289 to 145 ns at level 1), the 44-byte text 30-40% faster at
  levels -5 to 9, 64 KiB inputs 1-11% faster; a 44-byte call executes
  5.6K instructions instead of 17.6K (zstd: 6.0K).
- **The block pre-splitter is compiled per sampling rate and hash log**,
  as the reference's `ZSTD_GEN_RECORD_FINGERPRINT` does, and its distance
  loop vectorizes: `ZSTD_splitBlock` executes 40% fewer instructions,
  below the reference's, and 1 MiB of text takes 2.7% less time at
  level 9, 1 MiB of repeated bytes 4-8% less at levels 3 to 19.
- **The decoder no longer zeroes output ahead of each block.**
  `decompress` and `decompress_into` zeroed a whole block of room before
  decoding a compressed block into it. Sequences of 1 KiB or more, a
  block's trailing literals and literal-only blocks are now appended to
  the vector, so their bytes are written once, and room for shorter
  sequences is zeroed 4 KiB at a time as the sequence loop reaches it. A
  `BlockOutput` trait carries both kinds of destination; the sequence loop
  is compiled for fixed destinations (`decompress_to_slice`, streaming
  history), which keep the previous loop, and for growing ones, which
  leave it for a cold path when a sequence needs more room. 1 MiB of text
  decodes 8-16% faster, the 64 KiB binary corpus 14-22% (now faster than
  zstd at every level) and 1 MiB of repeated bytes 10-19%.
  `decompress_to_uninit` zeroes on demand, a chunk at a time.
- **Faster Huffman literal decoding.** The four-stream pair loop follows
  the reference's fast loop: 11-bit pair tables (half the size and build
  cost), five lookups per refill and two-byte stores whose window the
  compiler proves from the row's two-bit symbol count, and refills clamped
  instead of bounds checked: 13.2 instructions per lookup instead of 17.7
  (zstd: 11.25). The single-symbol loop stores symbols straight into
  fixed-size chunks of the four outputs instead of gathering them in an
  array that spilled to the stack (8.8 per symbol instead of 11.6; zstd
  8.2). Single-symbol tables are filled by weight in fixed-size runs
  after a counting sort (`HUF_readDTableX1_wksp`), weight streams decode
  four symbols per refill, and sequence FSE tables are spread without
  per-cell tests when no symbol has a "less than one" probability
  (`ZSTD_buildFSETable_body`): reading a Huffman tree costs 35% fewer
  instructions, a sequence table 20%. Alice and 1 MiB of text decode
  3-5% faster at levels 1 to 19. These table builds were measured slower
  on 2026-10-15; that run compared builds whose unchanged sequence loop
  moved by ±1.6% instructions with code placement, which this
  measurement controlled for.
- **The extDict fast finder selects its segment without a branch.**
  Streams use it once their buffer wraps (most blocks of a long stream at
  the fast levels). It picks the segment holding a hash-table candidate
  with `select_unpredictable` (the reference's `idxBase` select) and
  extends matches backwards inside that segment without a per-byte
  segment test. Mispredicted branches fall by a quarter on a 4 MiB stream
  at level 1, which compresses about 2% faster (`stream/mixed-4MiB`;
  levels -5 and 1, random data 3%).
- **Predefined sequence tables are built once per decoder.** A frame
  start forgot every sequence table, so each frame that used the
  predefined distributions rebuilt all three. A slot now remembers that
  its entries hold the predefined table until an RLE or described table
  overwrites them, as the reference points at static tables. Decoding
  1,000 small dictionary-compressed records one frame at a time takes 13%
  less time (27% fewer instructions); larger inputs are unchanged.
- **`EncoderWriter` no longer copies every block twice.** It offered each
  session call 128 KiB of output room, a few hundred bytes less than a
  full block's compress bound, so every block was compressed into the
  session's internal buffer and copied out by the flush stage. It now
  offers `ZSTD_CStreamOutSize` (131,591 bytes), into which full blocks are
  written directly. 4 MiB of random data written in 64 KiB pieces
  compresses 10% faster at level 1 and 6.5% at level 3 (17% fewer
  instructions); frames are unchanged.
- **AFL regressions cover the benchmark inputs.** The comparison
  benchmark's eight inputs at its 24 levels replay through the
  `differential_c` and `decode_roundtrip` bodies
  (`fuzz/afl/tests/benchmark_corpora.rs`; the 1 MiB inputs are cut to the
  targets' 256 KiB cap), the 44-byte text and empty input at every level
  are committed regressions, and `prepare-seeds.sh` adds Alice at one
  level per strategy to the campaign seeds.
- **Benchmarks.** Both comparison suites were rerun on this release
  (`enc-20261018`, `dec-20261018`); the previous run's archives were
  replaced. Compression runs at a median 1.112x zstd's speed, faster in 166
  of 192 dataset/level pairs, against 1.086x and 159 for the tree before
  these changes swept the same day; decompression at 1.111x, faster in 147,
  against 1.032x and 122. Every case that looked slower in the sweep was
  rerun as interleaved process pairs and was not.

## [v0.2.6](https://github.com/M-Compressors/mzstd/releases/tag/v0.2.6) - 2026-10-16

- **Decoder output into uninitialized memory.** `DecoderSession` and
  `DecoderSessionOwned` have `process_uninit`, and `Decompressor` has
  `decompress_to_uninit` and `decompress_with_dictionary_to_uninit`, as in
  mbrotli. Each takes `&mut [MaybeUninit<u8>]`, such as
  `Vec::spare_capacity_mut`, and writes exactly the bytes it reports,
  nothing past them; a session's `DecodeFailure` has written exactly its
  `produced` prefix. Bytes, counts, statuses and errors match `process`,
  `decompress_to_slice` and `decompress_with_dictionary_to_slice`, and
  `process` and `process_uninit` may be mixed on one operation. The crate
  stays free of `unsafe`. Sessions only ever copy finished output out of
  their history, so `process_uninit` writes the caller's memory once and
  never zeroes it. The one-shot direct path reads its output back as history,
  so it zeroes the destination through `borrowed-buf` one block ahead of the
  decoder and never past a frame's declared end; raw and RLE blocks are
  written once without zeroing. The `decompression` feature now pulls in
  `borrowed-buf`. The decoder is generic over its destination rather than
  matching on it, so `process` runs the code it ran before: streaming 4 MiB
  of mixed data took 2.81 ms against 2.84 ms before the change (Criterion,
  `benches/decompress.rs`, Apple M5 Pro). Against `decompress_to_slice`,
  `decompress_to_uninit` took 0.97-1.05 times as long on 4 MiB of text and
  binary at levels 1, 3 and 19 and on a 12 MiB LDM frame, and the same on
  random data; `process_uninit` took 2.83 ms against 2.81 ms for `process`.
  New Criterion cases `mzstd-uninit` cover both paths.
- **`mzstd_decompress` no longer zeroes the destination.** The C ABI views
  the caller's buffer as `MaybeUninit<u8>` and calls
  `decompress_to_uninit`, so on success no byte past the result is written.
  `mzstd-ffi` drops its `borrowed-buf` dependency. On
  `mzstd-ffi/benches/one_shot.rs`, 256 KiB of random data decodes in
  2.93 µs instead of 3.84 µs and 1 MiB of text 0-2.5% faster. The
  large-literal corpus is 2-4% slower, and a 64-byte frame takes 30 ns
  instead of 25 ns at level -5 and 1.5-3% longer at other levels.
- **Fix `FrameMode::Single` with skippable frames.** A single-frame
  operation skipped skippable frames and kept waiting for a Zstandard frame,
  unlike `ZSTD_decompressStream`, which reports a frame complete after
  either kind. A trailing skippable frame, or a stream made only of
  skippable frames, failed with `UnexpectedEndOfInput` although zstd reads
  it, and a skippable frame followed by a Zstandard frame decoded both in
  one member where zstd stops after the skippable one. `Single` now ends
  after exactly one frame of either kind: a skippable frame finishes the
  operation with no output and leaves what follows unconsumed (one-shot
  calls report it as `TrailingData`). Sessions, one-shots and the `io`
  adapters change alike; `FrameMode::Concatenated` is unchanged. Regression
  tests compare member-by-member decoding with the reference through the
  new `zstd_ffi::oracle::decompress_first_frame`.
- **`0` restores a compression parameter's default.** Every numeric
  `with_*` setter of `CompressionParams` and `LdmParams` stores `0` as
  unset, as `ZSTD_CCtx_setParameter(.., 0)` does. The last value given now
  wins, and a caller translating zstd-style parameters can apply each one
  directly in its loop instead of collecting them in `Option`s first.
  Before, `with_window_log(0)` stored `Some(0)` and `Compressor::new` rejected
  it as out of range, and no setter could clear a value. `0` was already
  accepted by `with_target_length` and `LdmParams::with_hash_rate_log` and
  meant the default there, so no frame changes.
- **Tests and fuzzing.** `tests/decoder_uninit.rs` compares the new
  methods with their initialized counterparts across corpora, levels, input
  chunks, output windows, failures, frame sequences and dictionaries, with
  sentinels past every reported length. The AFL decoder targets alternate
  `process` and `process_uninit`, and `decompress` checks
  `decompress_to_uninit`; `parameter_parsing` accepts a zero window log.
  The architecture adds a
  [decoder uninitialized output](architecture/decoder-uninit-output.md)
  specification.

## [v0.2.5](https://github.com/M-Compressors/mzstd/releases/tag/v0.2.5) - 2026-10-15

- **Faster fast levels (-5 to 2).** The hash widths of the default
  parameters for inputs above 256 KiB are compiled in, which turns the hash
  shift and table mask into constants, and a candidate's validity is
  checked with a branch once the history before a block is several times
  the table's size, where that branch predicts well (the branch-free check
  stays for blocks whose table is still filling). On the first 20 MB of
  Silesia (`silesia.tar`, warm contexts) levels -5 to 2 compress 5-10%
  faster. Frames are unchanged.
- **btlazy2 (levels 13-15) is as fast as zstd.** The binary-tree walk picks
  the side to descend with a branch, as the reference does, instead of a
  branch-free select: the walk is a chain of dependent cache misses, and a
  predicted branch starts the next node's load before the comparison that
  picks it resolves. On the first 5 MB of Silesia levels 13 and 15
  compress 17% and 21% faster. Frames are unchanged.
- **Sessions without a dictionary are small again.** `PreparedDictionary`
  keeps its tables behind one `Box`, so it is pointer-sized and an
  `EncoderSessionOwned` (default `D = PreparedDictionary`) no longer reserves
  about 8 KiB inline for a dictionary it may not have: 40 bytes instead of
  8192 on 64-bit targets. The public API is unchanged.
- **Benchmarks.** Both comparison suites were rerun on this release; the
  previous run's archives were replaced.

## [v0.2.4](https://github.com/M-Compressors/mzstd/releases/tag/v0.2.4) - 2026-10-15

- **Dictionaries.** `mzstd::dictionary` reads raw-content and
  Zstandard-format dictionaries (`DictionaryContent::{Auto, Raw, Zstd}`).
  `DictionaryBuilder` prepares a `PreparedDictionary` for one
  `EncoderConfig`, and `Compressor` gains `compress_with_dictionary`,
  `compress_with_dictionary_into`, `_to_slice`, `_to_uninit`,
  `start_with_dictionary`, `into_session_with_dictionary` and, with `std`,
  `writer_with_dictionary` and `reader_with_dictionary`; a dictionary
  prepared for another configuration (frame options aside) is
  `EncodeError::DictionaryMismatch`. Frames are byte-identical to zstd
  1.5.7's `ZSTD_compress2` and `ZSTD_compressStream2` after
  `ZSTD_CCtx_loadDictionary_advanced`: the dictionary's own tables and
  parameters (tagged fast/dfast tables), the reference's choice between
  attaching the dictionary and copying its tables, dictMatchState variants
  of every match finder (fast, dfast, greedy/lazy/lazy2 with hash chains and
  rows, btlazy2, btopt, btultra, btultra2), the dictionary's entropy tables,
  repeat offsets and ID, and the optimal parser's first-block statistics
  seeded from them. One exception, found by fuzzing: with a dictionary whose
  literal or match length table omits its last symbols, the reference's
  optimal parser reads uninitialized memory and its frames vary from run to
  run; mzstd's stay deterministic.

- **Dictionary decoding.** `DecodeDictionary` parses dictionaries as
  zstd 1.5.7's `ZSTD_DDict` does, rejected exactly when the reference
  rejects them; a `PreparedDictionary` decodes too, through
  `DictionaryRef::Prepared`, without a second copy of its bytes.
  `Decompressor` gains `decompress_with_dictionary`, `_into`, `_to_slice`,
  `start_with_dictionary`, `into_session_with_dictionary` and, with `std`,
  `reader_with_dictionary`/`writer_with_dictionary`. A frame naming another
  dictionary ID fails with the new `DecodeError::DictionaryMismatch`
  (`DictionaryUnsupported` still reports one decoded without a dictionary).
  `EncoderSessionOwned` and `DecoderSessionOwned` gain a defaulted type
  parameter for the dictionary they own. Decoding without a dictionary is
  unchanged.

- **Parallel segments are primed with the input before them instead of
  compressing it.** For fast, dfast, greedy, btopt, btultra and btultra2,
  each segment loads its overlap as a raw-content prefix, the way ZSTDMT
  primes a job (`ZSTD_compressBegin_advanced_internal` with
  `ZSTD_dtlm_fast` and `forceWindow`; checked byte-identical against
  `ZSTD_CCtx_refPrefix` with `ZSTD_c_forceMaxWindow` at every level, LDM
  included), where the overlap used to be compressed and thrown away.
  Lazy, lazy2 and btlazy2 still compress it: loading made them up to three
  times slower on random input (lazy, lazy2) and 9% slower on text
  (btlazy2). Overlap sizes are unchanged. With four threads and 4 MiB
  segments on 16 MiB of text, binary, random and mixed data and on the
  vendored zstd sources (i7-13700KF, `benches/parallel.rs`
  `parallel-corpora`, new): levels 18-22 run 15-27% faster (binary level
  19: 2.34 s instead of 2.77 s) with frames 0.01-0.27% larger, levels 1-5
  up to 27% faster and levels 16-17 1-7% faster with frames within 0.12%;
  levels 6-15 are unchanged. Parallel output changes; serial output does
  not. Each stream segment's buffer is now allocated once at its full size,
  and batch assembly reserves the frame.

- **Cheaper Huffman code packing.** Literals packed their codes symbol by
  symbol with a branch; the packing is now branch-free over the whole
  alphabet in the backend's feature context, which matters for a
  dictionary's 256-symbol table (one 107-byte record with a dictionary: 9.2k
  instructions instead of 11.5k). Output is unchanged.

- Measured against 0.2.3 on the Criterion suites (`benches/compress.rs`,
  `benches/decompress.rs`, pinned to one core, alternating runs): every
  compression and decompression case without a dictionary is within ±2%,
  and 64-byte and 1 KiB inputs execute 0.3% more instructions per call.
  Compression with a dictionary (`benches/dictionary.rs`, trained 64 KiB
  dictionary, JSON-like records) runs at 0.85-0.91x zstd 1.5.7's time at
  levels 16 and 19 on 100 and 1000 records, 1.09-1.19x at levels 1-9, and
  1.20-1.29x for a single 107-byte record.

- New fuzz target `compress_dictionary` and `decode_dictionary`, and the
  `dictionary` Criterion benchmark; the `parallel` benchmark gains a
  per-level group.

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
