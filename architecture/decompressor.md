# Decompressor

The `decompression` feature enables the public `decompressor` module: a
safe-Rust Zstandard (RFC 8878) decoder with one-shot, incremental and
`std::io` APIs, with or without a dictionary. The public module owns
configuration, decoder ownership, sessions, typed errors and the I/O
adapters; `crate::dictionary` owns the public dictionary types; the private
`decompressor::core` owns parsing and regeneration. The decoder needs only
`alloc`; the `io` adapters and the `std::io::Error` conversion exist unless
`no_std` is enabled. There is no `unsafe` code. Compressed blocks run
`fearless_simd` kernels on the `Backend` chosen when the decoder is built;
the fast paths are described in [decoder fast paths](decoder-performance.md).

The reference this decoder is checked against is zstd 1.5.7, vendored in
`zstd-ffi/vendor/zstd`; the format text is
[`zstd_compression_format.md`](../zstd-ffi/vendor/zstd/doc/zstd_compression_format.md).
How it is verified is described in
[decoder verification](decoder-verification.md).

## Module map

```mermaid
graph TD
    Lib[mzstd root re-exports] --> API
    subgraph public[decompressor]
        API[config / decoder / session / error]
        IO[io: DecoderReader / DecoderWriter]
    end
    IO --> API
    IO --> Finish[crate::finish_error::FinishError]
    API --> Retention[crate::RetentionPolicy]
    API --> Stream
    API --> Dict["crate::dictionary: DecodeDictionary, DictionaryRef, DictionaryId"]
    Dict --> DCore
    subgraph core[decompressor::core, private]
        Stream[stream: Stream state machine + decode_all] --> Frame[frame: FrameHeader, FrameDecoder]
        Stream --> History[history: output buffer = window]
        Frame --> DCore[dictionary: DecoderEntropy, DictView]
        Frame --> Block[block: BlockHeader, BlockDecoder]
        Block --> Literals[literals: LiteralsDecoder, Literals]
        Block --> Sequences[sequences: SequenceTables, execute]
        Literals --> Huffman[huffman: HuffmanTable, PairTable and LockstepRows, X1 and pair rounds]
        Huffman --> HufRef["huffman::reference: the reference's stream-end verdicts, TableKind"]
        Huffman --> Fse[fse: distributions, FseTable, SeqTable, weights]
        Sequences --> Fse
        Sequences --> Copy[copy: wildcopies, short-offset shuffles]
        Huffman --> Bits[bits: BitContainer, Sentinel, Parked, Reverse / Forward readers]
        Stream --> Ahead[ahead: WriteAhead and prefetch_write, next block's output prefetch]
        Huffman --> Ahead
        Sequences --> Ahead
        Fse --> Bits
        Sequences --> Bits
    end
    API --> Backend[crate::Backend: fearless_simd level]
    Frame --> Xxh[shared::xxhash::Xxh64]
    Core2[core modules] -.-> Format[shared::format wire constants]
```

`shared::format` (wire constants, length-code extra bits, predefined
distributions, `highbit32`) and `shared::xxhash` are shared with the
encoder; everything else is decoder-private. Length-code baselines are
derived at compile time from the shared extra-bit tables
(`sequences::baselines`).

## Public API

```mermaid
classDiagram
    class Decompressor {
        +new(DecoderConfig) Decompressor
        +builder(DecoderConfig) DecompressorBuilder
        +config() DecoderConfig
        +reconfigure(DecoderConfig)
        +retention() RetentionPolicy
        +backend() Backend
        +retained_bytes() usize
        +trim(RetentionPolicy)
        +decompress(src) Vec~u8~
        +decompress_into(src, Vec) Range
        +decompress_to_slice(src, slice) usize
        +decompress_to_uninit(src, uninit slice) usize
        +start() DecoderSession
        +into_session() DecoderSessionOwned
        +reader(R) DecoderReader
        +writer(W) DecoderWriter
        +decompress_with_dictionary(dict, src) Vec~u8~
        +decompress_with_dictionary_into(dict, src, Vec) Range
        +decompress_with_dictionary_to_slice(dict, src, slice) usize
        +decompress_with_dictionary_to_uninit(dict, src, uninit slice) usize
        +start_with_dictionary(dict) DecoderSession
        +into_session_with_dictionary(D) DecoderSessionOwned~D~
        +reader_with_dictionary(R, dict) DecoderReader
        +writer_with_dictionary(W, dict) DecoderWriter
    }
    class DecodeDictionary {
        +new(bytes, DictionaryContent) Result
        +try_from(bytes) Result
        +id() Option~DictionaryId~
        +content_len() usize
        +retained_bytes() usize
    }
    class DictionaryRef {
        <<enum, Copy>>
        Decode(&DecodeDictionary)
    }
    class DecompressorBuilder {
        +with_retention(RetentionPolicy)
        +with_backend(Backend)
        +build() Decompressor
    }
    class DecoderConfig {
        +DEFAULT
        +with_frame_mode(FrameMode)
        +with_limits(DecodeLimits)
    }
    class DecodeLimits {
        +DEFAULT
        +with_max_window_log(u8) Result
        +with_max_input_bytes(Option)
        +with_max_output_bytes(Option)
        +max_window_size() u64
    }
    class FrameMode {
        <<enum>>
        Concatenated
        Single
    }
    class DecoderSession {
        +process(input, output, DecodeOperation) DecodeProgress
        +process_uninit(input, uninit output, DecodeOperation) DecodeProgress
        +flush(output)
        +finish(output)
        +is_finished() bool
        +total_in() u64
        +total_out() u64
        +frames_decoded() u64
        +window_size() Option~u64~
    }
    class DecoderSessionOwned~D = DecodeDictionary~ {
        +process / process_uninit / flush / finish
        +reinit()
        +into_decompressor() Decompressor
    }
    Decompressor --> DecoderConfig
    DecoderConfig --> FrameMode
    DecoderConfig --> DecodeLimits
    DecompressorBuilder --> Decompressor
    DecoderSession --> Decompressor : borrows
    DecoderSession --> DictionaryRef : borrows, optional
    DecoderSessionOwned --> Decompressor : owns
    DecoderSessionOwned --> DecodeDictionary : owns D, optional
    DictionaryRef --> DecodeDictionary
```

- **Construction.** `Decompressor::new`, `Default`, `From<DecoderConfig>`
  and `Decompressor::builder(config).with_retention(..).with_backend(..).build()`
  are infallible (the backend defaults to the best detected level): every configuration value is validated where it is built
  (`DecodeLimits::with_max_window_log` returns `DecodeConfigError`).
  Nothing is allocated until the first decode.
- **`DecoderConfig`** is a `Copy` value with `DEFAULT`, `Default` and
  `From<DecodeLimits>` / `From<FrameMode>`.
  - `FrameMode::Concatenated` (default) decodes every frame until the input
    ends, as `ZSTD_decompress` does; at least one frame (Zstandard or
    skippable) is required.
  - `FrameMode::Single` decodes exactly one frame, Zstandard or
    skippable, and stops: sessions leave the rest unconsumed, one-shot calls
    report `TrailingData`. A skippable frame ends the operation with no
    output, as `ZSTD_decompressStream` reports a frame complete (returns 0)
    after either kind; a reader of single-frame members therefore sees a
    trailing skippable frame, or a stream of only skippable frames, as
    complete empty members, and a skippable frame before a Zstandard frame
    as its own member.
- **`DecodeLimits`**: the window limit (default `2^27`, the reference's
  `ZSTD_WINDOWLOG_LIMIT_DEFAULT`; `10..=31`) and optional input and output
  byte budgets over a whole operation.
- **One-shot.** `decompress` returns a new vector; `decompress_into`
  appends and returns the appended range, truncating back on error;
  `decompress_to_slice` fills a caller slice and reports `OutputTooSmall`
  with the slice completely filled. `decompress_to_uninit` does the same
  into `&mut [MaybeUninit<u8>]` and writes nothing past the result; see
  [uninitialized output](decoder-uninit-output.md).
- **Incremental.** `start()` borrows the decoder, `into_session()` owns it.
  Both run the same `core::Stream` state machine; see below. Each has
  `process_uninit`, the same call into `&mut [MaybeUninit<u8>]`, which may
  be mixed with `process` and writes exactly `output[..produced]`.
- **Retention.** `RetentionPolicy` (shared with the encoder) is applied
  after each one-shot operation, when a session's operation finishes or
  fails, when the next operation starts, and by
  `DecoderSessionOwned::{reinit, into_decompressor}`. `trim` applies a
  policy once; releasing drops every buffer but keeps the session state, so
  a finished session still reports `Finished`.
- **I/O.** `reader(R)` and `writer(W)` (module `decompressor::io`) own one
  8 KiB buffer each; a failed `DecoderWriter::finish` returns the writer in
  the crate-wide `io::FinishError`.
- **Dictionaries.** Every entry point has a `*_with_dictionary` twin taking
  `impl Into<DictionaryRef>`; see [Dictionaries](#dictionaries).
  `DecoderSession<'d>` keeps its single lifetime: `start_with_dictionary`
  borrows the dictionary for `'d` too. `DecoderSessionOwned<D>` owns any
  `D: AsRef<DecodeDictionary>` (an `Arc` shares one); `D` defaults to
  `DecodeDictionary`, so `DecoderSessionOwned` without parameters still
  names the plain owned session.

## Errors

```mermaid
graph LR
    Kind[InvalidDataKind] -->|From| Err[DecodeError]
    Entropy[bits / fse / huffman / literals / sequences / block] -->|Result&lt;_, InvalidDataKind&gt;| Frame
    Frame[frame / stream] -->|DecodeError| Session
    Session[session::process] -->|DecodeFailure: error + consumed + produced| Caller
    OneShot[decompress*] -->|DecodeError| Caller
    Err -->|From| Io[std::io::Error]
    Session -->|io adapters| Io
```

- Entropy-level code returns `InvalidDataKind`, a `#[non_exhaustive]`
  enum naming the malformed structure: `Magic`, `FrameHeader`,
  `SkippableFrame`, `BlockType`, `BlockSize`, `LiteralsSection`,
  `HuffmanTable`, `HuffmanStream`, `FseTable`, `SequencesSection`,
  `SequenceStream`, `LiteralsOverrun`, `Offset`.
- The frame and stream layers return `DecodeError` (`#[non_exhaustive]`,
  `thiserror`): `InvalidData { kind }` (with `kind` as its `source`),
  `UnexpectedEndOfInput`, `TrailingData`, `OutputTooSmall`,
  `ChecksumMismatch`, `ContentSizeMismatch`, `DictionaryUnsupported` (the
  frame names a dictionary and the operation has none),
  `DictionaryMismatch { expected, frame }` (the operation's dictionary
  declares another ID, or none), `WindowLimitExceeded`,
  `InputLimitExceeded`, `OutputLimitExceeded`, `AllocationFailed`,
  `InvalidState`.
- Building a `DecodeDictionary` returns `crate::dictionary::DictionaryError`:
  `NotZstdFormat` or `Corrupted`. The core's entropy parser reports an
  `InvalidDataKind`, which the dictionary layer maps to `Corrupted`.
- Sessions wrap errors in `DecodeFailure { error, consumed, produced }`,
  the exact progress of the failing call; `From<DecodeFailure>` recovers
  the error. Failures are terminal for the operation; the decoder is
  reusable.
- `From<DecodeError> for std::io::Error` maps format errors to
  `InvalidData`, truncation to `UnexpectedEof`, contract violations to
  `InvalidInput`, allocation failure to `OutOfMemory` and limits to `Other`.

## Streaming state machine

`core::stream::Stream` is a resumable parser over arbitrary input pieces.
Every header, and each block's whole content (at most 128 KiB), is
collected in a scratch buffer before it is acted on, so the parsers below it
never see partial data and never need to suspend.

```mermaid
stateDiagram-v2
    [*] --> Magic
    Magic --> FrameHeader: 0xFD2FB528
    Magic --> SkippableSize: 0x184D2A5?
    Magic --> Finished: Finish, no input left, >= 1 frame (Concatenated)
    Magic --> Failed: other magic
    SkippableSize --> Skipping: size <= u32::MAX - 8
    Skipping --> Magic: payload skipped (Concatenated)
    Skipping --> Done: payload skipped (Single)
    FrameHeader --> BlockHeader: header parsed and within limits
    BlockHeader --> BlockContent: type and size valid
    BlockContent --> BlockHeader: block regenerated, not last
    BlockContent --> Checksum: last block, checksum flag
    BlockContent --> FrameEnd: last block, no checksum
    Checksum --> FrameEnd: XXH64 low 32 bits match
    FrameEnd --> Magic: Concatenated
    FrameEnd --> Done: Single
    Done --> Finished: output delivered
    FrameHeader --> Failed
    BlockHeader --> Failed
    BlockContent --> Failed
    Checksum --> Failed
    Finished --> [*]
```

`FrameEnd` is not a stored state: `end_frame` counts the frame and picks
`Magic` or `Done` directly.

Every `process` call first delivers pending output from the history; it
returns `NeedsOutput` while any remains, so a new block is only decoded
once the previous one is fully delivered. Each state then either has
enough bytes to advance or returns `NeedsInput`.

### The incremental contract

```mermaid
stateDiagram-v2
    [*] --> Active: start / into_session / reinit
    Active --> Active: Process -> NeedsInput / NeedsOutput
    Active --> Finishing: first Finish latches total_in + input.len
    Finishing --> Finishing: Finish with exactly the unconsumed suffix
    Active --> Complete: Single frame done and delivered
    Finishing --> Complete: frames complete at end of input
    Active --> Failed: format, limit or check error
    Finishing --> Failed: truncation, error, or changed suffix / Process after Finish
    Complete --> Complete: zero progress, Finished
    Failed --> Failed: InvalidState
```

- `Process` with empty input only drains output; it never ends the input.
- In `Concatenated` mode a frame boundary is `NeedsInput` until `Finish`
  confirms the input ends there. `Finish` with the input exhausted inside
  a frame, or before any frame, is `UnexpectedEndOfInput`.
- The input budget slices each call's input to what remains of the
  budget; if the decoder then still needs input, the call fails with
  `InputLimitExceeded`. The output budget is checked after each block is
  regenerated, before any of it is delivered.

### One call

```mermaid
sequenceDiagram
    participant C as Caller
    participant S as Stream::process
    participant H as History
    participant F as FrameDecoder
    participant B as BlockDecoder
    C->>S: input, output, operation
    S->>S: check failed / finished / Finish contract, apply input budget
    loop until NeedsInput, NeedsOutput or Finished
        S->>H: deliver pending into output
        alt output still pending
            S-->>C: NeedsOutput
        end
        S->>S: fill scratch for the current state
        alt block content complete
            S->>H: prepare(window, block_max): compact if past window + slack, ensure room
            S->>F: decode_block(backend, header, scratch, buffer, start)
            F->>B: raw / RLE / literals + sequences (SIMD dispatch)
            B-->>F: end of the block's output
            S->>H: commit(end)
            F->>F: XXH64 update, content-size check
            S->>S: output budget check
        end
    end
    S-->>C: DecodeProgress or DecodeFailure (consumed, produced)
```

### History and window

`core::history::History` is one buffer that holds the frame's retained
history followed by undelivered output, kept initialized past its fill
mark with room for a block and the copies' 32-byte overlength. Blocks are
regenerated after the fill mark, and matches copy from the same buffer, so
history never wraps.
Before each block (all output delivered), if the buffer is longer than
`window + max(window, 128 KiB)` it drops everything but the last `window`
bytes. Compaction therefore moves each byte at most once on average, and
the buffer never exceeds `2 * window + 128 KiB` (window `>= 128 KiB`): the
retained window, a window of slack and one block.
A single-segment frame's window is its content size, so its history is
never compacted.

The one-shot driver (`Stream::walk`, behind `decode_all` and
`decode_to_slice`) parses straight from the input slice, with no scratch
copies, and regenerates into the caller's vector or slice, which serves as
history; each frame's history starts at its first byte in the output, so a
`decompress_into` prefix is never referenced. `decompress_to_slice`
decodes straight into `dst` when every frame declares a content size that
fits, and otherwise (or on any error) takes the incremental path, which
defines its results.

## Dictionaries

A dictionary is parsed once, when a `DecodeDictionary` is built
(`ZSTD_createDDict_advanced` and `ZSTD_loadEntropy_intoDDict`), and
borrowed by each operation; the decoder keeps no reference to it between
calls, so `Stream::process`, `decode_all` and `decode_to_slice` take it as
an `Option<DictView>` argument on every call.

```mermaid
flowchart TD
    Bytes[bytes, DictionaryContent] --> Empty{empty?}
    Empty -->|yes| RawParts[id 0, no tables]
    Empty -->|no| Layout{dictionary::layout}
    Layout -->|Raw, or Auto without magic or under 8 bytes| RawParts
    Layout -->|Zstd requested, no magic| E1[NotZstdFormat]
    Layout -->|magic + ID| Load["DecoderEntropy::load (ZSTD_loadDEntropy): Huffman tree, OF / ML / LL FSE tables, 3 repeat offsets"]
    Load -->|malformed table, log too large, rep 0 or beyond the content after it| E2[Corrupted]
    Load --> ZParts[id, DecoderEntropy with pair rows prebuilt]
    RawParts --> Parts[DecodeParts]
    ZParts --> Parts
    Parts --> Owned["DecodeDictionary: Box of bytes + DecodeParts"]
    Owned -->|DictionaryRef::view| View["DictView: content = all bytes, id, Option of entropy"]
```

- `parse_decode_parts(bytes, content)` (crate-private, in
  `dictionary::decode`) returns `DecodeParts { id, entropy }` without
  holding the bytes; `DecodeParts::view(bytes)` pairs them with whatever
  owns them. `DecodeDictionary` is one such owner; `PreparedDictionary`
  (with `compression`) is the other, and `DictionaryRef::Prepared` views
  its bytes the same way.
- As in the reference's `ZSTD_DDict`, **all** of a dictionary's bytes are
  history, the Zstandard format's magic number, ID and tables included;
  encoders only reference the content after the tables.
- The Huffman table's pair rows and its lockstep copy of the single rows
  are built when the dictionary is parsed (`HuffmanTable::build_shared`),
  so decoding reads the shared table without modifying it
  (`HuffmanTable::decode_streams_shared`).

### Frame start

```mermaid
sequenceDiagram
    participant S as Stream / walk
    participant F as FrameDecoder::begin
    participant B as BlockDecoder::reset
    participant D as FrameDecoder::decode_block
    participant Q as SequenceTables::execute
    S->>F: header, limits, Option of DictView
    alt frame names ID f, no dictionary
        F-->>S: DictionaryUnsupported
    else frame names ID f, dictionary declares another or none
        F-->>S: DictionaryMismatch
    end
    F->>B: Option of DecoderEntropy
    B->>B: literals tree = Dictionary or Missing; LL/OF/ML slots = Dictionary or Empty
    B->>B: repeat offsets = dictionary's or 1, 4, 8
    loop each compressed block
        S->>D: content, out, history range, same DictView
        D->>D: dictionary content reachable iff frame output so far < window
        D->>Q: BlockContext: history start, window, dictionary content, entropy
        Q->>Q: Repeat / treeless modes read the dictionary's tables until the frame replaces them
        Q->>Q: offsets past the frame's output take execute_exactly, which copies from the dictionary's tail
    end
```

- **IDs** follow `ZSTD_decodeFrameHeader`: a frame without a dictionary ID
  decodes with any dictionary or without one; a frame with ID `f` needs a
  dictionary declaring `f`. Raw content declares no ID.
- **Entropy** follows `ZSTD_copyDDictParameters`: a Zstandard-format
  dictionary sets the reference's `litEntropy` and `fseEntropy`, so the
  frame's first treeless literals block and its first repeat-mode sequence
  tables use the dictionary's tables (`TreeSource::Dictionary`,
  `SlotContent::Dictionary`), and its repeat offsets start at the
  dictionary's. A block that describes its own tables replaces them for
  the rest of the frame. Raw dictionaries leave the frame-start state of a
  frame without a dictionary.
- **History.** The dictionary's content is a separate segment before the
  frame's first byte (the reference's `extDict`): it is never copied into
  the output or the streaming history. A block may reach into it while the
  frame's output before the block is below the frame's window, and only
  when the executor's history starts at the frame's first byte; an offset
  may then reach `output + dictionary length` bytes back, whatever the
  window, as `oLitEnd - virtualStart` bounds it in the reference. The
  reference encoder references a dictionary only from blocks that end
  within one window of it (`ZSTD_checkDictValidity`), and the first window
  is never compacted out of the streaming history, so every frame it writes
  decodes. Past the first window, offsets are bounded by the window as
  without a dictionary.
- **Hot loop.** The fast path's offset test is unchanged; a sequence that
  fails it calls the cold `execute_exactly`, which is where the
  dictionary's reach and the two-segment copy live. Frames without a
  dictionary execute the same instructions as before (see
  [decoder fast paths](decoder-performance.md#dictionaries)).

## Block regeneration

```mermaid
flowchart TD
    Header[Block header: last, type, size] -->|reserved type| E1[BlockType]
    Header -->|size > min window, 128 KiB| E2[BlockSize]
    Header --> Raw[Raw: copy content]
    Header --> Rle[RLE: repeat one byte size times]
    Header --> Compressed
    Compressed --> Lit{Literals type}
    Lit -->|Raw| Borrow[borrow from block]
    Lit -->|RLE| Fill[fill literal buffer]
    Lit -->|Compressed| Tree[read Huffman tree: direct or FSE weights] --> Streams
    Lit -->|Treeless| Prev{previous tree?} -->|no| E3[LiteralsSection]
    Prev -->|yes| Streams[decode 1 stream or 4 with jump table; see Huffman stream ends]
    Borrow --> Seq
    Fill --> Seq
    Streams --> Seq[Sequences header: count, modes]
    Seq -->|count 0| Tail[append literals; no trailing bytes allowed]
    Seq --> Tables[LL, OF, ML tables: predefined / RLE / FSE / repeat]
    Tables --> Loop[decode offset, match and literal lengths; update states]
    Loop --> Exec[append literals; check offset; copy match]
    Exec --> Loop
    Exec --> End{bitstream exactly consumed?}
    End -->|no| E4[SequenceStream]
    End -->|yes| Rest[append remaining literals; save repeat offsets]
```

Invariants each layer relies on:

- **Bit readers.** Readers start after the end marker of the last byte (a
  zero last byte is rejected); sequence streams are valid only when
  consumed exactly, and Huffman streams whenever the reference accepts
  them (see [Huffman stream ends](#huffman-stream-ends)). `BitContainer` (Huffman and sequence streams) is refilled where
  the reference refills, and once reads pass the start its unread-bit count wraps
  above 64, so such streams are rejected; the Huffman weight decoder
  reads zeros past the start through the same container and checks
  `overflowed` (`BIT_DStream_overflow`) as the reference's FSE decoder does. Reads are
  at most 56 bits (the widest is a 31-bit offset).
- **FSE.** `read_distribution` accepts accuracy logs `5..=15` and requires
  the probabilities to sum exactly to the table size with no symbol above
  the alphabet; callers then cap the log (LL/ML 9, OF 8, Huffman weights 6).
  `FseTable::from_distribution` follows RFC 8878 section 4.1.1 (and
  matches Appendix A row for row); `SeqTable::fill_rows` spreads the same
  way and adds each symbol's baseline and extra-bit count from static
  alphabet rows. Basic sequence modes borrow complete predefined tables
  built at compile time. Custom tables retain their private allocations;
  repeat mode reuses the selected static, private or dictionary table.
  Every state a table produces indexes that table.
- **Huffman.** Weights above 12 or trees deeper than 12 bits are rejected;
  the implied last weight must complete a power of two and there must be an
  even number, at least two, of weight-1 symbols. The lookup table is
  indexed by the next `log` bits; codes are assigned by ascending weight,
  then symbol. Rows are replicated to a 12-bit lookup.
- **Literals.** Regenerated size at most the block maximum; four streams
  need at least six literals and a 6-byte jump table plus four non-empty
  streams; the Huffman table persists across blocks of one frame, with the
  decoder type (`TableKind`) the reference read it for.
- **Sequences.** Repeat mode needs an earlier table in the frame; an RLE
  symbol must be in its alphabet. Each sequence is checked before it is
  executed: literal length within the remaining literals, block output
  within the block maximum, and `0 < offset <= min(window, bytes of this
  frame produced so far)`. The repeat-offset rules (including
  `Repeated_Offset1 - 1` for a zero literal length) follow the reference
  exactly; a zero offset is `Offset`.
- **Headers.** `FrameHeader::parse` reads the fields at the offsets
  `header_size` fixed, through `dictionary_id_size`, `described_window_size`
  and `read_le`; `sequence_count` reads `Number_of_Sequences` before
  `read_header_on` updates the tables. These and `BlockHeader::parse` are
  proved against RFC 8878 in [formal verification](formal-verification.md),
  including that none of their indexing can panic.
- **Frames.** The reserved descriptor bit must be clear; a non-zero
  dictionary ID must match the operation's dictionary; the window must be
  within the limit (single-segment frames: the content size). The
  regenerated size
  may never exceed a declared content size and must equal it after the
  last block; the checksum is the low 32 bits of XXH64 with seed 0.

## Huffman stream ends

RFC 8878 calls a Huffman stream faulty unless decoding it consumes every
bit exactly. zstd 1.5.7 (`huf_decompress.c`) does not always check that,
and this decoder accepts exactly what it accepts:

- **Fast four-stream path.** `HUF_decompress4X{1,2}_usingDTable_internal_fast`
  runs for four streams when the tree is at most 11 bits deep (its table
  log is then `HUF_DECODER_FAST_TABLELOG`), every stream holds at least
  eight bytes, and the fourth segment is not empty. Its C loops read each
  stream as a continuation of the bytes before it, down to the jump
  table; each stream is then finished by `HUF_decodeStreamX{1,2}` over a
  reader whose start is the jump table, and only the regenerated length is
  checked. A stream may therefore stop short of its first bit, or read on
  into the previous stream, the jump table and, past them, the wrapped
  container of `BIT_lookBitsFast`. The one rejection after the loop
  (`HUF_initRemainingDStream`) is a stream whose loop position moved more
  than a word before its own start, which depends on how many rounds the
  loop ran: those round counts (`ip[0] - ilowest` over 7, and the outputs'
  room over 5 or 10) and the loop's exit when one position crosses
  another are ported as they are. The fast path also opens streams whose
  last byte is zero.
- **Pair decoding.** `HUF_decodeLastSymbolX2` caps the bits a stream's
  last pair lookup consumes at the container's end, so a stream whose last
  lookup reads past its bits still ends exactly. This applies on every
  path that decodes pairs, including the general four-stream body and a
  single stream.
- **Decoder type.** A `Compressed_Literals_Block` reads its tree for
  single symbols when it has one stream (`HUF_decompress1X1_DCtx_wksp`)
  and as `HUF_selectDecoder` chooses for four, from the literal count and
  the section size including the tree; a dictionary's tree is read for
  pairs (`ZSTD_loadDEntropy`). A `Treeless_Literals_Block` keeps the type
  of the table it reuses. `HuffmanTable` records it as a `TableKind`,
  which `literals` and `dictionary` set after each read.

```mermaid
flowchart TD
    Entry[HuffmanTable::decode_streams / decode_streams_shared] --> Exact[exact decoder: SIMD fast loops, exact end checks]
    Exact -->|Ok| Done[literals]
    Exact -->|HuffmanStream| Cold["decode_as_reference (cold): huffman::reference::decode"]
    Cold --> Streams{four streams?}
    Streams -->|no| One["HUF_decompress1X{1,2}: decode, BIT_endOfDStream"]
    Streams -->|yes| Init{"HUF_DecompressFastArgs_init"}
    Init -->|section under 10 bytes, or length4 past it| Reject[HuffmanStream]
    Init -->|"table log 12, a stream under 8 bytes, or op[3] at the end"| General["HUF_decompress4X{1,2} body: per-stream readers, BIT_endOfDStream"]
    Init -->|supported| Loop["fast C loop, X1 or X2 by TableKind"]
    Loop --> Remaining{"each stream: op within its segment, ip at most a word before its start?"}
    Remaining -->|no| Reject
    Remaining -->|yes| Tail["HUF_decodeStream{X1,X2} from the jump table, no end check"]
    Tail --> Done
    One -->|ended exactly| Done
    One -->|no| Reject
    General -->|every stream ended exactly| Done
    General -->|no| Reject
```

The exact decoder stays the only hot path. Whatever it accepts the
reference accepts with the same literals: an exactly consumed stream reads
only its own bits, which every reference path decodes to the same
symbols, and no reference check fails on it. Only a section it rejects
reaches `huffman::reference`, a scalar port that follows the reference's
control flow statement for statement over a model of `BIT_DStream_t`
(`BitStream`) and the fast loops' sentinel containers; its verdict and
literals are final. The port builds `HUF_DEltX2` rows on demand from the
single-symbol rows (a pair when both codes fit in the table log), which
is how `HUF_fillDTableX2` fills them, also for 12-bit trees the pair rows
of the hot path do not serve.

The reference takes these paths on 64-bit little-endian hosts and, on
x86-64 builds with dynamic BMI2 dispatch, only where the CPU has BMI2;
elsewhere it uses the general body and rejects what the fast path
accepts. This decoder follows the fast path on every host, so its verdicts
do not depend on the machine.

## Compatibility with the reference decoder

Valid frames decode identically. On malformed input the verdicts match the
reference's, Huffman literal streams that are not consumed exactly
included (see [Huffman stream ends](#huffman-stream-ends)), with these
deliberate exceptions:

- **Offsets beyond the window.** Offsets are limited to the window as well
  as to the frame's output, deterministically, in every API. The
  reference's one-shot decoder only checks the latter, and its streaming
  decoder depends on its buffer layout. Reference encoders never emit
  such offsets.
- **Block sizes.** Raw and RLE blocks larger than the block maximum and
  compressed blocks regenerating more than it are rejected in every API,
  as the reference's streaming decoder does; its one-shot decoder only
  bounds compressed block input.
- **Empty input** is `UnexpectedEndOfInput`; `ZSTD_decompress` returns an
  empty result.
- **Window limit.** The limit applies to one-shot calls too (the
  reference's one-shot API has none beyond `2^31`), and the default limit
  is exactly `2^27`, where the reference's streaming default is
  `2^27 + 1` bytes.
- **Huffman depth.** Trees of 12 bits are accepted, as by the reference,
  although RFC 8878 caps literal codes at 11 bits.
- **Dictionary reach.** A dictionary's content is reachable only from
  blocks that start within the frame's first window. The reference's
  one-shot decoder lets any offset reach it, and its streaming decoder until
  its ring buffer first wraps, which is never before the end of the first
  window. Reference encoders never reference a dictionary later.
- **Dictionary acceptance** matches `ZSTD_DCtx_loadDictionary_advanced`
  exactly: empty bytes are no dictionary, raw content is never rejected,
  and Zstandard-format bytes are rejected where `ZSTD_loadDEntropy` rejects
  them. `DictionaryContent::Zstd` on bytes without the magic number is
  `NotZstdFormat`, where the reference reports `dictionary_corrupted`.

## Magicless frames and header queries

`DecoderConfig::with_format(FrameFormat::Magicless)` (`ZSTD_d_format`) makes
`State::Magic` of the streaming machine, and the one-shot `walk`, skip the
magic number and the skippable-frame check: every frame starts at its header
descriptor, and concatenated frames follow header to header.
`decompressor::FrameHeaderInfo::parse(src, format)` ports
`ZSTD_getFrameHeader_advanced` over `FrameHeader::parse`; see
[OpenZL integration](openzl-integration.md#decoder-side).

```mermaid
stateDiagram-v2
    [*] --> Magic
    Magic --> FrameHeader: Magicless, or magic number read
    Magic --> SkippableSize: Standard, skippable magic
    SkippableSize --> Skipping
    Skipping --> Magic
    FrameHeader --> BlockHeader
```

## Known gaps

- **Dictionaries.** Only one dictionary per operation: there is no
  selection among several by frame ID (`ZSTD_d_refMultipleDDicts`), and
  `DecodeDictionary` always copies its bytes (no `ZSTD_dlm_byRef`). A
  `PreparedDictionary` decodes through `DictionaryRef::Prepared`, which
  views its own bytes with the `DecodeParts` it parsed when it was built.
- **Speed.** Text at levels 1-3 decodes about 1.1x slower than the
  reference; see [decoder fast paths](decoder-performance.md#known-gaps).
- **Memory.** The streaming history can reach twice the window plus
  128 KiB, where the reference keeps about one window plus two blocks; a
  ring buffer would close the gap. Allocation follows the output actually
  regenerated: `decompress`/`decompress_into` reserve a declared size up
  front only up to 32 bytes per unread input byte, and grow with the output
  beyond that ([output paths](decoder-performance.md#output-paths)).
- **Uninitialized output** has no `decompress_into` variant, and the
  one-shot path zeroes compressed blocks' output, 4 KiB at a time just
  ahead of the sequence loop, before decoding into it
  ([uninitialized output](decoder-uninit-output.md#known-gaps)).
  `decompress` zeroes only the room of sequences shorter than 1 KiB. There is
  no exact-size output contract, unlike mbrotli.
- **Legacy formats** (zstd v0.x) are not decoded.
