# Compression comparison — 2026-10-15 (mzstd 0.2.5)

[Benchmark index](README.md) · [Results by level](encoders/README.md)

The [384-row CSV](encoder-comparison.csv) records mzstd 0.2.5 (the working
tree on `39d1ae4` that became this release) against zstd 1.5.7, under the
Criterion baseline `enc-20261015c`. The median speed relative to C is
**1.099×** over all 192 dataset/level pairs, and mzstd has the lower mean
latency in **157** of them. The fastest levels gained the most: the level
medians at -5 and -1 rose from 1.08× and 1.13× to 1.21×. Alice text at
levels -5 to 1 measures 0.89–0.94×, cyclic Alice at 1 MiB 0.97–1.06×.

The [environment record](encoder-comparison-environment.json) identifies the
measured source files and binary by SHA-256. The
[size manifest](encoder-comparison-2026-10-15/sizes.csv) and
[raw Criterion samples and estimates](encoder-comparison-2026-10-15/criterion-samples.json.gz)
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
| -5 | 1.213× | 1.000× |
| -1 | 1.210× | 1.000× |
| 1 | 1.131× | 1.000× |
| 2 | 1.090× | 1.000× |
| 3 | 1.037× | 1.000× |
| 4 | 0.996× | 1.000× |
| 5 | 1.072× | 1.000× |
| 6 | 1.087× | 1.000× |
| 7 | 1.086× | 1.000× |
| 8 | 1.095× | 1.000× |
| 9 | 1.104× | 1.000× |
| 10 | 1.124× | 1.000× |
| 11 | 1.058× | 1.000× |
| 12 | 1.062× | 1.000× |
| 13 | 1.074× | 1.000× |
| 14 | 1.037× | 1.000× |
| 15 | 1.014× | 1.000× |
| 16 | 1.090× | 1.000× |
| 17 | 1.093× | 1.000× |
| 18 | 1.106× | 1.000× |
| 19 | 1.114× | 1.000× |
| 20 | 1.123× | 1.000× |
| 21 | 1.132× | 1.000× |
| 22 | 1.104× | 1.000× |

The unrounded overall median is **1.098681×**. mzstd has a lower mean latency
in **157 of 192** pairs; in 145 of those, its individual 95% confidence interval
for mean latency lies wholly below C's. These intervals have no adjustment for
multiple comparisons.

The medians over all 24 levels for each dataset show the workload differences:

| Dataset | mzstd median speed / C ↑ |
| --- | ---: |
| empty | 2.437× |
| tiny-text | 1.609× |
| alice29 | 0.965× |
| text-1m | 1.015× |
| binary-64k | 1.105× |
| random-64k | 1.053× |
| random-1m | 1.061× |
| repeated-1m | 1.133× |

Text results at selected levels:

| Dataset | Level | mzstd, MiB/s ↑ | C, MiB/s ↑ | Speed / C ↑ | Frame bytes, both |
| --- | ---: | ---: | ---: | ---: | ---: |
| alice29 | 1 | 358.88 | 402.82 | 0.891× | 59,657 |
| alice29 | 3 | 283.78 | 298.26 | 0.951× | 56,970 |
| alice29 | 9 | 54.29 | 57.56 | 0.943× | 52,188 |
| alice29 | 19 | 5.83 | 5.39 | 1.081× | 49,211 |
| text-1m | 1 | 2661.05 | 2510.61 | 1.060× | 61,206 |
| text-1m | 3 | 1722.21 | 1756.34 | 0.981× | 55,878 |
| text-1m | 9 | 395.20 | 396.17 | 0.998× | 52,381 |
| text-1m | 19 | 37.37 | 33.74 | 1.107× | 49,285 |

Every mzstd frame passed byte-for-byte comparison with zstd and decoded back
to the original input through zstd before timing. The
[level pages](encoders/README.md) expose all datasets, timings, throughput and
95% mean confidence bounds.

## Regression checks

Every case was compared with the previous publication (`enc-20261014-fix`).
Cases whose speed relative to C fell by more than 5% were rerun as separate
processes, the previous release's profile driver against this release's,
nine interleaved pairs each on an idle CPU 4:

| Cases | Sweep change vs previous run | Paired time, this release / previous | Verdict |
| --- | ---: | ---: | --- |
| alice29 l12, l14 | -5.3%, -5.6% | +0.4%, -0.9% | noise |
| text-1m l10, l16 | -7.8%, -8.6% | -0.0%, +0.5% | noise |
| repeated-1m l-1 | -15.2% | -0.2% | noise |
| empty l12 | -12.0% | +1.5% | noise |
| random-64k l3 | -7.5% | +0.3% | noise |

The same paired runs time Alice at level 1 2.0% faster than the previous
release (nine of nine pairs), matching its sweep change of +2.1%.

The first sweep of this release's tree measured cold 64 KiB inputs 25-31%
slower at levels -5 to 1: a sequence store kept initialized at full
capacity paid for that initialization on every cold call. It was removed
before release, and the sweep above measured the released tree.

## Measurement contract

| Setting | Recorded value |
| --- | --- |
| Host | Intel Core i7-13700KF, x86_64, WSL2 |
| Affinity | One single-threaded process pinned to logical CPU 2; the decoder sweep ran at the same time on logical CPU 6 |
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
