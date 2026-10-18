# Compression comparison — 2026-10-18 (mzstd 0.2.7)

[Benchmark index](README.md) · [Results by level](encoders/README.md)

The [384-row CSV](encoder-comparison.csv) records mzstd 0.2.7 (the working
tree on `eb10fd0` that becomes this release) against zstd 1.5.7, under the
Criterion baseline `enc-20261018`. The median speed relative to C is
**1.112×** over all 192 dataset/level pairs, and mzstd has the lower mean
latency in **166** of them. The same sweep of the tree before this
release's changes (`enc-before`, same host, settings and day) measured
1.086× and 159 pairs. The fastest levels stay the strongest (1.24× at
levels -5 and -1); Alice text at levels -5 to 1 rose from 0.87-0.89× to
0.92-0.94×, and at levels 2 and 3 to 0.98× and 1.03×.

The [environment record](encoder-comparison-environment.json) identifies the
measured source files and binary by SHA-256. The
[size manifest](encoder-comparison-2026-10-18/sizes.csv) and
[raw Criterion samples and estimates](encoder-comparison-2026-10-18/criterion-samples.json.gz)
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
| -5 | 1.239× | 1.000× |
| -1 | 1.245× | 1.000× |
| 1 | 1.148× | 1.000× |
| 2 | 1.142× | 1.000× |
| 3 | 1.100× | 1.000× |
| 4 | 1.040× | 1.000× |
| 5 | 1.095× | 1.000× |
| 6 | 1.115× | 1.000× |
| 7 | 1.109× | 1.000× |
| 8 | 1.122× | 1.000× |
| 9 | 1.128× | 1.000× |
| 10 | 1.095× | 1.000× |
| 11 | 1.074× | 1.000× |
| 12 | 1.075× | 1.000× |
| 13 | 1.078× | 1.000× |
| 14 | 1.048× | 1.000× |
| 15 | 1.071× | 1.000× |
| 16 | 1.116× | 1.000× |
| 17 | 1.107× | 1.000× |
| 18 | 1.110× | 1.000× |
| 19 | 1.114× | 1.000× |
| 20 | 1.113× | 1.000× |
| 21 | 1.116× | 1.000× |
| 22 | 1.111× | 1.000× |

The unrounded overall median is **1.112091×**. mzstd has a lower mean latency
in **166 of 192** pairs; in 160 of those, its individual 95% confidence interval
for mean latency lies wholly below C's. These intervals have no adjustment for
multiple comparisons.

The medians over all 24 levels for each dataset show the workload differences:

| Dataset | mzstd median speed / C ↑ |
| --- | ---: |
| empty | 4.340× |
| tiny-text | 2.096× |
| alice29 | 1.021× |
| text-1m | 1.041× |
| binary-64k | 1.107× |
| random-64k | 1.071× |
| random-1m | 1.048× |
| repeated-1m | 1.182× |

Text results at selected levels:

| Dataset | Level | mzstd, MiB/s ↑ | C, MiB/s ↑ | Speed / C ↑ | Frame bytes, both |
| --- | ---: | ---: | ---: | ---: | ---: |
| alice29 | 1 | 383.45 | 417.42 | 0.919× | 59,657 |
| alice29 | 3 | 300.61 | 292.42 | 1.028× | 56,970 |
| alice29 | 9 | 55.49 | 57.12 | 0.972× | 52,188 |
| alice29 | 19 | 5.86 | 5.45 | 1.077× | 49,211 |
| text-1m | 1 | 2718.64 | 2518.65 | 1.079× | 61,206 |
| text-1m | 3 | 1911.37 | 1786.20 | 1.070× | 55,878 |
| text-1m | 9 | 422.75 | 430.27 | 0.983× | 52,381 |
| text-1m | 19 | 38.83 | 34.93 | 1.112× | 49,285 |

Every mzstd frame passed byte-for-byte comparison with zstd and decoded back
to the original input through zstd before timing. The
[level pages](encoders/README.md) expose all datasets, timings, throughput and
95% mean confidence bounds.

## Regression checks

Every case was compared with the sweep of the tree before this release's
changes (`enc-before`, run the same day with the same settings). Cases
whose own time rose by more than 3% in the sweep were rerun as separate
processes, the previous tree's profile driver against this release's, nine
interleaved pairs each on an idle CPU 2:

| Cases | Sweep: this release / previous time | Paired: this release / previous time | Verdict |
| --- | ---: | ---: | --- |
| random-1m l-5, l-1 | +14%, +18% | -18%, -22% | faster |
| random-1m l2, l13-l15, l19, l20 | +4% to +13% | -0.5% to +0.8% | noise |
| binary-64k l13, l14 | +4%, +7% | +1.1%, +0.5% | noise |
| alice29 l18 | +5% | -0.1% | noise |

The same paired runs time Alice 5.0% faster at level 1, 6.5% at level 3
and 1.4% at level 9, 1 MiB of text 5.5% faster at level 4 and 2.7% at
level 9. The 1 MiB inputs' cold timings depend on the allocator state the
long-running Criterion process has reached, which moves them by 5-15%
between sweeps.

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

The previous tree's sweep (`enc-before`, median 1.086×) was run earlier
the same day with the same sampling, while nothing else ran. The change
between the two runs is consistent with paired, process-per-case
measurements of the same binaries, but the two sweeps are not a controlled
before/after experiment.

Streaming, retained contexts and parallel compression have separate root
benchmarks (`cargo bench --bench compress`, `--bench parallel`) and are outside
this comparison. See the [harness specification](../../architecture/benchmarking.md).
