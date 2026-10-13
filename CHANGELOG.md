# Changelog

## Unreleased

- `mzstd` and `mzstd-ffi` are now dual-licensed under MIT or
  Apache-2.0, at your option. The MIT text moved from `LICENSE` to
  `LICENSE-MIT`, and `LICENSE-APACHE` was added.
- The README and crate docs have one "Choose your API" table covering
  both codecs, the `std::io` adapters and `ParallelCompressor`
  (`prepare_slice`, `start`). In the crate docs it renders for every feature
  set; APIs a build leaves out link to the feature-selection section.
- The `CI Fuzz` workflow archives AFL's findings before uploading them:
  `actions/upload-artifact` rejects AFL's file names, which contain `:`.

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
