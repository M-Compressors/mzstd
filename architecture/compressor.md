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
| `dictionary` | `DictionaryBuilder`, `PreparedDictionary` (re-exported from `mzstd::dictionary`); see [dictionaries](dictionaries.md). |
| `encoder` | `Compressor`, `CompressorBuilder` (`with_retention`, `with_backend`), one-shot calls (with and without a dictionary), `start`/`into_session` and their `_with_dictionary` forms, retention. The backend (default: the detected SIMD level) is stored in the `Engine` for the match finders. |
| `session` | `EncoderSession` (optionally borrowing a dictionary), `EncoderSessionOwned<D = PreparedDictionary>` (optionally owning one; `into_parts`), `StreamConfig`, `InputSize`, `Operation`, `EncoderStatus`, `Progress`. |
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
`ZSTD_c_*` parameter does. Every numeric `with_*` setter of
`CompressionParams` and `LdmParams` stores `0` as `None`, as
`ZSTD_CCtx_setParameter(.., 0)` resets a parameter to its default, so the
last value given wins and a caller can apply a list of zstd-style parameters
directly, one setter call each. Lowering maps `None` to `0`, which the core
already treated as "derive from the level", so frames are unchanged.

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

The slice entry points use `core::compress_once_into`, which is
`ZSTD_compress2` with a stable input: the input is a segment of its own
(`Segment::Input(id)`, with a fresh id per call), so the engine matches
directly in caller memory and never copies it. `compress_to_uninit` runs the
same pipeline through `core::compress_once_uninit`, which writes into the
caller's `MaybeUninit` memory via `core::output::UninitOutput`: the
destination is zeroed just ahead of each block instead of being copied from
a temporary buffer (see [encoder core](encoder-core.md#destinations)). It
succeeds, fails and writes exactly as `compress_to_slice`.

`compress` and `compress_into` use `core::compress_once_append`: they reserve
`compressBound` capacity, but for inputs larger than 128 KiB initialize only
the frame header and the room required by each next block. The shared engine
pipeline writes through `core::output::VecOutput`, reusing the initialized
tail between blocks. Highly compressible frames therefore avoid zeroing a
destination as large as their input. Inputs up to 128 KiB keep the slice path.
`compress_into` preserves the existing prefix and rolls the vector back on
failure. Returned capacity and retention policy are unchanged.

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
    Open --> Failed: core error, or first End contradicting a declared size
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
- A first call that is already `End` deviates from the reference on
  purpose. `ZSTD_compressStream2` replaces the pledged size with that call's
  input length and writes a frame for it; the session instead fails with
  `InputSizeMismatch` before touching the stream, as it does for every later
  mismatch, so a truncated or overlong input never produces a frame. The
  check lives in `session::Core`; the core stream remains a faithful port.
  When the length matches, the bytes are the reference's.
- `process_uninit` is `process` over a `[MaybeUninit<u8>]` destination: the
  same generic `Core::process` and `Stream::compress`, with blocks written
  straight to the caller zeroed just ahead and buffered output copied in.
  Bytes, counts, statuses and errors equal `process`, and the two may be
  mixed on one frame; nothing is allocated per call. Past `produced` the
  output may hold zeros or the engine's scratch bytes (a block it replaced
  with a raw or RLE block, for instance), exactly as a slice would; only
  the reported prefix is meaningful.

```mermaid
flowchart TD
    Process["Core::process(input, output: &mut D, op)"] --> St{state}
    St -- Failed --> Invalid[InvalidState]
    St -- Finished --> Done["Finished, nothing moved"]
    St -- Open --> First{"op == End, stream idle, declared != input.len()?"}
    First -- yes --> Mismatch["state = Failed, reset stream, InputSizeMismatch"]
    First -- no --> Stream["Stream::compress (D = [u8] or [MaybeUninit u8])"]
    Stream -- CoreError --> Failed["state = Failed, mapped EncodeError"]
    Stream -- step --> Status[NeedsInput / NeedsOutput / Finished]
```
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
`EncoderWriter` pulls compressed bytes into an owned buffer of
`ZSTD_CStreamOutSize` bytes (a full block's compress bound plus a block
header and a checksum). A session writes a block straight into the
caller's output whenever the room left there holds the block's bound, so
every full block lands in the writer's buffer without passing through the
session's internal output buffer; only a frame's end or a partly drained
buffer falls back to it and the Flush stage. With 128 KiB, a few hundred
bytes short of the bound, every block took that detour and its copy.

## Errors

```mermaid
flowchart LR
    Core[CoreError, private] -->|from_core| Encode[EncodeError]
    Config[ConfigError] --> Build[Compressor::new / reconfigure]
    Encode --> Io["std::io::Error (adapters)"]
    Encode -->|OutputTooSmall| Caller
    Encode -->|InputSizeMismatch| Caller
    Encode -->|InvalidState| Caller
    Encode -->|DictionaryMismatch| Caller
    Encode -->|Internal| Caller
```

`ConfigError::OutOfRange` names the `Parameter`, the value and its bounds.
`EncodeError::Internal` stands for a core failure that valid input cannot
produce; it exists so such a bug is an error rather than a panic.
`EncodeError::Bound` reports a `compressBound` overflow for inputs near
`usize::MAX`. `EncodeError::DictionaryMismatch` reports a dictionary
prepared for another configuration (frame options aside); it is checked
before the engine is touched.

## Retention

`RetentionPolicy::Aggressive` keeps every table and buffer between frames,
`Bounded(n)` releases them when they exceed `n` bytes, and `ReleaseAll`
releases them after each operation. `trim` applies a policy once.
`retained_bytes` sums the engine's tables and the stream buffers. Reuse never
changes output: a reused engine's frames equal a fresh one's (checked by
`compressor_lifecycle` and `tests/reuse_probe.rs` against the reference).

## Known gaps

- No `targetCBlockSize`, no magicless frames, no external
  sequence producers, and no explicit `ZSTD_c_useRowMatchFinder`,
  `ZSTD_c_blockSplitterLevel` or `ZSTD_c_maxBlockSize` control in the public
  API; the core models them only at their automatic values.
- The stable-buffer streaming modes (`ZSTD_c_stableInBuffer`,
  `ZSTD_c_stableOutBuffer`) are not exposed; sessions always behave as the
  reference's buffered mode.
- Streaming at the fast levels trails zstd on the `stream/mixed-4MiB`
  bench (level 1: 7.4 ms against 6.1 ms on the i7-13700KF before the
  branch-free segment select, about 2% less since) because of the extDict
  fast finder, which most blocks use once the stream buffer wraps: it
  executes about 1.6x the reference's instructions (73.5M against about
  45M per 4 MiB stream; stack spills and bounds checks in its search loop)
  and mispredicts 136K branches against 95K. The session, window and
  output layers cost under 0.02M instructions per 4 MiB call, at parity
  with the reference.
