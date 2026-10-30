# Benchmarking

mzstd has two benchmark layers. The library's own Criterion benches
(`benches/`) time warm contexts and streaming against zstd 1.5.7 for
development work. The implementation comparison (`benchmarks/comparison/`)
times both libraries' cold one-shot APIs on a fixed corpus and produces the
published reports in [`docs/benchmarks/`](../docs/benchmarks/README.md).
This page describes the comparison harness; [running it](../docs/benchmarking.md)
is described with the commands.

## Framed container benches

`benches/framed_compress.rs` and `benches/framed_decompress.rs` need
`--features framed`. They compare [framed containers](framing.md) with the
closest thing zstd ships, the contrib seekable format compiled into
`zstd-ffi`: identical inputs (`zstd_ffi::corpus` text, binary and mixed
data, a 16 KiB text, 512 records), identical levels and 64 KiB frames.
Before timing, every container must decode with `FramedDecompressor`,
`ZSTD_decompress` and `ZSTD_seekable_decompress`, and every reference output
with the seekable reader; both sizes are printed.

```mermaid
flowchart LR
    In["corpus input"] --> M["FramedCompressor::compress_into, reused owner"]
    In --> C["ZSTD_seekable_compressStream, maxFrameSize 64 KiB"]
    M --> V{"decodes with mzstd, ZSTD_decompress, contrib seekable"}
    C --> V
    V --> T[Criterion timing, throughput in input bytes]
    Ct["container (level 3, 64 KiB chunks)"] --> D1["FramedDecompressor::decompress_into"]
    Ct --> D2["FramedSeekReader resource read"]
    Ct --> D3["ZSTD_decompressDCtx"]
    Ct --> D4["ZSTD_seekable_initBuff + decompress"]
```

The records case stores each record as its own resource and compares with
one `ZSTD_compress2` frame per record; the difference in size is the
container's per-resource structure (header frame, directory record and seek
table entries).

## Module map

```mermaid
graph TD
    subgraph comparison["benchmarks/comparison (own workspace, excluded from the root)"]
        Lib["lib.rs: run_encoders, run_decoders, run_profile"] --> Core["core (private): corpora, cold encoders, validation"]
        Core --> Dec["core::decoder (private): cold decoders"]
        Core --> Profile["core::profile (private): one validated case"]
        BEnc["benches/encoders.rs"] --> Lib
        BDec["benches/decoders.rs"] --> Lib
        BProfile["bin/profile.rs"] --> Lib
        BWarm["bin/warm.rs: reused contexts, corpus or file:PATH"] --> Mzstd
        Paired["paired.py: randomized AB/BA process pairs"] --> BProfile
        Paired --> BWarm
        Silesia["silesia.py: C / main / working tree rounds on whole files, median chart"] --> BWarm
        Report["report.py: Criterion run to CSV"] --> Docs["level_docs.py: compression level pages and dataset charts"]
        Report --> Plot["plot.py: overview, throughput and size charts"]
        Report --> DecDocs["decoder_docs.py: decompression level pages and charts"]
        Docs --> Plot
        DecDocs --> Docs
    end
    Core --> Mzstd["mzstd (path dependency)"]
    Core --> Ffi["zstd-ffi: zstd 1.5.7 C library"]
    Core --> Alice["testdata/alice29.txt"]
    Docs --> Out["docs/benchmarks/{encoders,decoders}/"]
    Plot --> Out
    DecDocs --> Out
    Silesia --> SOut["docs/benchmarks/silesia*"]
```

The package has its own `[workspace]` and lockfile, and the root workspace
excludes it, so Criterion output and the comparison's dependency resolution
never touch the library's own checks. Its `core` module is private and
exposes the encoder and decoder benchmark entry points plus the focused
profiling driver. The driver validates one case before entering an uninlined
collection boundary, so Callgrind can exclude corpus setup and preflight.
`paired.py` runs saved driver binaries with randomized AB/BA process ordering
and records every pair for analysis; it does not generate the published
Criterion matrix. See the [profiling commands](../docs/benchmarking.md#profiling).
The `warm` driver times one case with a context and destination reused
across iterations, after checking its frame or output against C; it also
reads any file as `file:PATH`. `silesia.py` drives two saved `warm`
binaries over whole Silesia files, the working tree's (C and mzstd) and the
main branch's (mzstd), in five rounds of seeded random order per cell.

## Corpus and matrix

The eight datasets are mbrotli's comparison corpus, with the same
generators and seeds, so the two projects' results describe one corpus:
`empty`, `tiny-text` (44 bytes), `alice29` (152,089 bytes), `text-1m` (Alice
repeated to 1 MiB), `binary-64k` (`((i / 16) ^ i) as u8`), `random-64k`
and `random-1m` (xorshift64), and `repeated-1m` (`a` bytes). Each dataset is
timed at 24 levels: -5, -1 and 1 through 22. That is 384 encoder and 384
decoder cases per run (two implementations each).

## Data flow

```mermaid
sequenceDiagram
    participant B as bench binary
    participant C as core::run / decoder::run
    participant Z as zstd 1.5.7
    participant M as mzstd
    participant Cr as Criterion
    participant R as report.py
    participant L as level_docs.py / plot.py / decoder_docs.py
    B->>C: run_encoders / run_decoders
    C->>Z: preflight: every dataset x level
    C->>M: same input, same level
    C->>C: frames identical (encoders) / outputs equal (decoders), else Err
    C->>C: write target/criterion*/sizes.csv
    C->>Cr: time c-zstd and mzstd per group
    Cr-->>R: <group>/<impl>/<baseline>/estimates.json
    R->>R: join with sizes.csv, reject incomplete or duplicate matrices
    R-->>L: CSV with means, 95% bounds, MiB/s, speed / C
    L-->>L: README, l<level>.md pages, summary and per-dataset SVGs (encoders)
    L-->>L: README, l<level>.md pages, one SVG per level (decoders)
    Note over R,L: optional --main-csv: the main branch's matrix from its own run
```

- **Preflight before timing.** The encoder run compresses every case with
  both libraries first and fails unless the frames are byte-identical and
  decode to the input through the C decoder. The decoder run compresses with
  C and requires both decoders to restore the input. Filtered runs preflight
  the whole matrix too, so a partial sweep never succeeds over a broken case.
- **Cold APIs.** Each iteration includes construction, allocation, the call
  and disposal: `ZSTD_createCCtx` + `ZSTD_CCtx_setParameter(level)` +
  `ZSTD_compress2` into a `ZSTD_compressBound` buffer against
  `Compressor::new(level).compress`; `ZSTD_getFrameContentSize` +
  `ZSTD_decompress` into an exactly sized buffer against
  `Decompressor::default().decompress`. Buffers on the C side are
  uninitialized `Vec` capacity, which C fills; mzstd's API returns its own
  vector.
- **Sampling defaults.** 20 samples, 1 s warm-up and at least 3 s of measurement
  per case; Criterion command-line options override them. Each published
  environment record stores the settings actually used for that run.

## Report generation

```mermaid
flowchart TD
    Run[Criterion baseline] --> Collect{report.py: every case measured once, sizes match, bounds ordered?}
    Collect -->|no| Fail[exit with the reason]
    Collect -->|yes| Csv["encoder-comparison.csv / decoder-comparison.csv"]
    Csv --> Validate{level_docs.validate_rows: complete matrix, consistent lengths and frame sizes?}
    Validate -->|no| Fail
    Validate -->|encoders| Enc["level_docs.py: README + l-5.md ... l22.md, l<level>-summary.svg, l<level>-<dataset>.svg"]
    Enc --> Plot["plot.py: overview.svg, throughput.svg, size.svg"]
    Validate -->|decoders| Dec["decoder_docs.py: README + l-5.md ... l22.md, l<level>.svg, overview.svg"]
    Main["--main-csv: main branch's CSV, validated the same way"] -.-> Enc
    Main -.-> Plot
    Main -.-> Dec
```

The layout mirrors mbrotli's benchmark documentation. Per level and
dataset, speed / C = C mean latency / mzstd mean latency and output / C =
mzstd bytes / C bytes; level summaries take the median of the eight
per-dataset ratios, and the index pages order levels by mzstd's median
speed / C. With `--main-csv`, a third series, mzstd main, joins every
median, overview and summary chart and table: the main branch's harness
timed in its own process beside the working tree's, each normalized to the
C timings of its own run, so drift between the two processes cancels as
far as C sees it. Per-dataset charts keep the working tree's run only.
Each compression level page has a summary chart, one chart per
dataset (throughput or latency with 95% bounds, beside the frame size) and
the exact numbers; each decompression level page has one panel per dataset.
The hand-written run reports (`encoder-comparison.md`,
`decoder-comparison.md`) record the measurement contract, reproduction
commands and interpretation limits; the environment records hash the
measured sources and name the archived size manifests. SVGs are generated
with Matplotlib 3.10.8 with fixed hash salts and no timestamps, so
regenerating from the same CSV yields the same files.

## Invariants

- Both sides of a case see identical bytes and the same level; the encoder
  comparison only runs when the frames are identical, so every speed ratio
  is read at an identical compression ratio.
- A published CSV always holds the complete 384-case matrix of one named
  Criterion baseline; `report.py` and `level_docs.py` refuse anything else.
- Python unit tests (`python3 -m unittest` in `benchmarks/comparison`)
  cover validation, ordering, page text and the exporter without Matplotlib
  (`test_level_docs.py`, `test_decoder_docs.py`, `test_report.py`,
  `test_silesia.py`); `test_plot.py` renders every chart kind when
  Matplotlib is installed.

## Known gaps

- One host (AMD Ryzen 9 7950X, two dedicated KVM vCPUs); frequency and
  scheduling are not controlled beyond pinning each process to one vCPU.
- The main branch's series comes from a separate process run beside the
  working tree's, not from interleaved pairs; only the Silesia sweep
  interleaves the three sides.
- The C library is built by `zstd-ffi` with `cc` defaults (`-O3`), without
  its x86-64 Huffman assembly (`ZSTD_DISABLE_ASM`) and single-threaded.
- Reused contexts are published only for Silesia; streaming performance is
  measured only by the root `benches/`.
