# Benchmarks and profiling

The independent [implementation comparison](../benchmarks/comparison/Cargo.toml)
measures `mzstd` against zstd 1.5.7, the C library built from the vendored
submodule by `zstd-ffi`. Browse the [benchmark results index](benchmarks/README.md)
for one page per compression level, with medians across all eight datasets,
vertical speed/size bars, and complete timings. The strongest mzstd comparisons
appear first; the pages explain the ranking and retain every dataset.
The index also links the recorded run reports, raw data, and limitations.

Both codecs use the same documentation layout under `docs/benchmarks/`:

| Artifact | Encoder | Decoder |
| --- | --- | --- |
| Report | `encoder-comparison.md` | `decoder-comparison.md` |
| Measurements | `encoder-comparison.csv` | `decoder-comparison.csv` |
| Environment | `encoder-comparison-environment.json` | `decoder-comparison-environment.json` |
| Size manifest | `encoder-comparison-<date>/sizes.csv` | `decoder-comparison-<date>/sizes.csv` |
| Main branch's run | `encoder-comparison-<date>/main.csv` | `decoder-comparison-<date>/main.csv` |
| Index and level pages | `encoders/README.md`, `encoders/l-5.md`–`l22.md` | `decoders/README.md`, `decoders/l-5.md`–`l22.md` |
| Charts | `encoders/charts/` | `decoders/charts/` |

Each chart directory contains `overview.svg`. Install plotting support in an
isolated environment if needed:

```sh
python3 -m venv /tmp/zstd-plots
/tmp/zstd-plots/bin/python -m pip install matplotlib==3.10.8
```

Use that environment's Python for the generation commands below.

## Encoder comparison

Run from the repository root:

```sh
git submodule update --init --recursive
cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench encoders --locked -- --test
cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench encoders --locked -- --save-baseline my-comparison
python3 benchmarks/comparison/report.py --baseline my-comparison --csv /tmp/my-comparison.csv
```

Use a fresh baseline name for each complete run. The standalone package owns its
lockfile and writes to `benchmarks/comparison/target/criterion/`. Its own
workspace keeps the comparison's dependency resolution and Criterion results
apart from the root benchmarks and `target/criterion/`.

The suite contains **384 cases**: eight corpora at levels -5, -1 and 1–22 for
both implementations. Every case compresses a complete known input with only
the level set, no dictionary and no explicit flush.

| Implementation | Version | Timed API |
| --- | --- | --- |
| zstd C | 1.5.7, pinned submodule | `ZSTD_createCCtx`, `ZSTD_CCtx_setParameter(level)`, `ZSTD_compress2` into a `ZSTD_compressBound` buffer, `ZSTD_freeCCtx` |
| mzstd | Local checkout | Construct `Compressor` for the level, then `compress` |

This is **cold end-to-end compression**: configuration, construction, scratch and
destination allocation, encoding, and disposal occur inside each iteration.
No implementation retains workspace. Before timing starts, every mzstd frame must
be **byte-identical** to zstd's and decode to its input through the C decoder;
`sizes.csv` records the frame lengths.

The corpora are empty input, a 44-byte text, `testdata/alice29.txt`, Alice
repeated to 1 MiB, structured 64 KiB binary, deterministic pseudorandom bytes at
64 KiB and 1 MiB, and 1 MiB of repeated `a` bytes — mbrotli's comparison corpus.
Construction and validation of all 384 frames finish before timing starts.

Defaults are 20 samples, one second of warmup, and three seconds of measurement
per case. Criterion options override them. A focused timing run is:

```sh
cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench encoders --locked -- 'alice29/l3/'
```

Filtered runs still validate the full matrix. The exporter rejects incomplete
timing sweeps and checks input lengths. Export immediately after the matching run:
`sizes.csv` describes the most recent preflight. Archive that file with the named
baseline for later analysis. CSV columns include mean nanoseconds and 95%
confidence bounds, MiB/s, `C time / mzstd time`, compressed bytes, and
`compressed / input`. For empty input, throughput is zero and the compressed
fraction is undefined; compare latency.

Rebuild the checked-in encoder pages and charts with Matplotlib 3.10.8:

```sh
python3 benchmarks/comparison/level_docs.py \
  --csv docs/benchmarks/encoder-comparison.csv \
  --main-csv docs/benchmarks/encoder-comparison-2026-10-30/main.csv \
  --environment docs/benchmarks/encoder-comparison-environment.json \
  --report docs/benchmarks/encoder-comparison.md \
  --output docs/benchmarks/encoders
python3 benchmarks/comparison/plot.py \
  --csv docs/benchmarks/encoder-comparison.csv \
  --main-csv docs/benchmarks/encoder-comparison-2026-10-30/main.csv \
  --output docs/benchmarks/encoders/charts \
  --subtitle 'Ryzen 9 7950X (KVM); 20 samples; one vCPU per process; 2026-10-30'
```

`level_docs.py` writes the index, 24 level pages and their detailed charts.
`plot.py` writes `overview.svg`, `throughput.svg` and `size.svg` into the same
charts directory. All eight corpora contribute to the medians. `--main-csv`
adds the main branch's series (see [main branch series](#main-branch-series)).
The published encoder and decoder CSVs are the Criterion baseline
`full-20261030` (2026-10-30), with their size manifests, the main branch's
CSVs and the raw Criterion samples of both trees in
`benchmarks/encoder-comparison-2026-10-30/` and
`benchmarks/decoder-comparison-2026-10-30/`. They use 20 samples, 0.5 s
warmup and 1 s requested measurement; see the
[encoder](benchmarks/encoder-comparison.md) and
[decoder](benchmarks/decoder-comparison.md) reports. Each
environment record identifies the measured sources; regenerating charts reuses
those measurements and does not rerun the benchmark. When publishing a fresh
run, update the CSV, archived manifest and environment record together before
regenerating pages and charts.

## Decoder comparison

The [decoder results](benchmarks/decoders/README.md) compare zstd C and mzstd on
**identical frames**, written by zstd C at levels -5, -1 and 1–22 from the same
eight inputs as the encoder comparison. All **384 cases** must restore the
original bytes through both decoders before timing starts.

```sh
cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench decoders --locked -- --test
cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench decoders --locked -- --save-baseline my-decoders
python3 benchmarks/comparison/report.py --decoding --baseline my-decoders --csv /tmp/my-decoders.csv
```

Results live in `benchmarks/comparison/target/criterion-decoders/`, independent
of encoder and root API measurements. Archive its `sizes.csv` with each fresh
named baseline. Filtering, for example `'alice29/l3/'`, still validates the
complete matrix; a full sweep is required for export. Sampling defaults match
the encoder comparison and accept the same Criterion overrides.

| Decoder | Version | Timed API |
| --- | --- | --- |
| zstd C | 1.5.7, pinned submodule | `ZSTD_getFrameContentSize`, then `ZSTD_decompress` into an uninitialized buffer of exactly that size |
| mzstd | Local checkout | Construct `Decompressor`, then `decompress` |

This measures cold native APIs, including construction, allocation, decode and
disposal. C reads the output capacity from the frame header; mzstd's
`decompress` returns its own vector. No decoder retains state between
iterations. Throughput counts **restored bytes**, not compressed input bytes.
In the CSV, `input_bytes` is the original corpus length and `compressed_bytes`
is the shared frame size. Level is the **source encoder's** setting. Empty input
has latency but no meaningful throughput or compression fraction.

Rebuild the checked-in overview, 24 level pages and 25 SVG charts with
Matplotlib 3.10.8:

```sh
python3 benchmarks/comparison/decoder_docs.py \
  --csv docs/benchmarks/decoder-comparison.csv \
  --main-csv docs/benchmarks/decoder-comparison-2026-10-30/main.csv \
  --environment docs/benchmarks/decoder-comparison-environment.json \
  --report docs/benchmarks/decoder-comparison.md \
  --output docs/benchmarks/decoders
```

The overview takes the median of eight per-dataset C-time/decoder-time ratios,
with equal weight. Empty and tiny inputs have equal weight in the median and
use latency panels. Per-level panels retain all individual mean confidence
bounds. Shared frame sizes are listed alongside restored lengths instead of
ranking decoders by size. See the [run report](benchmarks/decoder-comparison.md)
for environment and limitations.

The generators are covered by unit tests that need no Matplotlib:

```sh
cd benchmarks/comparison && python3 -m unittest
```

## Main branch series

To compare the working tree with the main branch, export main into a
separate directory (`git archive main`, plus the vendored zstd submodule),
build its harness with its own `CARGO_TARGET_DIR`, and run each suite of
both trees side by side, one process per vCPU, under the same baseline name:

```sh
taskset -c 0 "$current_encoders" --bench --save-baseline full-20261030 --sample-size 20 --warm-up-time 0.5 --measurement-time 1 --noplot &
taskset -c 1 "$main_encoders" --bench --save-baseline full-20261030 --sample-size 20 --warm-up-time 0.5 --measurement-time 1 --noplot &
wait
python3 benchmarks/comparison/report.py --root "$main_tree/benchmarks/comparison/target/criterion" --baseline full-20261030 --csv /tmp/main-encoders.csv
```

Each tree's harness writes Criterion output under its own manifest
directory. The generators normalize each tree's mzstd to the C timings of
its own process, so the two series are comparable as ratios to C, not as
absolute times. Repeat with the decoder binaries and `--decoding`.

Allocation-heavy cold cases (levels 13 and above, the 1 MiB inputs, empty
input) move by up to 40% between processes, so the published change from
main comes from paired process runs of the two trees' `profile` drivers
over the whole matrix:

```sh
python3 benchmarks/comparison/paired.py /tmp/profile-main /tmp/profile-current \
  --csv docs/benchmarks/encoder-comparison.csv --output /tmp/paired-encoders.json \
  --codec encode --cpu 0 --pairs 5 --seconds .1
```

## Silesia

`silesia.py` times whole Silesia files with reused contexts. It takes two
saved `warm` binaries: the working tree's, which times C and mzstd, and the
main branch's. Every cell runs five rounds of the three sides in a seeded
random order, each process at about 0.25 s of iterations calibrated on C,
and checks its bytes against C first:

```sh
python3 benchmarks/comparison/silesia.py run --current /tmp/warm-current --main /tmp/warm-main \
  --corpus /path/to/silesia --output /tmp/silesia-encode.json --cpu 0 \
  --plan encode-1,encode1,encode3,encode5,encode7,encode9,encode12
python3 benchmarks/comparison/silesia.py report --results docs/benchmarks/silesia-2026-10-30/silesia.json.gz \
  --output docs/benchmarks/silesia-2026-10-30/silesia.svg
```

Interrupted runs resume from the output file. `report` reads plain or
gzipped JSON.

## Sharded runs

A complete sweep on one core takes about 45 minutes for each suite. On a host
with idle cores, run the two suites, or corpus shards of them, at once, one
process per physical core. Every implementation of a case still runs in the
same process, so ratios between implementations stay comparable; absolute
timings drop by a few percent because the shards share the last-level cache,
memory bandwidth and turbo budget. Record the lanes in the environment file,
and keep builds and other work off those cores.

Build the harness once, then run its executables directly so that concurrent
`cargo bench` invocations do not wait on the build directory lock. The harness
writes to fixed directories under `benchmarks/comparison/target/`, separate
benchmark IDs land in separate directories, and every shard writes the same
deterministic `sizes.csv` during its preflight:

```sh
cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench encoders --bench decoders --locked --no-run
cd benchmarks/comparison
enc=$(ls -t target/release/deps/encoders-* | grep -v '\.d$' | head -1)
dec=$(ls -t target/release/deps/decoders-* | grep -v '\.d$' | head -1)
taskset -c 2 "$enc" --bench --save-baseline my-enc &
taskset -c 6 "$dec" --bench --save-baseline my-dec &
wait
cd ../..
python3 benchmarks/comparison/report.py --baseline my-enc --csv /tmp/my-enc.csv
python3 benchmarks/comparison/report.py --decoding --baseline my-dec --csv /tmp/my-dec.csv
```

The published runs used exactly these two lanes.

## Development benchmarks

`cargo bench --bench compress`, `--bench decompress` and `--bench parallel` at
the repository root time warm contexts, streaming and parallel compression
against the same C library, after checking identical output.

## Profiling

The comparison package also supplies a focused driver with the same datasets
and cold APIs. Arguments are `encode|decode`, `c-zstd|mzstd`, corpus, level,
and a positive iteration count. It validates the selected case before the
loop; use Criterion for reported timings and this driver for diagnosis or
paired process runs.

```sh
CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release --manifest-path benchmarks/comparison/Cargo.toml --bin profile --locked
taskset -c 2 benchmarks/comparison/target/release/profile encode mzstd repeated-1m 1 10000
valgrind --tool=callgrind --collect-atstart=no --toggle-collect='*profile_encode*' \
  benchmarks/comparison/target/release/profile encode mzstd repeated-1m 1 30
```

For decoding, use `decode` and `--toggle-collect='*profile_decode*'`.
The uninlined collection boundaries exclude corpus construction and preflight.

Save each build's `profile` executable before rebuilding, then collect paired
runs with a fixed core and randomized AB/BA ordering:

```sh
python3 benchmarks/comparison/paired.py /tmp/profile-before /tmp/profile-after \
  --csv /tmp/my-comparison.csv --output /tmp/paired.json \
  --cpu 2 --pairs 10 --seconds 0.15 --filter '^(text-1m|repeated-1m)/l(1|3|9)$'
```

Both processes use the same iteration count, calibrated from the CSV. The JSON
retains every pair; it is not a significance claim. Use a new output path for
each run. To compare C with Rust, pass the same executable twice and add
`--baseline-implementation c-zstd`. Keep builds and other CPU-heavy work out of
timed runs. Include tiny, incompressible, warm and streaming cases when checking
for regressions, and report intervals and noisy cases alongside improvements.

- **Stages.** `cargo build --release --features hotpath` turns on the
  `hotpath::measure` anchors at the codec's stage boundaries; a binary whose
  `main` carries `#[hotpath::main]` prints the timing table on exit.
- **Sampling.** `samply record --save-only --rate 20000 -o profile.json.gz --
  <binary>` samples a release build (needs `perf_event_paranoid <= 1`). The
  leaf addresses map onto `objdump -d` or `addr2line` of the binary for
  instruction and source attribution. Samples locate hotspots; they are not
  exact per-instruction cycle counts. Record the event and sampling settings
  when comparing C and Rust profiles of the same workload.
- **Instructions and cache.** `valgrind --tool=callgrind` (optionally with
  `--dump-instr=yes` or `--cache-sim=yes`) on a build with
  `CARGO_PROFILE_RELEASE_DEBUG=line-tables-only` gives exact instruction
  counts per function and source line; `callgrind_annotate` reads them.
- **Instruction counts per stage.** On x86_64 Linux, `hotpath-callgrind`
  adds per-anchor instruction counts from Callgrind. The `hotpath benchmark`
  workflow runs in merged mode on every pull request: one build runs natively
  for timing and allocations, then under Callgrind, which merges its
  instruction counts into the native report and uploads it to hotpath Cloud,
  judged by `hotpath/policy.toml`. Valgrind distorts allocation data as well
  as timing, so both come from the native run. Counts and allocations are
  judged, timing is shown but not judged; compare counts only within one
  Valgrind version, Rust toolchain and runner image:

  ```sh
  cargo build --release --locked --example profile_compressor --features hotpath-alloc,hotpath-callgrind
  HOTPATH_OUTPUT_FORMAT=json HOTPATH_OUTPUT_PATH=native.json \
    target/release/examples/profile_compressor testdata/alice29.txt
  HOTPATH_OUTPUT_FORMAT=json HOTPATH_MERGE_REPORT=native.json HOTPATH_OUTPUT_PATH=merged.json \
    valgrind -q --tool=callgrind --instr-atstart=no \
    target/release/examples/profile_compressor testdata/alice29.txt
  ```
- **Code.** `cargo asm --lib --release '<symbol>'` shows what a kernel compiled
  to; the SIMD-dispatched kernels appear as `vectorize_avx2::<…::{closure}>`
  symbols.

Measure allocation strategies in a fresh process: glibc raises its mmap
threshold after the first large free, so the first cold call and later ones pay
different page-fault and zeroing costs.
