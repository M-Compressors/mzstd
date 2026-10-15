# Decoder fast paths

How `decompressor::core` regenerates blocks at speed: SIMD dispatch, the
bit readers, the Huffman and sequence loops, the copies, and the output
paths. Every fast path decodes exactly the bits the reference decoder
(zstd 1.5.7) reads, in the same order, so it accepts and rejects the same
streams as the straightforward port it replaced; only refill timing and
copy granularity differ. The crate stays `forbid(unsafe_code)`: bounds are
checked, and the code is shaped so the compiler can drop most of the
checks (see [index masks](#index-masks)).

## Dispatch

```mermaid
sequenceDiagram
    participant D as Decompressor (Backend)
    participant S as Stream
    participant F as FrameDecoder
    participant B as BlockDecoder::decode
    participant H as HuffmanTable::decode_streams
    participant Q as SequenceTables::execute
    D->>S: Stream::new(config, backend)
    S->>F: decode_block(backend, ...)
    F->>B: decode(backend, ...)
    alt raw / RLE block
        B->>B: copy_from_slice / fill
    else compressed block
        B->>B: MatchCopy from backend.has_fast_shuffle()
        B->>B: fearless_simd::dispatch!(backend.0, simd => ...)
        B->>H: #[simd] kernel: literals
        B->>Q: #[simd] kernel: sequences
    end
```

`Backend` (public, `src/backend.rs`) wraps a `fearless_simd::Level`
detected once when the decoder is built (`DecompressorBuilder::with_backend`
overrides it). `BlockDecoder::decode` dispatches once per compressed
block. The two hot loops are separate `#[fearless_simd_macros::simd]`
functions: each is compiled per level with that level's target features
(on AVX2 that includes BMI2, so bit reads use `shrx`/`bzhi`), and keeping
them apart keeps each loop's register allocation independent.
`MatchCopy::Shuffle` is chosen on levels with a native byte shuffle
(SSSE3 and up, NEON, WASM SIMD); `sse2` and `fallback` use
`MatchCopy::Scalar`.

## Bit readers

```mermaid
classDiagram
    class BitContainer {
        container: u64
        consumed: u32
        pos: usize
        +new(src) Option
        +look(n) / look_nonzero(n) / read(n) / skip(n)
        +reload_fast(src)
        +reload(src) bool
        +is_exhausted() bool
    }
    class Sentinel {
        bits: u64
        pos: usize
        +from_container(BitContainer, src, start)
        +into_container(src, start) BitContainer
        +reload(src)
        +peek(shift) / skip(n)
    }
    class ReverseBitReader {
        reads zeros past the start
        +overflowed() bool
    }
    BitContainer <--> Sentinel : Huffman fast loops
```

- **`BitContainer`** is `BIT_DStream_t`: the eight stream bytes at `pos`,
  `consumed` bits read from the top. `look` is `BIT_getMiddleBits`.
  `reload` is `BIT_reloadDStream`: a full refill while `pos >= 8`, the
  cautious refill near the start (`#[cold]`, by value so the container
  stays in registers), and no change once more than 64 bits were consumed.
  While `pos >= 8` every refill so far was full and callers read at most
  57 bits between refills, so the overflow test only runs on the cautious
  path. A stream is valid only when `pos == 0 && consumed == 64`; once
  reads pass the start, `consumed` stays above 64, so corrupt streams are
  rejected exactly as by `BIT_endOfDStream`.
- **`Sentinel`** is the reference's fast-loop stream
  (`HUF_DecompressFastArgs`): unread bits at the top, a `1` below them
  shifted up with every read, so the consumed count is the sentinel's
  position (`trailing_zeros`) and a 12-bit lookup is one shift. Positions
  index the buffer holding all four streams. `reload(src, last)` clamps the
  position to `last`, the start of the buffer's last whole word: the fast
  loops never pass it, so the clamp changes nothing, but it replaces the
  bounds-checked load and its fallback branch with a `cmov`.
- **`ReverseBitReader`** keeps the old per-read semantics for Huffman
  weight streams, whose overflow test mirrors the reference's FSE decoder.

## Huffman literals

```mermaid
flowchart TD
    Read[HuffmanTable::read: weights, rows at 12 bits] --> Kind{four streams?}
    Kind -->|no| Tail1[decode_tail: HUF_decodeStreamX1]
    Kind -->|yes| Open[open four BitContainers]
    Open --> Fast{every pos >= 8?}
    Fast -->|no| Tails
    Fast -->|yes| Sel{prefers_pairs: HUF_selectDecoder}
    Sel -->|X1| Single[single_rounds: 5 symbols per stream per round, 4 if 12-bit tree]
    Sel -->|X2| Pairs[build_pairs if stale; pair_rounds: 4 lookups per stream per round]
    Single --> Back[Sentinels back to containers]
    Pairs --> Back
    Back --> Tails[decode_tail per stream: exact end check]
```

- Rows are built for 12-bit lookups whatever the tree depth (each code's
  rows repeated `1 << (12 - log)` times), so lookups need no mask.
- The fast loops count the rounds every stream can afford up front (each
  refill moves back at most seven bytes and must stay seven bytes into its
  stream; outputs need room for the round's stores), run them without
  per-stream tests, then recount.
- Pair rows (`HUF_DEltX2`) are derived from the single-symbol rows: a row
  pairs the code at the top of its index with the next one when both fit
  in 12 bits. A round gathers each stream's symbols in one word and stores
  eight bytes; bytes past the symbols stay inside the stream's own segment.
  They are rebuilt only after a new tree.
- A `PairRow` packs symbols, bit count and shift in one `u32`, so a row of
  one symbol plus the row that continues it is the row of both. A code of
  `b` bits fills a run of `1 << (12 - b)` rows whose `j`th row continues
  with the code at row `j << b`, the same series for every code of that
  length (the reference's `HUF_fillDTableX2Level2` per rank). The first run
  of each length is built from the single rows; later runs of that length
  copy it with one add per row that swaps in their own first symbol. This
  cut the build from 1.97M to 0.58M instructions on Alice at level 1, and a
  test checks every row of trees of every depth against the definition.
- Every stream finishes with the single-symbol table in the reference's
  cautious loop and must be consumed exactly; the pair decoder never
  decides how a stream ends.
- The loops reach a table through `TableAccess`: the decoder's own table
  (`&mut HuffmanTable`, `decode_streams`) builds its pair rows on first
  use, a dictionary's shared table (`&HuffmanTable`,
  `decode_streams_shared`) only uses rows built when the dictionary was
  parsed. Both are monomorphized, so the own-table path compiles as before.

## Sequences

```mermaid
flowchart TD
    Header[read_header: TableSlot::update per symbol type] --> Init[initial states, reload]
    Init --> Next{sequences left?}
    Next -->|yes| Dec[SequenceDecoder::decode: ZSTD_decodeSequence]
    Dec -->|zero repeat offset| EOff[Offset]
    Dec --> Exe[Executor::execute: ZSTD_execSequence]
    Exe -->|literals overrun| ELit[LiteralsOverrun]
    Exe -->|match_end + 32 <= out.len| Wild[offset check; wildcopy literals; wildcopy match]
    Exe -->|else| Exact[execute_exactly, cold: BlockSize / Offset / OutputFull; exact copies]
    Wild --> Next
    Exact --> Next
    Next -->|no| End{bitstream exhausted?}
    End -->|no| ESeq[SequenceStream]
    End -->|yes| Last[copy remaining literals; save repeat offsets]
```

- `SeqTable` rows are `ZSTD_seqSymbol`: baseline, next-state base,
  extra-bit and state-bit counts. Both counts are a `BitCount`, a
  32-variant `#[repr(u8)]` enum: as an enum its range reaches the compiler
  with every load, so reads by these counts need no `& 63` before the
  `bzhi` or shift (about four instructions per sequence). The loop holds references to rows and
  reads fields as needed. Slots are allocated on first use and rebuilt in
  place; the predefined tables are kept while no other table replaces
  them.
- Decoding refills once per sequence and once more when the sequence's
  extra bits exceed `64 - 7 - (9 + 9 + 8)`, as the reference does.
- Errors keep the old order: literal overrun, block size, offset (and
  output room for caller-sized buffers).

### Copies

`copy.rs` holds the wildcopies (`ZSTD_wildcopy`, `ZSTD_overlapCopy8`):

| Case | Copy |
| --- | --- |
| literals | one 16-byte chunk, then 32-byte chunks |
| match, offset >= 32 | 32-byte chunks; the first is copied from the output split at the literals' end, whose history's last `offset` bytes provably hold it, so it needs no bounds check |
| match, 16 <= offset < 32 | 16-byte chunks |
| match, offset < 16, `Shuffle` | `PERIOD[offset]` spreads the period over a `u8x16`; `ROTATE[offset]` turns each chunk's vector into the next's |
| match, offset < 16, `Scalar` | the reference's offset spreading (`dec32table`/`dec64table`), then 8-byte chunks |
| end of buffer | `execute_exactly`: exact literal copy, `exact_match` doubling copies |

Fast copies write up to `WILDCOPY_OVERLENGTH` (32) bytes past a sequence
and read as far past its source; the executor uses them only when the
output has that room, and literals always carry 32 readable bytes after
their end (`Literals`): raw literals are borrowed in place when the block
holds 32 more bytes after them, and copied otherwise.

### Index masks

Release-mode `usize` additions may wrap, so `out[i..i + 16]` normally
costs an overflow test and a bounds test. The sequence executor cuts the
output to start at `max(history_start, op - window)` — no valid match
reaches further back — so every index is below `2^31 + 2^19`
(compile-time asserted against `DecodeLimits::MAX_WINDOW_LOG`), and
`copy::idx` masks operands to 32 bits. The masked sums provably do not
wrap, which lets the compiler relate the executor's room tests to the
copies' bounds checks and drop many of them. The masks never change a
valid index.

### Dictionaries

A dictionary adds nothing to the per-sequence fast path. Its content is
not copied into the output: `Executor` carries it as a slice that only the
cold `execute_exactly` reads. The fast path's test is the one it always
had, `offset > lit_end || offset > window`; a sequence failing it, which
without a dictionary is always an error, now calls `execute_exactly`,
which reports the error or, with a dictionary, copies the match's head
from the dictionary's tail and the rest from the output. The dictionary's
tables are picked once per block (`TableSlot::get`), and its repeat
offsets once per frame. The index bounds above are unchanged: the cut
output still starts at the history's first byte or a window before the
block.

Measured on a loaded i7-13700KF (AVX2, pinned), `cargo bench --bench
decompress -- 'decompress-(dictionary|small)'`: a thousand JSON-like
records of about 120 bytes, each its own level-3 frame with a 16 KiB
dictionary trained on similar records, decode in 198 µs against zstd
1.5.7's 135 µs (1.47x), and the same records compressed without a
dictionary in 1.60 ms against 1.11 ms (1.44x). The dictionary adds no gap
of its own beyond the decoder's per-frame overhead on tiny frames. Frames
without a dictionary decode as before: on a quiet host, text level 1
2.12 → 2.06 ms, text level 3 1.85 → 1.80 ms, binary level 3 3.49 →
3.43 ms, random 19.7 → 19.1 µs, streaming mixed 2.46 → 2.44 ms (Criterion
medians, before and after the change).

## Output paths

```mermaid
flowchart LR
    ToSlice[decompress_to_slice] --> Direct{decode_to_slice: every frame declares a size that fits?}
    Direct -->|yes| Slice[SliceOutput: frame cut at its declared end]
    Direct -->|no, or any error| Session[incremental session over History]
    Into[decompress / decompress_into] --> Cut[decode_all: VecOutput, declared frames cut at their end]
    Cut -->|any error| Uncut[reset, rerun with VecOutput uncut: defines the error]
    Process[DecoderSession::process] --> History[History: prepare / commit]
```

`decompress` and `decompress_into` write through `VecOutput`, which owns
three decisions per frame and block:

```mermaid
flowchart TD
    Begin[begin_frame: content size, unread input] --> Declared{declared size?}
    Declared -->|yes| Reserve["reserve min(declared end, filled + 32 x unread input)"]
    Declared -->|no| Grow
    Reserve --> Block
    Grow[no reservation] --> Block
    Block[next block] --> Kind{raw or RLE?}
    Kind -->|yes| Place["place: extend_from_slice / resize with the byte, one pass"]
    Kind -->|compressed| Slack["reserve: grow_output to filled + block_max + 32, cut at the declared end"]
    Place --> Account[FrameDecoder::account_block: checksum, size checks]
    Slack --> Decode[FrameDecoder::decode_block] --> Account
```

- `Stream::walk` is the one-shot frame grammar over an `Output`.
  `SliceOutput` cuts the caller's slice at each frame's declared end, so
  chunked copies never touch bytes past the output and the rest of `dst`
  stays unchanged.
- `VecOutput` cuts a declared frame's blocks at its end, like
  `SliceOutput`, so no slack past the output is zeroed or kept. A frame that
  overruns its declared size then fails with `OutputTooSmall` instead of the
  reference's `ContentSizeMismatch`, so `decode_all` resets the stream and
  reruns the input uncut on any error; the uncut pass defines every error and
  leaves the vector as it was.
- Capacity grows by doubling (`grow_output`/`reserve_output`), never past
  twice the requested length. When the frame's last request is within that
  factor, capacity jumps straight to it, so a declared frame ends with no
  reallocation and no spare capacity. At a frame start, a declared size is
  reserved only up to 32 output bytes per unread input byte
  (`PLAUSIBLE_RATIO`): an allocation always needs input in proportion, and
  a frame that claims more grows with its actual output.
- Raw and RLE blocks are appended in one pass (`Output::place`):
  `extend_from_slice` or `resize` with the run byte write straight into spare
  capacity. Compressed blocks still need initialized room, which costs one
  zeroing pass over the output; safe Rust cannot hand the decoder
  uninitialized memory.
- `decompress_to_slice` tries the direct path and, when a frame has no
  content size, does not fit, or anything fails, reruns the whole input
  through the incremental session, which defines the method's results and
  errors.
- `History` keeps an initialized buffer with a fill mark; `prepare`
  compacts as before and ensures room for a block plus the overlength,
  `commit` records the block's end.
- The literal buffer only grows, so blocks of varying sizes zero nothing
  but its new tail.

## Measurements

`cargo bench --bench decompress -- --warm-up-time 1 --measurement-time 3`,
pinned to one core of an i7-13700KF (AVX2 backend). Frames are written by
zstd 1.5.7 from `zstd_ffi::corpus` inputs and decoded with reused contexts,
`decompress_to_slice` against `ZSTD_decompressDCtx`. Times are Criterion
medians; "before" is the scalar port this replaced.

| Case | before | now | zstd 1.5.7 | ratio |
| --- | --- | --- | --- | --- |
| text 4 MiB, level 1 | 5.96 ms | 2.14 ms | 1.81 ms | 1.18 |
| text 4 MiB, level 3 | 4.47 ms | 1.86 ms | 1.64 ms | 1.13 |
| text 4 MiB, level 19 | 2.70 ms | 1.10 ms | 1.05 ms | 1.05 |
| binary 4 MiB, level 1 | 18.89 ms | 2.13 ms | 2.28 ms | 0.93 |
| binary 4 MiB, level 3 | 14.37 ms | 3.52 ms | 3.28 ms | 1.07 |
| binary 4 MiB, level 19 | 15.98 ms | 4.73 ms | 4.34 ms | 1.09 |
| random 1 MiB, levels 1-19 | 64.5 µs | 19.8-21.3 µs | 19.6-21.4 µs | 0.99-1.04 |
| text+binary+text 12 MiB, LDM | 18.02 ms | 5.06 ms | 4.79 ms | 1.06 |
| mixed 4 MiB, streaming | 7.26 ms | 2.48 ms | — | — |

Both decoders run near five instructions per cycle on these inputs, so
instruction counts (callgrind) track time closely; the remaining gap is in
the sequence loop (about 140 instructions per sequence against the
reference's 125, mostly bounds checks and bit-count masks) and the pair
Huffman loop.

XXH64 checksums are not vectorized: each of its four lanes is a serial
multiply-rotate-multiply chain, and AVX2 has no 64-bit vector multiply. A
`u64x4` kernel measured 11.8 GB/s against 20.9 GB/s for the scalar code,
which already matches the reference's 21 GB/s.

## Known gaps

- Measured and rejected for `decompress` (2026-10-14): appending sequences
  to the vector with `extend_from_slice`/`extend_from_within`, which needs
  no zeroed output, is slower (Alice level 1: 110 µs against 96 µs) because
  every chunk reloads and stores the vector's length through memory, which
  serializes the copies; decoding a batch of sequences before executing it,
  in two out-of-line loops, executes about 15% more instructions; one
  allocation for the three sequence tables changes nothing.
- Measured and rejected (2026-10-15, `silesia.tar` levels 1, 3 and 19):
  copying matches with offsets of 32 or more in 16-byte chunks, as
  `ZSTD_wildcopy` does, instead of 32-byte ones (no faster; the hot store
  after the match load is a cache miss on the match source, which the
  reference pays too); gathering each stream's symbols of a single-symbol
  Huffman round in one word instead of an array (8% more samples in
  `decode_streams`). Filling Huffman rows by weight in fixed-size runs, as
  `HUF_readDTableX1_wksp` does, and spreading FSE tables without "less than
  one" symbols without per-cell checks, as `ZSTD_buildFSETable_body` does,
  cut table building by about 30% of its samples on `silesia.tar` level 3,
  but the whole decode did not get measurably faster, and the cold
  one-shot comparison decoded text 2.5-4% slower (Alice level 3, cyclic
  Alice levels 3 and 19; 11 interleaved process pairs) with an unchanged
  `SequenceTables::execute` source executing 1.6% more instructions: the
  changed modules moved its inlining and layout.

- Compressible text decodes at 0.81-0.96x the reference's speed through
  `decompress` ([benchmarks](../docs/benchmarks/decoders/README.md)): the
  sequence loop executes about 1.05x the reference's instructions on Alice
  (133 against 126 per sequence; both spill about 40 values per sequence to
  the stack), the four-stream pair Huffman loop about 1.4x per lookup, and
  compressed blocks need their output zeroed before decoding, about
  8-10 µs per MiB that the reference, writing into uninitialized memory,
  does not pay.
- `decompress_to_slice` takes the incremental path, with its extra copies,
  for frames without a content size.
- No prefetching variant for long offsets (`ZSTD_decompressSequencesLong`)
  and no split literal buffer.
