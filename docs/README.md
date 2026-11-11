# User guide

[Getting started](getting-started.md) · [Examples](examples/README.md) · [API reference](https://docs.rs/mzstd)

| Task | Read |
| --- | --- |
| Decode and set resource limits | [Decompression](#decompression) |
| Choose compression settings | [Configuration](#configuration) |
| Control allocations | [Output buffers](#output-buffers) and [reuse](#reusing-memory) |
| Connect synchronous I/O or drive a session | [Streaming](#streaming-and-completion) |
| Drive compression with caller buffers | [Sans I/O compression](sans-io-compression.md) |
| Decode arriving chunks and resource events | [Sans I/O decompression](sans-io-decompression.md) |
| Connect readers and writers for every format | [Read / Write I/O](io.md) |
| Share a dictionary or use magicless frames | [Dictionaries and extended formats](dictionaries.md) |
| Package several resources | [Framing containers](frames.md) |
| Split a job across workers | [Parallel compression](parallel.md) |
| Compare libraries | [Benchmarks](benchmarks/README.md) |

## Decompression

`Decompressor` owns reusable state. `decompress` returns a Vec,
`decompress_into` appends a result, and `decompress_to_slice` writes into caller
storage. Sessions report consumed/produced counts; `reader` and `writer` adapt
synchronous I/O. Construction is infallible: `Decompressor::new(config)`.

The default accepts concatenated Zstandard frames and skips skippable frames.
Content checksums are always verified. The default window limit is 128 MiB;
input and output byte budgets are unlimited. Set limits for your application:

```rust
use mzstd::{DecodeLimits, DecoderConfig, Decompressor};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let limits = DecodeLimits::default()
        .with_max_window_log(23)?
        .with_max_input_bytes(Some(1 << 20))
        .with_max_output_bytes(Some(8 << 20));
    let mut decoder = Decompressor::new(DecoderConfig::default().with_limits(limits));
    let frame = mzstd::Compressor::default().compress(b"bounded payload")?;
    assert_eq!(decoder.decompress(&frame)?, b"bounded payload");
    Ok(())
}
```

The window log accepts 10–31. Input/output budgets count the whole operation,
including concatenated frames; they are not process-memory ceilings.
`FrameMode::Single` stops a session after one Zstandard or skippable frame.
One-shot decoding rejects trailing bytes in that mode. `FrameHeaderInfo` inspects
headers without decoding the payload. See [decoder mechanics](../architecture/decompressor.md).

## Configuration

`EncoderConfig` holds settings shared by successive frames:

| Setting | Type / method | Behavior |
| --- | --- | --- |
| Compression level | `Level` / `with_level` | -131072–22; default 3; 0 aliases 3 |
| Advanced compression parameters | `CompressionParams` / `with_params` | Window, hash/chain/search logs, match length, target length, strategy |
| Long distance matching | `LongDistanceMatching` / `with_long_distance_matching` | Automatic, disabled, or explicit `LdmParams` |
| Content checksum | `with_checksum` | Add a checksum to the frame |
| Frame format | `with_format` | Standard or magicless |
| Target compressed block size | `with_target_compressed_block_size` | Optional `NonZeroU32` target, like `ZSTD_c_targetCBlockSize` |

Explicit parameters override the level as `ZSTD_CCtx_setParameter` does.
`Compressor::new` validates parameter combinations. `CParameter` provides a
validated parameter-by-ID interface for integrations; see [OpenZL integration](../architecture/openzl-integration.md).

`StreamConfig` carries an `InputSize`: unknown by default, or
`InputSize::Exact(input.len() as u64)` when the full length is known. Declared
sizes are checked. Session scheduling is part of the byte-identity contract.

## Output buffers

`compress` returns a new Vec. `compress_into` appends to a supplied vector and
returns the range containing the new frame. Existing bytes remain intact and
its original length is restored on failure.

`compress_to_slice` returns the number of bytes written. Allocate a conservative
buffer using `Compressor::max_compressed_size`:

```rust
use mzstd::Compressor;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = b"data to compress";
    let mut encoder = Compressor::default();
    let mut output = vec![0; Compressor::max_compressed_size(input.len())?];
    let written = encoder.compress_to_slice(input, &mut output)?;
    output.truncate(written);
    assert_eq!(output, encoder.compress(input)?);
    Ok(())
}
```

An undersized slice returns an error and can contain a partial prefix. Retry
the complete operation with sufficient storage. `compress_to_uninit` and
`decompress_to_uninit` accept caller-owned `MaybeUninit<u8>` destinations;
read only the prefix reported as initialized. See [decoder output contracts](../architecture/decoder-uninit-output.md).

## Reusing memory

Keep a compressor or decompressor across requests to reuse workspace. Caller
output capacity is managed separately. A retention policy controls storage
kept between operations:

| Retention policy | Effect |
| --- | --- |
| `Aggressive` (default) | Keeps allocated buffers |
| `Bounded { max_bytes }` | Releases storage when retained accounting exceeds the ceiling |
| `ReleaseAll` | Releases storage after an operation |

Use `Compressor::builder(config).with_retention(policy).build()` or the decoder
builder, and `trim(policy)` for immediate cleanup. `retained_bytes()` excludes
caller output and allocator overhead. A retention ceiling is not a peak memory
limit. Use a separate codec per simultaneous independent operation, or
[parallel compression](parallel.md) to split one input across workers.

## Streaming and completion

`encoder.reader(source, stream)` pulls plain input and yields compressed bytes.
`encoder.writer(sink, stream)` accepts plain input and delivers compressed bytes.
Decoder adapters take just the source or sink: `decoder.reader(source)` and
`decoder.writer(sink)`. Their frame mode and limits come from `DecoderConfig`.

Read a reader to EOF. Call `finish` on a writer: dropping it performs no
finalization. Encoder `flush` creates an intermediate decoding boundary and can
change output size; decoder `flush` drains output without declaring final input.
`try_finish` supports retrying a sink failure. Consuming `finish` returns the sink
or a `FinishError`; `into_parts` recovers the error and writer, while `into_error`
discards the writer. Reader `into_parts` preserves source read-ahead.

For direct control, `encoder.start(stream)` creates a borrowed `EncoderSession`.
`decoder.start()` creates a `DecoderSession`. Neither performs I/O.

| Operation / status | Meaning |
| --- | --- |
| `Operation::Continue` | Accept input without ending the frame |
| `Operation::Flush` | Make accepted input decodable without ending the frame |
| `Operation::End` | End the frame after this input |
| `EncoderStatus::NeedsInput` | Supply more input or explicit flush/finish |
| `EncoderStatus::NeedsOutput` | Supply output space and retain unconsumed input |
| `EncoderStatus::Finished` | All final output has been delivered |

Advance input by `Progress::consumed` and deliver only `output[..produced]`.
Finish can take several calls. Decoder sessions use `DecodeOperation` and
`DecoderStatus`; use `Finish` when no more compressed input can arrive.
Owned session variants move the codec into a session without a borrowed-owner
lifetime. See [Sans I/O compression](sans-io-compression.md) and
[decompression](sans-io-decompression.md) for complete loops.

## Output compatibility

Single-threaded one-shot compression is byte-identical to zstd **1.5.7**
`ZSTD_compress2` with equivalent input, parameters, and dictionary. Sessions
match `ZSTD_compressStream2` driven with the same chunks, output capacities,
flushes, and pledged size. Different schedules are not interchangeable byte
oracles. SIMD selection and workspace reuse preserve observable behavior.

Parallel compression writes valid, deterministic Zstandard frames, but its bytes
differ from serial compression and zstd's multithreaded mode. Framing is this
crate's container format built from Zstandard and skippable frames. See
[compatibility and validation](correctness.md).

## Errors and further reading

`ConfigError` covers encoder configuration, `DecodeConfigError` decoder limits,
`DictionaryError` dictionary preparation, and `EncodeError` / `DecodeError`
codec failures. Failed decoder calls report exact progress in `DecodeFailure`;
output delivered before final validation can be partial. I/O adapters return
`std::io::Error`, with the codec error as its source.

- [Dictionaries and extended formats](dictionaries.md)
- [Parallel compression](parallel.md)
- [Framing containers](frames.md)
- [Benchmark results by compression level](benchmarks/README.md)
- [Running benchmarks and profiling](benchmarking.md)
- [Development checks](development.md)
- [Compressor mechanics](../architecture/compressor.md)
