# Compression comparison — 2026-10-14 (fb36eb6 + working tree)

[Benchmark index](README.md) · [Results by level](encoders/README.md)

The [384-row CSV](encoder-comparison.csv) records the final working tree on
`fb36eb6db56038c4b25f34bd7141c880181eb775` against zstd 1.5.7, under the
Criterion baseline `parity-final-20261014`. The median speed relative to C
is **1.000×** over all 192 dataset/level pairs. This is approximately parity
on this corpus; the small margin does not establish an overall lead, and
large text remains slower.

The [environment record](encoder-comparison-environment.json) identifies the
measured source files and binary by SHA-256. The
[size manifest](encoder-comparison-2026-10-14/sizes.csv) and
[raw Criterion samples and estimates](encoder-comparison-2026-10-14/criterion-samples.json.gz)
are archived with this run. The [changelog](../../CHANGELOG.md) describes the
encoder changes; the source hashes match the final working tree.

## Results

![Median speed and output relative to C by level](encoders/charts/overview.svg)

Speed / C is C mean latency divided by mzstd mean latency for the same input.
Output / C is mzstd bytes divided by C bytes. Each level takes the median of
these ratios over eight equally weighted datasets, including empty and tiny
input. These are medians across datasets, not median Criterion sample times.
Higher speed and lower output are better.

| Level | mzstd median speed / C ↑ | mzstd median output / C ↓ |
| ---: | ---: | ---: |
| -5 | 1.110× | 1.000× |
| -1 | 1.095× | 1.000× |
| 1 | 1.056× | 1.000× |
| 2 | 1.035× | 1.000× |
| 3 | 1.062× | 1.000× |
| 4 | 1.013× | 1.000× |
| 5 | 0.977× | 1.000× |
| 6 | 0.995× | 1.000× |
| 7 | 0.956× | 1.000× |
| 8 | 0.974× | 1.000× |
| 9 | 0.932× | 1.000× |
| 10 | 1.024× | 1.000× |
| 11 | 0.999× | 1.000× |
| 12 | 1.001× | 1.000× |
| 13 | 0.972× | 1.000× |
| 14 | 0.942× | 1.000× |
| 15 | 0.961× | 1.000× |
| 16 | 0.986× | 1.000× |
| 17 | 0.973× | 1.000× |
| 18 | 0.998× | 1.000× |
| 19 | 1.010× | 1.000× |
| 20 | 1.015× | 1.000× |
| 21 | 0.999× | 1.000× |
| 22 | 1.012× | 1.000× |

The unrounded overall median is **1.000420×**. mzstd has a lower mean latency
in **96 of 192** pairs; in 84 of those, its individual 95% confidence interval
for mean latency lies wholly below C's. These intervals have no adjustment for
multiple comparisons and do not establish a statistically significant lead
for the aggregate.

The medians over all 24 levels for each dataset show the workload differences:

| Dataset | mzstd median speed / C ↑ |
| --- | ---: |
| empty | 2.115× |
| tiny-text | 1.471× |
| alice29 | 0.925× |
| text-1m | 0.934× |
| binary-64k | 0.991× |
| random-64k | 0.945× |
| random-1m | 0.934× |
| repeated-1m | 1.127× |

Text results at selected levels:

| Dataset | Level | mzstd, MiB/s ↑ | C, MiB/s ↑ | Speed / C ↑ | Frame bytes, both |
| --- | ---: | ---: | ---: | ---: | ---: |
| alice29 | 1 | 317.44 | 404.51 | 0.785× | 59,657 |
| alice29 | 3 | 239.12 | 290.17 | 0.824× | 56,970 |
| alice29 | 9 | 48.97 | 56.13 | 0.872× | 52,188 |
| alice29 | 19 | 5.12 | 5.15 | 0.994× | 49,211 |
| text-1m | 1 | 2077.58 | 2465.44 | 0.843× | 61,206 |
| text-1m | 3 | 1446.19 | 1706.95 | 0.847× | 55,878 |
| text-1m | 9 | 367.59 | 418.55 | 0.878× | 52,381 |
| text-1m | 19 | 34.14 | 33.81 | 1.010× | 49,285 |

Every mzstd frame passed byte-for-byte comparison with zstd and decoded back
to the original input through zstd before timing. The
[level pages](encoders/README.md) expose all datasets, timings, throughput and
95% mean confidence bounds.

## Measurement contract

| Setting | Recorded value |
| --- | --- |
| Host | Intel Core i7-13700KF, x86_64, WSL2 |
| Affinity | One single-threaded process pinned to logical CPU 2; no concurrent decoder sweep |
| Rust | rustc 1.98.1; Cargo release defaults; runtime SIMD dispatch (AVX2 here) |
| C compiler | GCC 11.4.0; `cc` crate defaults (`-O3`) |
| zstd | 1.5.7; submodule `f8745da6ff1ad1e7bab384bd1f9d742439278e99`; single-threaded, no legacy formats, no Huffman assembly |
| Compression | Level only, complete known input, no dictionary or explicit flush |
| API | Cold one-shot: `ZSTD_createCCtx` + `ZSTD_compress2` into a `ZSTD_compressBound` buffer against `Compressor::new(level).compress`; construction, allocation, compression and disposal timed |
| Sampling | 20 samples; 0.2 s warmup; 0.5 s requested measurement per case; 2,000 bootstrap resamples; 95% confidence intervals |
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

This is one complete sweep on one WSL2 host. C and mzstd run consecutively for
each case in a fixed order. Affinity restricts CPU migration but does not
control frequency, thermal state or host scheduling. Confidence bounds do not
capture every source of host or allocator variation. A 0.04% aggregate margin
is too small to claim a stable overall lead.

Every dataset/level pair has equal weight, regardless of input size or execution
time. The median describes this corpus, not a production workload distribution.
Cold APIs include allocation and allocator state; timings include the effects
of glibc's changing mmap threshold within a process.

The previous published encoder sweep used longer warmup and measurement times
and ran concurrently with the decoder sweep. The change in aggregate score
between those runs is not a controlled before/after speedup. The latest
[decoder report](decoder-comparison.md) retains its separate 2026-10-13 data.

Streaming, retained contexts and parallel compression have separate root
benchmarks (`cargo bench --bench compress`, `--bench parallel`) and are outside
this comparison. See the [harness specification](../../architecture/benchmarking.md).
