# Compression comparison — 2026-10-14 (cd91929 + working tree)

[Benchmark index](README.md) · [Results by level](encoders/README.md)

The [384-row CSV](encoder-comparison.csv) records the working tree on
`cd91929828c289ad9da7703554761ce4af1fcb99` against zstd 1.5.7, under the
Criterion baseline `enc-20261014-fix`. The median speed relative to C is
**1.092×** over all 192 dataset/level pairs, and mzstd has the lower mean
latency in **158** of them. Text at the fastest levels remains slower:
Alice at levels -5 to 1 measures 0.87–0.88×.

The [environment record](encoder-comparison-environment.json) identifies the
measured source files and binary by SHA-256. The
[size manifest](encoder-comparison-2026-10-14-fix/sizes.csv) and
[raw Criterion samples and estimates](encoder-comparison-2026-10-14-fix/criterion-samples.json.gz)
are archived with this run. The [changelog](../../CHANGELOG.md) describes the
encoder changes.

## Results

![Median speed and output relative to C by level](encoders/charts/overview.svg)

Speed / C is C mean latency divided by mzstd mean latency for the same input.
Output / C is mzstd bytes divided by C bytes. Each level takes the median of
these ratios over eight equally weighted datasets, including empty and tiny
input. These are medians across datasets, not median Criterion sample times.
Higher speed and lower output are better.

| Level | mzstd median speed / C ↑ | mzstd median output / C ↓ |
| ---: | ---: | ---: |
| -5 | 1.078× | 1.000× |
| -1 | 1.129× | 1.000× |
| 1 | 1.164× | 1.000× |
| 2 | 1.046× | 1.000× |
| 3 | 1.033× | 1.000× |
| 4 | 0.973× | 1.000× |
| 5 | 1.067× | 1.000× |
| 6 | 1.143× | 1.000× |
| 7 | 1.106× | 1.000× |
| 8 | 1.107× | 1.000× |
| 9 | 1.078× | 1.000× |
| 10 | 1.078× | 1.000× |
| 11 | 1.076× | 1.000× |
| 12 | 1.042× | 1.000× |
| 13 | 1.080× | 1.000× |
| 14 | 1.046× | 1.000× |
| 15 | 1.042× | 1.000× |
| 16 | 1.124× | 1.000× |
| 17 | 1.092× | 1.000× |
| 18 | 1.099× | 1.000× |
| 19 | 1.096× | 1.000× |
| 20 | 1.096× | 1.000× |
| 21 | 1.111× | 1.000× |
| 22 | 1.100× | 1.000× |

The unrounded overall median is **1.092093×**. mzstd has a lower mean latency
in **158 of 192** pairs; in 136 of those, its individual 95% confidence interval
for mean latency lies wholly below C's. These intervals have no adjustment for
multiple comparisons.

The medians over all 24 levels for each dataset show the workload differences:

| Dataset | mzstd median speed / C ↑ |
| --- | ---: |
| empty | 2.431× |
| tiny-text | 1.582× |
| alice29 | 1.000× |
| text-1m | 1.009× |
| binary-64k | 1.101× |
| random-64k | 1.046× |
| random-1m | 1.045× |
| repeated-1m | 1.149× |

Text results at selected levels:

| Dataset | Level | mzstd, MiB/s ↑ | C, MiB/s ↑ | Speed / C ↑ | Frame bytes, both |
| --- | ---: | ---: | ---: | ---: | ---: |
| alice29 | 1 | 352.90 | 404.19 | 0.873× | 59,657 |
| alice29 | 3 | 271.39 | 296.00 | 0.917× | 56,970 |
| alice29 | 9 | 53.83 | 55.28 | 0.974× | 52,188 |
| alice29 | 19 | 5.61 | 5.44 | 1.031× | 49,211 |
| text-1m | 1 | 2561.35 | 2479.87 | 1.033× | 61,206 |
| text-1m | 3 | 1683.43 | 1723.27 | 0.977× | 55,878 |
| text-1m | 9 | 401.42 | 428.25 | 0.937× | 52,381 |
| text-1m | 19 | 37.64 | 34.33 | 1.096× | 49,285 |

Every mzstd frame passed byte-for-byte comparison with zstd and decoded back
to the original input through zstd before timing. The
[level pages](encoders/README.md) expose all datasets, timings, throughput and
95% mean confidence bounds.

## Regression checks

A first sweep of this tree (`enc-20261014-speed`, run beside the decoder
sweep) was compared case by case with the previous publication. The 16 cases
more than 3% slower than before were rerun with `paired.py`: one process per
case, randomized AB/BA order, ten pairs, the previous tree's driver against
this tree's and C against this tree's.

| Cases | Previous tree → this tree | C → this tree | Verdict |
| --- | ---: | ---: | --- |
| repeated-1m l13-15 | 0.82-0.84× | 0.96-1.01× | real: btlazy2's inline tree count walked whole-block runs a word at a time; fixed (runs past four words go to the vector scan), then 0.99× and 1.17-1.21× |
| random-64k l-5, l-1, l1 | 0.92-0.95× | 1.00-1.03× | real: the cost of fast's branch-free candidate check on data where the branch predicts well; kept for its gain on text |
| binary-64k l3, l4, l10; random-1m l2; repeated-1m l1, l3, l6, l8, l18, l19 | 0.97-1.14× | 1.00-1.28× | noise |

This sweep measured the tree after the fix, without a concurrent decoder
sweep.

## Measurement contract

| Setting | Recorded value |
| --- | --- |
| Host | Intel Core i7-13700KF, x86_64, WSL2 |
| Affinity | One single-threaded process pinned to logical CPU 2; no concurrent decoder sweep |
| Rust | rustc 1.99.0; Cargo release defaults; runtime SIMD dispatch (AVX2 here) |
| C compiler | GCC 11.4.0; `cc` crate defaults (`-O3`) |
| zstd | 1.5.7; submodule `f8745da6ff1ad1e7bab384bd1f9d742439278e99`; single-threaded, no legacy formats, no Huffman assembly |
| Compression | Level only, complete known input, no dictionary or explicit flush |
| API | Cold one-shot: `ZSTD_createCCtx` + `ZSTD_compress2` into a `ZSTD_compressBound` buffer against `Compressor::new(level).compress`; construction, allocation, compression and disposal timed |
| Sampling | 20 samples; 0.2 s warmup; 0.5 s requested measurement per case; Criterion's default 100,000 bootstrap resamples; 95% confidence intervals |
| Validation | All 192 mzstd frames byte-identical to zstd and decoded to the original input through zstd before timing |

Inputs are empty, 44-byte text, `testdata/alice29.txt` (152,089 bytes), cyclic
Alice text at 1 MiB, structured binary at 64 KiB, deterministic pseudorandom
bytes at 64 KiB and 1 MiB, and 1 MiB of repeated `a` bytes: mbrotli's corpus,
with the same generators and seeds. Construction and validation occur outside
timing. Empty input has latency and output size but no meaningful throughput
or compressed/input fraction.

## Reproduction

Run from the repository root and choose a fresh baseline name for new timings.
These options reproduce the recorded sampling settings:

```sh
cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench encoders --locked -- --test
taskset -c 2 cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench encoders --locked -- --save-baseline my-encoders --warm-up-time 0.2 --measurement-time 0.5 --sample-size 20 --nresamples 2000
python3 benchmarks/comparison/report.py --baseline my-encoders --csv /tmp/my-encoders.csv
cp benchmarks/comparison/target/criterion/sizes.csv /tmp/my-encoders-sizes.csv
```

Criterion extends slow cases to collect the requested samples. The exporter
rejects incomplete matrices and mismatched input lengths. Export immediately
after timing and archive `sizes.csv` with the named baseline: a later preflight
replaces that shared manifest. The archived gzip file contains each case's
`benchmark`, `estimates` and `sample` JSON records from Criterion.

Use the [benchmark guide](../benchmarking.md#encoder-comparison) to regenerate
the checked-in pages and charts from the published CSV and environment record.
Plotting uses Matplotlib 3.10.8 and does not rerun timings.

## Interpretation limits

This is one complete sweep on one WSL2 host. C and mzstd run consecutively
for each case in a fixed order. Affinity restricts CPU migration but does not
control frequency, thermal state or host scheduling. Confidence bounds
do not capture every source of host or allocator variation.

Every dataset/level pair has equal weight, regardless of input size or execution
time. The median describes this corpus, not a production workload distribution.
Cold APIs include allocation and allocator state; timings include the effects
of glibc's changing mmap and trim thresholds within a process.

The previous published encoder sweep (`parity-final-20261014`, median
1.000×) measured the tree before these changes with the same sampling. The
change between the two runs is consistent with paired, process-per-case
measurements of the same binaries, but the two sweeps are not a controlled
before/after experiment.

Streaming, retained contexts and parallel compression have separate root
benchmarks (`cargo bench --bench compress`, `--bench parallel`) and are outside
this comparison. See the [harness specification](../../architecture/benchmarking.md).
