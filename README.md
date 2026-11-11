# mzstd

Zstandard (RFC 8878) compression and decompression in safe Rust.

[Documentation](https://m-compressors.github.io/mzstd/) · [Getting started](docs/getting-started.md) · [API reference](https://docs.rs/mzstd)

[![Crates.io](https://img.shields.io/crates/v/mzstd.svg)][crate]
[![docs.rs](https://docs.rs/mzstd/badge.svg)][api]
[![Tests](https://github.com/M-Compressors/mzstd/actions/workflows/ci.yml/badge.svg?branch=main)][ci]
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![hotpath-cloud](https://img.shields.io/endpoint?url=https://hotpath.rs/badge-cloud.json)](https://hotpath.rs/app/repos/M-Compressors/mzstd)

- **Byte-identical encoder.** For the same input and parameters,
  single-threaded compression writes exactly the bytes of zstd 1.5.7:
  one-shot calls match `ZSTD_compress2`, and sessions match
  `ZSTD_compressStream2` driven with the same chunks, output capacities and
  flushes. This holds for levels -131072 to 22, the explicit compression
  parameters, long distance matching, checksums, content sizes, pledged sizes,
  size hints, target compressed block sizes (`ZSTD_c_targetCBlockSize`) and
  dictionaries (raw or Zstandard-format, as `ZSTD_CCtx_loadDictionary` loads
  them).
- **Independent decoder** written from the format specification. It decodes
  every frame zstd writes, with or without a dictionary, with bounded window,
  input and output limits.
- **Parallel compression** with tasks you run on your own executor. Frames
  are valid Zstandard frames and are identical for any task count, but they
  are not byte-identical to zstd's multithreaded mode. Segments are primed
  with the input before them the way zstd's multithreaded mode primes a job.
- **No `unsafe`, no dependencies on zstd.** It runs with `no_std` + `alloc`,
  and there is a C ABI, one-shot and streaming, in `mzstd-ffi`.
- **Sans-I/O sessions.** `Encoder` and `Decoder` sessions work only with the buffers you
  pass in and never perform I/O themselves, so you can drive them from blocking code,
  async runtimes, or your own event loop.

## Performance

### Our benchmarks

Compression writes byte-identical frames to zstd 1.5.7 and reaches a median
1.248× its speed across the comparison corpus; decompression reaches 1.517×.
These medians include empty and tiny inputs. Alice text remains slower at
some compression levels and at most decompression levels, and on the whole
`silesia.tar` in lzbench both codecs still trail C by a few percent at most
levels.

<details>
<summary>Compression benchmark results and methodology</summary>

![Compression speed and output size relative to zstd 1.5.7 across levels -5 to 22][bench-chart]

**Recorded 2026-11-09 · mzstd 0.2.12 · AMD Ryzen 9 7950X (KVM, host shared with other tenants) · cold one-shot APIs.**
Each value is the median of per-dataset ratios across eight equally weighted datasets,
including empty and tiny inputs. A speed ratio above **1×** is faster than C; every
frame is byte-identical to zstd's, so the size ratio is **1×** at every level. The
median over all 192 dataset/level pairs is **1.248×**, with lower mean latency
in 174 pairs (2026-10-30: 1.256×, 179 pairs). The 18 slower pairs are Alice at
ten levels, cyclic Alice at four and random data at four (0.84–0.998×). The host
was loaded during the run: medians are stable, single cells good to about ±10%.

The benchmark includes compressor construction, allocation, compression, and disposal.
Results apply to the [recorded revision and machine][bench-levels]; [lzbench](#lzbench)
covers a whole large archive.

[Per-dataset results and methodology][benchmarks] · [Raw measurements][bench-csv] · [Reproduce the benchmarks][benchmarking]

</details>

<details>
<summary>Decompression benchmark results and methodology</summary>

![Decompression speed relative to zstd 1.5.7 by source level][decoder-bench-chart]

**Recorded 2026-11-09 · mzstd 0.2.12 · AMD Ryzen 9 7950X (KVM, host shared with other tenants) · cold one-shot APIs.**
zstd C and mzstd decode identical C-written frames at source levels **-5 to 22**.
All **384 cases** restore the original bytes. The median over all 192 pairs is
**1.517×** (2026-10-30: 1.502×), faster in 151 pairs: much faster on empty and tiny
frames and on random data, 1.45× on structured binary and 1.53× on run-length
data, and 0.95× (Alice) and 0.94× (cyclic Alice) on compressible text.
Construction, allocation, decode and disposal are timed; C reads the output
capacity from the frame header.

[Per-level and dataset results][decoder-benchmarks] · [Raw measurements][decoder-bench-csv] · [Methodology and limits][decoder-bench-report]

</details>

### External benchmarks

#### lzbench

<details>
<summary>lzbench 2.4.1 results for mzstd 0.2.12 and zstd 1.5.7</summary>

[lzbench](https://github.com/inikep/lzbench) is a long-running open-source
in-memory benchmark: every codec is built from source into one binary with the
same compiler and flags and runs on the same data and machine. We ran lzbench
master at [`a6fa8aa`][lzbench-commit] (lzbench 2.4.1, gcc 15.2, C and Rust
codecs built with `-march=native`), with this release's sources in place of
the mzstd 0.2.11 it vendors, on the 211 MB `silesia.tar` of the
[Silesia corpus](http://sun.aei.polsl.pl/~sdeor/index.php?page=silesia),
pinned to one thread of an AMD Ryzen 9 7950X VDS, with `-t8,8`, in three
rounds on 2026-11-09. [Command, raw output of every round and the 0.2.11 run][lzbench-run].

mzstd 0.2.12 against zstd 1.5.7; bold marks the faster result. Ratio is the
compressed size as a percentage of the input and matches for both: every frame
is byte-identical.

| Level | Compress mzstd | Compress zstd | Decompress mzstd | Decompress zstd | Ratio, % |
| ----: | -------------: | ------------: | ---------------: | --------------: | -------: |
| -22 | **2.27 MB/s** | 2.13 MB/s | 1498 MB/s | **1608 MB/s** | 24.67 |
| -18 | **5.03 MB/s** | 4.28 MB/s | 1745 MB/s | **1858 MB/s** | 25.15 |
| -15 | **9.04 MB/s** | 8.41 MB/s | 2014 MB/s | **2116 MB/s** | 26.83 |
| -11 | 42.7 MB/s | **45.7 MB/s** | 1982 MB/s | **2082 MB/s** | 27.35 |
| -8 | 103 MB/s | **107 MB/s** | 1945 MB/s | **2040 MB/s** | 28.17 |
| -5 | 203 MB/s | **208 MB/s** | 1769 MB/s | **1858 MB/s** | 29.60 |
| -3 | **374 MB/s** | 370 MB/s | 1784 MB/s | **1892 MB/s** | 31.20 |
| -2 | 505 MB/s | **534 MB/s** | 1834 MB/s | **1966 MB/s** | 32.70 |
| -1 | 626 MB/s | **658 MB/s** | 1992 MB/s | **2134 MB/s** | 34.53 |
| --fast=1 | 682 MB/s | **720 MB/s** | 2504 MB/s | **2695 MB/s** | 41.01 |
| --fast=3 | 763 MB/s | **812 MB/s** | 2673 MB/s | **2867 MB/s** | 44.63 |
| --fast=5 | 838 MB/s | **893 MB/s** | 2828 MB/s | **3048 MB/s** | 48.61 |

**Summary:** byte-identical output at every level; the fastest of three
rounds compresses 6.6-17.5% faster than zstd at levels 15, 18 and 22, at its
speed at level 3 and 2.4-6.6% slower at the other levels, and decompresses
4.7-7.2% slower at every level. Compression at level 2 and above moves by
3-21% between rounds on this loaded host. Against 0.2.11 in the same
rounds, relative to zstd, decompression gained 0.8-5 points at levels 1 to 22
(level 1: 0.883 to 0.933), level 3 compression went from 0.973 to 1.011,
and the `--fast` levels and levels 1 and 2 compress 0.8-2.4 points faster.
lzbench compresses the whole archive as one frame, so it measures
large-input throughput; the medians above also weight empty and tiny
inputs, where mzstd is much faster.

</details>

#### TurboBench
Results available [here](https://github.com/powturbo/TurboBench).

## Usage

### Install

```toml
[dependencies]
mzstd = "0.2"
```

### Examples

```rust
use mzstd::{Compressor, Decompressor, EncoderConfig, Level};

let mut encoder = Compressor::new(EncoderConfig::default().with_level(Level::try_from(19)?))?;
let compressed = encoder.compress(b"hello, hello, hello")?;

let restored = Decompressor::default().decompress(&compressed)?;
assert_eq!(restored, b"hello, hello, hello");
# Ok::<(), Box<dyn std::error::Error>>(())
```

Explicit parameters override the level just as `ZSTD_CCtx_setParameter` does:

```rust
use mzstd::{CompressionParams, Compressor, EncoderConfig, LdmParams, Level, LongDistanceMatching, Strategy};

let config = EncoderConfig::default()
    .with_level(Level::try_from(9)?)
    .with_checksum(true)
    .with_params(CompressionParams::default().with_window_log(24).with_strategy(Strategy::Lazy2))
    .with_long_distance_matching(LongDistanceMatching::Enabled(LdmParams::default()));
let mut encoder = Compressor::new(config)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Dictionaries make small inputs compress well. Prepare one for the
configuration you compress with; the same dictionary decodes its frames:

```rust
use mzstd::dictionary::DictionaryBuilder;
use mzstd::{Compressor, Decompressor, EncoderConfig};

let config = EncoderConfig::default();
// Raw content here; a trained Zstandard-format dictionary works the same way.
let dictionary = DictionaryBuilder::new(br#"{"user":"alice","city":"Oslo","tags":["a","b"]}"#)
    .with_config(config)
    .build()?;
let record = br#"{"user":"alice","city":"Oslo","tags":["b"]}"#;
let frame = Compressor::new(config)?.compress_with_dictionary(&dictionary, record)?;
let restored = Decompressor::default().decompress_with_dictionary(&dictionary, &frame)?;
assert_eq!(restored, record);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Choose your API

Compression and decompression expose the same core I/O shapes. Pick the shape that
matches how your application already moves bytes. `ParallelCompressor` (in
`mzstd::compressor::parallel`) is a separate execution strategy for compression: it
splits one frame into independent segments and hands them to your executor as tasks,
either for a whole input at once or for input that arrives in chunks. It requires `std`.

| I/O shape | Compression | Decompression |
| --- | --- | --- |
| Return a new `Vec<u8>` | `Compressor::compress` | `Decompressor::decompress` |
| Append to an existing Vec | `compress_into` | `decompress_into` |
| Write into a caller-owned slice | `compress_to_slice`, `compress_to_uninit` | `decompress_to_slice` |
| Pull output through `std::io::Read` | `Compressor::reader` | `Decompressor::reader` |
| Push input through `std::io::Write` | `Compressor::writer` | `Decompressor::writer` |
| Drive input/output incrementally | `start` → `EncoderSession` | `start` → `DecoderSession` |
| Same, with the session owning the codec | `into_session` → `EncoderSessionOwned` | `into_session` → `DecoderSessionOwned` |
| Any of the above with a dictionary | `compress_with_dictionary*`, `start_with_dictionary`, `into_session_with_dictionary`, `writer_with_dictionary`, `reader_with_dictionary` | `decompress_with_dictionary*`, `start_with_dictionary`, `into_session_with_dictionary`, `writer_with_dictionary`, `reader_with_dictionary` |
| Split one known input across your threads | `ParallelCompressor::prepare_slice` / `prepare_source` → `ScopedParallelBatch` | — |
| Stream chunked input across your threads | `ParallelCompressor::start` → `ParallelEncoderSession` + `ParallelTaskQueue` | — |
| Same, with the session owning the compressor | `ParallelCompressor::into_session` → `ParallelEncoderSessionOwned` | — |
| Push or pull through `std::io` across your threads | `ParallelCompressor::writer`, `ParallelCompressor::reader` | — |

## Features

| Feature | Default | Enables |
| --- | --- | --- |
| `compression` | yes | `mzstd::compressor` |
| `decompression` | yes | `mzstd::decompressor` |
| `std` | yes | `std::error::Error`, `mzstd::io`, parallel compression |
| `no_std` | no | alloc-only build; overrides `std` |
| `framed` | no | `mzstd::framing`: seekable containers of resources and metadata, built only from Zstandard frames (see below) |
| `hotpath`, `hotpath-cpu`, `hotpath-alloc`, `hotpath-callgrind`, `hotpath-cloud` | no | profiling anchors; Callgrind instruction counts and hotpath Cloud upload |

## Framed containers

With `features = ["framed"]`, `mzstd::framing` stores ordered resources,
metadata, padding, a central directory and a footer in one container,
with the same API as mbrotli's framing. The container is only Zstandard
frames: every visible resource is a sequence of regular frames, so
`zstd -d` prints the visible resources concatenated; everything else is
in skippable frames; and the last frame is a seek table in zstd's
`contrib/seekable_format` layout, which seekable readers use to jump to
any offset.

```rust
use mzstd::framing::{FramedCompressor, FramedDecompressor, FramedItem, FramedResource};

let items = [FramedItem::Resource(FramedResource::from(&b"payload"[..]))];
let container = FramedCompressor::new(Default::default())?.compress(items.as_slice().into())?;
let output = FramedDecompressor::new(Default::default())?.decompress(&container)?;
assert_eq!(output.resources[0].data, b"payload");
```

`FramedDecompressor::framed_seek_reader` reads one resource of a container
through any `Read + Seek` source. The format is specified in
[architecture/framing.md](architecture/framing.md).

## Verification

- Differential tests against zstd 1.5.7 (built from the vendored submodule by
  `zstd-ffi`). They cover every level on a standard corpus, thousands of
  random advanced-parameter configurations, streaming schedules with flushes
  and wrapped windows, LDM, short destinations, and a build mode that forces
  index overflow correction on both sides.
- Decoder tests: reference frames through every entry point, upstream's
  golden vectors, truncation and bit-flip corruption, limits.
- AFL++ targets in `fuzz/afl`. Encoder targets: differential, output
  capacity, sessions, lifecycle, parallel and parameter parsing. Decoder
  targets: raw input against the reference decoder, chunked sessions, and
  reference-written frames. ClusterFuzzLite runs the same targets under
  libFuzzer and AddressSanitizer daily and on pull requests that touch code.
- Miri over the code that reaches `unsafe` in dependencies (the
  `fearless_simd` vector kernels and `borrowed-buf` uninitialized output),
  and AddressSanitizer over the integration tests, on every release tag.
- 100% function coverage, checked with `cargo llvm-cov`.
- [Formal verification](architecture/formal-verification.md) in Lean 4 of
  parts of the format against RFC 8878 itself, not the C library: Charon and
  Aeneas translate the Rust code, and machine-checked proofs show that the
  frame and block headers, `Number_of_Sequences`, the length and offset code
  tables, the default distributions, the repeat offset update and the FSE
  state transitions follow the RFC, that encoder and decoder agree on them,
  and that the translated code cannot panic or overflow on the inputs the
  proofs cover.

See [`architecture/`](architecture/README.md) for how the code is organized
and verified.

## Not supported

- External sequence producers
- Legacy (v0.x) frames
- Byte identity with zstd's multithreaded mode

## Contributing

Bug reports, differential test cases, fuzz findings and pull requests are
welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) for the checks a
change must pass, and follow the [Code of Conduct](CODE_OF_CONDUCT.md). If
compression output differs from zstd 1.5.7, open an issue with the
[Output differs from zstd][mismatch] template.

Report security vulnerabilities privately as described in
[SECURITY.md](SECURITY.md), not as public issues.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <https://opensource.org/licenses/MIT>)

at your option. The encoder is a port of zstd, used under its BSD license;
see [NOTICE](NOTICE).

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

[crate]: https://crates.io/crates/mzstd
[api]: https://docs.rs/mzstd
[ci]: https://github.com/M-Compressors/mzstd/actions/workflows/ci.yml
[mismatch]: https://github.com/M-Compressors/mzstd/issues/new?template=output_mismatch.yml
[benchmarks]: ./docs/benchmarks/README.md
[bench-levels]: ./docs/benchmarks/encoders/README.md
[bench-csv]: ./docs/benchmarks/encoder-comparison.csv
[bench-chart]: ./docs/benchmarks/encoders/charts/overview.svg
[benchmarking]: ./docs/benchmarking.md
[decoder-benchmarks]: ./docs/benchmarks/decoders/README.md
[decoder-bench-csv]: ./docs/benchmarks/decoder-comparison.csv
[decoder-bench-chart]: ./docs/benchmarks/decoders/charts/overview.svg
[decoder-bench-report]: ./docs/benchmarks/decoder-comparison.md
[lzbench-commit]: https://github.com/inikep/lzbench/tree/a6fa8aa758932a02841c0cc68c07e5abf8870951
[lzbench-run]: ./docs/benchmarks/lzbench-2026-11-09.md
