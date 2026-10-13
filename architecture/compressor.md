# Compressor

The public compression layer: configuration, the `Compressor` owner, one-shot
calls, incremental sessions, `std::io` adapters, errors and memory retention.
Everything here is a thin shell over the private engine described in
[encoder core](encoder-core.md); the bytes it produces are decided there.

## Modules and ownership

```mermaid
graph TD
    Root["mzstd (re-exports)"] --> Config[compressor::config]
    Root --> Encoder[compressor::encoder]
    Root --> Session[compressor::session]
    Root --> Error[compressor::error]
    Root --> IO["compressor::io (std)"]
    Root --> Parallel["compressor::parallel (std)"]
    Config --> Internal[compressor::internal: lowering]
    Internal --> Core["compressor::core (private)"]
    Encoder --> Core
    Session --> Core
    IO --> Session
    Parallel --> Core
    Encoder --> Retention[retention::RetentionPolicy]
```

| Module | Owns |
| --- | --- |
| `config` | `Level`, `Strategy`, `CompressionParams`, `LdmParams`, `LongDistanceMatching`, `EncoderConfig`, `Parameter`, `ConfigError`, `SizeOverflow`, and validation against the reference's bounds. |
| `internal` | `requested_params`: the single translation from `EncoderConfig` to the core's `RequestedParams` (the reference's `ZSTD_CCtx_params`). |
| `encoder` | `Compressor`, `CompressorBuilder` (`with_retention`, `with_backend`), one-shot calls, `start`/`into_session`, retention. The backend (default: the detected SIMD level) is stored in the `Engine` for the match finders. |
| `session` | `EncoderSession`, `EncoderSessionOwned`, `StreamConfig`, `InputSize`, `Operation`, `EncoderStatus`, `Progress`. |
| `error` | `EncodeError` and the mapping from the core's private `CoreError`. |
| `io` | `EncoderReader`, `EncoderWriter`, `EncoderReaderParts` (re-exported from `mzstd::io`). `EncoderReader::into_parts` returns the source with the bytes read from it that the session had not accepted; `ParallelEncoderReader::into_parts` returns the same type. |
| `parallel` | See [parallel compression](parallel-compression.md). |

`Compressor` owns a boxed `core::cctx::Engine` (match state, entropy tables,
LDM state, sequence store) and a `core::stream::Stream` (the streaming input
and output buffers). Sessions borrow the compressor mutably, or own it in the
`Owned` variant, so one engine serves one frame at a time.

## Types

```mermaid
classDiagram
    class EncoderConfig {
        level: Level
        params: CompressionParams
        ldm: LongDistanceMatching
        checksum: bool
        content_size: bool
        size_hint: Option~u32~
    }
    class Compressor {
        config: EncoderConfig
        requested: RequestedParams
        retention: RetentionPolicy
        backend: Backend
        engine: Box~Engine~
        stream: Stream
        +backend() Backend
        +compress(&[u8]) Vec~u8~
        +compress_into(&[u8], &mut Vec~u8~) Range
        +compress_to_slice(&[u8], &mut [u8]) usize
        +compress_to_uninit(&[u8], &mut [MaybeUninit~u8~]) usize
        +start(StreamConfig) EncoderSession
        +into_session(StreamConfig) EncoderSessionOwned
        +reconfigure(EncoderConfig)
        +trim(RetentionPolicy)
        +max_compressed_size(usize)$ usize
    }
    class EncoderSession {
        +process(&[u8], &mut [u8], Operation) Progress
        +process_uninit(...)
        +flush(...)
        +finish(...)
        +is_finished() bool
    }
    EncoderConfig --> Compressor : validated by build
    Compressor --> EncoderSession : start
    Compressor --> EncoderSessionOwned : into_session
```

`EncoderConfig::default()` is level 3 with content size on and checksum off,
like the reference's defaults; `Compressor::default()` uses it without
validation because it has no explicit parameter. Explicit parameters are
`Option`s: `None` keeps the level's value, exactly as an unset
`ZSTD_c_*` parameter does.

## One-shot compression

```mermaid
sequenceDiagram
    participant Caller
    participant C as Compressor
    participant E as core::compress_once_into
    participant Eng as Engine
    Caller->>C: compress_to_slice(input, dst)
    C->>E: requested params, input, dst
    E->>Eng: begin(resolve(input.len()))
    E->>Eng: compress_end(whole input as one segment)
    Eng-->>E: frame size or CoreError
    E-->>C: Result
    C->>C: apply_retention()
    C-->>Caller: size or EncodeError
```

All one-shot entry points funnel into `core::compress_once_into`, which is
`ZSTD_compress2` with a stable input: the input is a segment of its own
(`Segment::Input(id)`, with a fresh id per call), so the engine matches
directly in caller memory and never copies it. `compress` sizes its output
with `compressBound`; `compress_into` appends and rolls the vector back on
failure; `compress_to_uninit` writes through a temporary initialized buffer
only as large as the destination.

A destination smaller than the frame fails with `OutputTooSmall` exactly when
`ZSTD_compress2` fails with `dstSize_tooSmall`; the reference needs room for
the largest frame header and some slack, so an output exactly as long as the
frame may be refused. The `output_capacity` fuzz target checks this.

## Sessions

A session is one frame driven by `process(input, output, operation)`, which
is `ZSTD_compressStream2` in buffered mode: the same sequence of inputs,
output capacities and operations produces the same bytes.

```mermaid
stateDiagram-v2
    [*] --> Open: start / into_session
    Open --> Open: process(Continue) / Flush with output drained
    Open --> Open: Flush or End needing output (NeedsOutput)
    Open --> Finished: End with nothing left to write
    Open --> Failed: core error
    Finished --> Finished: further calls report Finished, consume nothing
    Failed --> Failed: further calls return InvalidState
    Finished --> Open: reinit (owned session)
    Failed --> Open: reinit (owned session)
```

- `InputSize::Exact(n)` pledges the size (`ZSTD_CCtx_setPledgedSrcSize`):
  it is written in the header and a mismatch fails with
  `InputSizeMismatch`. `InputSize::Unknown` leaves the header without a
  content size unless the whole input arrives with the first `End`, as in the
  reference.
- `process_uninit` writes into `MaybeUninit` output through the same path.
- `flush` and `finish` are `process` with `Flush` and `End`.
- Dropping a session mid-frame leaves the compressor reusable: the next
  `start` resets the stream state.

`EncoderStatus` reports `NeedsInput`, `NeedsOutput` (internal output is still
pending) or `Finished`. `Progress` carries the bytes consumed and produced by
the call.

## I/O adapters (`std`)

`EncoderWriter` wraps a `Write` sink: `write` feeds `Continue`, `flush` feeds
`Flush` and drains, and `finish`/`try_finish` feed `End`, returning the sink.
`finish` reports a failure as `FinishError`, which keeps the writer so the
caller can retry or recover the sink. `EncoderReader` wraps a `Read` source
and yields the compressed frame; it drives the session with `End` once the
source is exhausted. Both use internal buffers of one block.

## Errors

```mermaid
flowchart LR
    Core[CoreError, private] -->|from_core| Encode[EncodeError]
    Config[ConfigError] --> Build[Compressor::new / reconfigure]
    Encode --> Io["std::io::Error (adapters)"]
    Encode -->|OutputTooSmall| Caller
    Encode -->|InputSizeMismatch| Caller
    Encode -->|InvalidState| Caller
    Encode -->|Internal| Caller
```

`ConfigError::OutOfRange` names the `Parameter`, the value and its bounds.
`EncodeError::Internal` stands for a core failure that valid input cannot
produce; it exists so such a bug is an error rather than a panic.
`EncodeError::Bound` reports a `compressBound` overflow for inputs near
`usize::MAX`.

## Retention

`RetentionPolicy::Aggressive` keeps every table and buffer between frames,
`Bounded(n)` releases them when they exceed `n` bytes, and `ReleaseAll`
releases them after each operation. `trim` applies a policy once.
`retained_bytes` sums the engine's tables and the stream buffers. Reuse never
changes output: a reused engine's frames equal a fresh one's (checked by
`compressor_lifecycle` and `tests/reuse_probe.rs` against the reference).

## Known gaps

- No dictionaries, no `targetCBlockSize`, no magicless frames, no external
  sequence producers, and no explicit `ZSTD_c_useRowMatchFinder`,
  `ZSTD_c_blockSplitterLevel` or `ZSTD_c_maxBlockSize` control in the public
  API; the core models them only at their automatic values.
- The stable-buffer streaming modes (`ZSTD_c_stableInBuffer`,
  `ZSTD_c_stableOutBuffer`) are not exposed; sessions always behave as the
  reference's buffered mode.
