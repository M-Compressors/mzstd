# Choose your API

Compression and decompression expose the same I/O shapes. Pick the row that
matches how your application already moves bytes, then follow its guide.
New to the crate? Start with [getting started](getting-started.md).

## Both codecs

| I/O shape | Compression | Decompression | Guide |
| --- | --- | --- | --- |
| Return a new `Vec<u8>` | `Compressor::compress` | `Decompressor::decompress` | [Getting started](getting-started.md) |
| Append to an existing Vec | `compress_into` | `decompress_into` | [Buffers and memory reuse](memory.md) |
| Write into a caller-owned slice | `compress_to_slice`, `compress_to_uninit` | `decompress_to_slice`, `decompress_to_uninit` | [Buffers and memory reuse](memory.md#output-buffers) |
| Pull output through `std::io::Read` | `Compressor::reader` | `Decompressor::reader` | [Read / Write I/O](io.md) |
| Push input through `std::io::Write` | `Compressor::writer` | `Decompressor::writer` | [Read / Write I/O](io.md) |
| Drive input and output incrementally | `start` → `EncoderSession` | `start` → `DecoderSession` | Sans I/O [compression](sans-io-compression.md), [decompression](sans-io-decompression.md) |
| Same, with the session owning the codec | `into_session` → `EncoderSessionOwned` | `into_session` → `DecoderSessionOwned` | Sans I/O [compression](sans-io-compression.md), [decompression](sans-io-decompression.md) |
| Any of the above with a dictionary | `compress_with_dictionary*`, `start_with_dictionary`, `into_session_with_dictionary`, `writer_with_dictionary`, `reader_with_dictionary` | `decompress_with_dictionary*`, `start_with_dictionary`, `into_session_with_dictionary`, `writer_with_dictionary`, `reader_with_dictionary` | [Dictionaries](dictionaries.md) |

`Compressor` and `Decompressor` own reusable working state. Keep one alive
across payloads when allocation reuse matters.

## Parallel compression

`ParallelCompressor` (in `mzstd::compressor::parallel`) is a separate execution
strategy for compression: it splits one frame into independent segments and
hands them to your executor as tasks. The library does not create a thread
pool. The output is a valid Zstandard frame, decoded by the ordinary
`Decompressor`. Requires `std`.

| I/O shape | Entry point |
| --- | --- |
| Split one known input across your threads | `prepare_slice` / `prepare_source` → `ScopedParallelBatch` |
| Stream chunked input across your threads | `start` → `ParallelEncoderSession` + `ParallelTaskQueue` |
| Same, with the session owning the compressor | `into_session` → `ParallelEncoderSessionOwned` |
| Push or pull through `std::io` across your threads | `writer`, `reader` |

See [parallel compression](parallel.md) for scoped threads, Rayon, sources,
staging, and streaming.

## Guides by task

| Task | Read |
| --- | --- |
| Pick a level, parameters, or Cargo features | [Configuration and features](configuration.md) |
| Control allocations and retained memory | [Buffers and memory reuse](memory.md) |
| Decode untrusted input with resource limits | [Decompression and limits](decompression.md) |
| Understand readers, writers, sessions, and `finish` | [Streaming and completion](streaming.md) |
| Connect readers and writers for every format | [Read / Write I/O](io.md) |
| Drive compression with caller buffers | [Sans I/O compression](sans-io-compression.md) |
| Decode arriving chunks and resource events | [Sans I/O decompression](sans-io-decompression.md) |
| Split a job across workers | [Parallel compression](parallel.md) |
| Package several resources | [Framing containers](frames.md) |
| Share a dictionary or use magicless frames | [Dictionaries and extended formats](dictionaries.md) |
| Copy a complete program | [Examples](examples/README.md) |

## Errors

`ConfigError` covers encoder configuration, `DecodeConfigError` decoder limits,
`DictionaryError` dictionary preparation, and `EncodeError` / `DecodeError`
codec failures. Failed decoder calls report exact progress in `DecodeFailure`;
output delivered before final validation can be partial. I/O adapters return
`std::io::Error`, with the codec error as its source.

## Reference

- [API reference on docs.rs](https://docs.rs/mzstd)
- [Compatibility and validation](correctness.md): what is byte-identical to
  zstd 1.5.7, and how it is checked
- [Benchmark results](benchmarks/README.md) for every measured level, with raw data
- [Architecture](../architecture/README.md): how the implementation works
- [Development](development.md) and [benchmarking](benchmarking.md) for
  contributors
