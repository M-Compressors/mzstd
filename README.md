# mzstd

Zstandard (RFC 8878) compression and decompression in safe Rust.

[![Crates.io](https://img.shields.io/crates/v/mzstd.svg)][crate]
[![docs.rs](https://docs.rs/mzstd/badge.svg)][api]
[![Tests](https://github.com/M-Compressors/mzstd/actions/workflows/ci.yml/badge.svg?branch=main)][ci]
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

- **Byte-identical encoder.** For the same input and parameters,
  single-threaded compression writes exactly the bytes of zstd 1.5.7:
  one-shot calls match `ZSTD_compress2`, and sessions match
  `ZSTD_compressStream2` driven with the same chunks, output capacities and
  flushes. This holds for levels -131072 to 22, the explicit compression
  parameters, long distance matching, checksums, content sizes, pledged sizes
  and size hints.
- **Independent decoder** written from the format specification. It decodes
  every frame zstd writes apart from dictionary frames, with bounded window,
  input and output limits.
- **Parallel compression** with tasks you run on your own executor. Frames
  are valid Zstandard frames and are identical for any task count, but they
  are not byte-identical to zstd's multithreaded mode.
- **No `unsafe`, no dependencies on zstd.** It runs with `no_std` + `alloc`,
  and there is a C ABI in `mzstd-ffi`.

```toml
[dependencies]
mzstd = "0.1"
```

## Usage

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
| Split one known input across your threads | `ParallelCompressor::prepare_slice` → `ScopedParallelBatch` | — |
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
| `hotpath`, `hotpath-cpu`, `hotpath-alloc` | no | profiling anchors |

## Performance

Speed was not a goal of this first version: both codecs are scalar ports
without SIMD. As a reference point, here is `cargo bench --bench compress`
and `--bench decompress` on an Apple M5 Pro with rustc 1.99 (Criterion
medians; both sides write and read identical frames):

| Case | mzstd | zstd 1.5.7 |
| --- | --- | --- |
| compress text 1 MiB, level 1 | 398 MiB/s | 694 MiB/s |
| compress text 1 MiB, level 3 | 418 MiB/s | 775 MiB/s |
| compress text 1 MiB, level 9 | 54.5 MiB/s | 105 MiB/s |
| compress text 1 MiB, level 19 | 3.8 MiB/s | 5.5 MiB/s |
| compress binary 1 MiB, level 3 | 197 MiB/s | 350 MiB/s |
| decompress text 4 MiB (level 3 frame) | 716 MiB/s | 2.19 GiB/s |
| decompress binary 4 MiB (level 3 frame) | 270 MiB/s | 1.22 GiB/s |

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
  reference-written frames.
- 100% function coverage, checked with `cargo llvm-cov`.

See [`architecture/`](architecture/README.md) for how the code is organized
and verified.

## Not supported

- Dictionaries
- Magicless frames
- `targetCBlockSize`
- External sequence producers
- Legacy (v0.x) frames
- Byte identity with zstd's multithreaded mode

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
