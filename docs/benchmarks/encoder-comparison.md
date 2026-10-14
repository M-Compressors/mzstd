# Compression comparison — 2026-10-13 (e6db371 + working tree)

[Benchmark index](README.md) · [Results by level](encoders/README.md)

The [384-case CSV](encoder-comparison.csv) and
[environment record](encoder-comparison-environment.json) describe mzstd at
`e6db371` (v0.2.0) plus the unreleased changes listed in the
[changelog](../../CHANGELOG.md), recorded as `enc-2026-10-13`, against zstd
1.5.7 (the vendored C library built by `zstd-ffi`). The run's
[size manifest](encoder-comparison-2026-10-13/sizes.csv) is archived with it;
the environment record holds the source hashes.

The encoder matrix ran in one process pinned to one core while the decoder
matrix ran on another physical core. See
[interpretation limits](#interpretation-limits) before reading absolute timings.

## Final measurements

![Median speed and output relative to C by level](encoders/charts/overview.svg)

Each level summarizes eight equally weighted datasets. Speed / C is C mean
latency divided by mzstd's mean latency for the same input; output / C is
mzstd bytes divided by C bytes. The summary takes the median of those eight
ratios, including empty and tiny input. These are not median Criterion sample
times. Higher speed and lower output are better.

| Level | mzstd median speed / C ↑ | mzstd median output / C ↓ |
| ---: | ---: | ---: |
| -5 | 0.928× | 1.000× |
| -1 | 0.930× | 1.000× |
| 1 | 0.950× | 1.000× |
| 2 | 0.931× | 1.000× |
| 3 | 0.946× | 1.000× |
| 4 | 0.932× | 1.000× |
| 5 | 0.926× | 1.000× |
| 6 | 0.952× | 1.000× |
| 7 | 0.958× | 1.000× |
| 8 | 0.958× | 1.000× |
| 9 | 0.921× | 1.000× |
| 10 | 0.922× | 1.000× |
| 11 | 0.989× | 1.000× |
| 12 | 1.000× | 1.000× |
| 13 | 0.931× | 1.000× |
| 14 | 0.956× | 1.000× |
| 15 | 0.966× | 1.000× |
| 16 | 0.982× | 1.000× |
| 17 | 0.971× | 1.000× |
| 18 | 0.992× | 1.000× |
| 19 | 0.995× | 1.000× |
| 20 | 1.006× | 1.000× |
| 21 | 0.995× | 1.000× |
| 22 | 0.994× | 1.000× |

The median over all 192 dataset/level pairs is **0.975×**. mzstd has a lower
mean latency than C in 70 of 192 pairs; in 59 of them its 95% mean timing
interval lies wholly below C's. These counts compare point estimates, not
statistical significance. By dataset, the medians across levels are: empty
2.29×, tiny text 1.49×, repeated bytes 1.04×, structured binary 0.98×, Alice
0.91×, random 64 KiB 0.91×, cyclic Alice 0.90× and random 1 MiB 0.87×.

For Alice at level 1, mzstd measures 311.8 MiB/s against C's 400.5 MiB/s,
both producing 59,657 bytes. At level 3, 237.2 against 282.3 MiB/s (56,970
bytes); at level 9, 48.3 against 55.5 MiB/s (52,188 bytes); at level 19, 5.16
against 5.17 MiB/s (49,211 bytes).

Every mzstd frame in this run is byte-identical to zstd's: the preflight
compares the frames themselves, not only their lengths, before timing
starts. The [level pages](encoders/README.md) retain all datasets and exact
timings with 95% mean confidence bounds.

Cycle profiles of Alice put the remaining difference in the match finders
of levels -5 to 12 (fast, dfast and the row-based lazy search): entropy
coding, histograms, Huffman and sequence encoding run within a few percent of
C at every level. Levels 11-22 (binary trees and the optimal parser) are at
0.87-1.02× on text.

## Measurement contract

| Setting | Recorded value |
| --- | --- |
| Host | Intel Core i7-13700KF, x86_64, WSL2 |
| Affinity | One process pinned to logical CPU 2; the decoder matrix ran concurrently on logical CPU 6 |
| Rust | rustc 1.98.1; Cargo release defaults; runtime SIMD dispatch (AVX2 here) |
| C compiler | GCC 11.4.0; `cc` crate defaults (`-O3`) |
| zstd | 1.5.7; submodule `f8745da6ff1ad1e7bab384bd1f9d742439278e99`; single-threaded, no legacy formats, no Huffman assembly |
| Compression | Level only, complete known input, no dictionary or explicit flush |
| API | Cold one-shot: `ZSTD_createCCtx` + `ZSTD_compress2` into a `ZSTD_compressBound` buffer against `Compressor::new(level).compress`; construction, allocation, compression and disposal timed |
| Sampling | 20 samples; 1 s warmup; 3 s requested measurement per case |
| Validation | All 192 mzstd frames byte-identical to zstd's and decoded to the original input through zstd before timing |

Inputs are empty, 44-byte text, `testdata/alice29.txt` (152,089 bytes), cyclic
Alice text at 1 MiB, structured binary at 64 KiB, deterministic pseudorandom
bytes at 64 KiB and 1 MiB, and 1 MiB of repeated `a` bytes: mbrotli's corpus,
with the same generators and seeds. Corpus construction and validation occur
outside timing. Empty input has latency and output size but no meaningful
throughput or compressed/input fraction.

## Reproduction

Run from the repository root; use a fresh baseline name when repeating
timings.

```sh
cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench encoders --locked -- --test
taskset -c 2 cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench encoders --locked -- --save-baseline enc-2026-10-13
python3 benchmarks/comparison/report.py --baseline enc-2026-10-13 --csv docs/benchmarks/encoder-comparison.csv
python3 benchmarks/comparison/level_docs.py --csv docs/benchmarks/encoder-comparison.csv --environment docs/benchmarks/encoder-comparison-environment.json --report docs/benchmarks/encoder-comparison.md --output docs/benchmarks/encoders
python3 benchmarks/comparison/plot.py --csv docs/benchmarks/encoder-comparison.csv --output docs/benchmarks/encoders/charts --subtitle 'i7-13700KF; e6db371 + working tree; 20 samples; 1 core; 2026-10-13'
```

Plotting uses Matplotlib 3.10.8. Criterion extends slow cases to collect the
requested samples. The exporter rejects incomplete matrices and mismatched
input lengths. Export immediately after timing and archive `sizes.csv` with
the named baseline: a later preflight replaces that shared manifest. This
run's [size manifest](encoder-comparison-2026-10-13/sizes.csv) is checked in.
Raw samples remain local under `benchmarks/comparison/target/criterion/`.
Chart regeneration reuses the CSV; it does not rerun timings.

## Interpretation limits

This is a single sweep on one WSL2 host, one benchmark process pinned to one
core. Both implementations of a case ran in the same process, one after the
other, so ratios to C are the comparable quantity. Affinity restricts CPU
migration but does not control frequency, thermal state, or host scheduling;
the concurrent decoder sweep shared the L3 cache. Implementation order is
fixed. The lowest mean does not necessarily imply a statistically significant
lead; confidence bounds do not capture every source of host or allocator
variation. Ratios to C moved by up to ±3% between repeated runs of the same
binaries on this host.

Cold APIs include allocation. glibc raises its mmap threshold after the first
large free, so the first iterations of a process and later ones can pay
different page-fault and zeroing costs; both implementations alternate within
every case and see the same allocator state.

The suite measures cold one-shot compression. Streaming, retained contexts
and parallel compression have separate root benchmarks (`cargo bench --bench
compress`, `--bench parallel`) and are not represented by these results. See
the [benchmark guide](../benchmarking.md) and
[harness specification](../../architecture/benchmarking.md).
