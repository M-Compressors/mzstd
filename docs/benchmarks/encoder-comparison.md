# Compression comparison — 2026-11-09

[Benchmark index](README.md) · [Results by level](encoders/README.md)

The [384-row CSV](encoder-comparison.csv) records mzstd 0.2.12 (the working
tree on `5ecb52f` with the v0.2.12 changes in the
[changelog](../../CHANGELOG.md)) against zstd 1.5.7, under the Criterion
baseline `long-20261109`. The median speed relative to C is **1.248×** over
all 192 dataset/level pairs, and mzstd has the lower mean latency in
**174** of them; in 153 of those its 95% interval for mean latency lies
wholly below C's. Every frame is byte-identical to zstd's.

The host was loaded by other tenants throughout the run (see
[interpretation limits](#interpretation-limits)). Sampling was lengthened
to 50 samples of 6 s after a 3 s warmup, which keeps the medians stable but
leaves a median 95% interval of about 6% per case, against 0.5% in the
2026-10-30 run. Read single cells as ±10%.

The [environment record](encoder-comparison-environment.json) identifies the
measured sources and binaries by SHA-256. The
[size manifest](encoder-comparison-2026-11-09/sizes.csv) and the raw
[Criterion samples and estimates](encoder-comparison-2026-11-09/criterion-samples.json.gz)
are archived with this run.

## Results

![Median speed and output relative to C by level](encoders/charts/overview.svg)

Speed / C is C mean latency divided by mzstd mean latency for the same input,
in the same process. Each level takes the median of these ratios over eight
equally weighted datasets, including empty and tiny input; higher is better.
The [level overview](encoders/README.md) lists every level's medians.

The medians over all 24 levels for each dataset, with the 2026-10-30 run
(mzstd 0.2.7 plus the then-unreleased changes) for reference:

| Dataset | mzstd speed / C ↑ | 2026-10-30 | Range across levels |
| --- | ---: | ---: | ---: |
| empty | 8.410× | 8.267× | 5.09–11.44× |
| tiny-text | 3.531× | 3.618× | 1.35–9.11× |
| alice29 | 1.008× | 1.019× | 0.84–1.29× |
| text-1m | 1.088× | 1.085× | 0.94–1.33× |
| binary-64k | 1.241× | 1.230× | 1.08–1.80× |
| random-64k | 1.151× | 1.100× | 0.92–1.64× |
| random-1m | 1.111× | 1.218× | 0.85–1.88× |
| repeated-1m | 2.437× | 2.390× | 1.22–3.34× |

Eighteen pairs are slower than C, ten of them Alice (levels -1, 1, 2,
4-8, 10 and 12; 0.84–0.998×). The rest are cyclic Alice at levels 7, 8,
10 and 13 (0.94–0.99×), random 64 KiB at level 14 and random 1 MiB at
levels 4, 13 and 14 (0.85–0.95×). With this run's noise, the cells below
0.9× (Alice level 2, random 1 MiB level 13, Alice levels 8 and 12) are
outside the range of the previous run but not yet confirmed. Text results
at selected levels:

| Dataset | Level | mzstd, MiB/s ↑ | C, MiB/s ↑ | Speed / C ↑ | Frame bytes, both |
| --- | ---: | ---: | ---: | ---: | ---: |
| alice29 | -5 | 487.1 | 439.1 | 1.109× | 85,658 |
| alice29 | 1 | 392.6 | 395.3 | 0.993× | 59,657 |
| alice29 | 3 | 259.4 | 256.5 | 1.011× | 56,970 |
| alice29 | 9 | 54.6 | 54.3 | 1.006× | 52,188 |
| alice29 | 19 | 5.4 | 4.1 | 1.295× | 49,211 |
| text-1m | -5 | 3,415 | 3,130 | 1.091× | 96,657 |
| text-1m | 1 | 2,478 | 2,148 | 1.153× | 61,206 |
| text-1m | 3 | 1,590 | 1,388 | 1.145× | 55,878 |
| text-1m | 9 | 357.8 | 328.2 | 1.090× | 52,381 |
| text-1m | 19 | 37.6 | 34.5 | 1.090× | 49,285 |

Absolute throughput is lower than on 2026-10-30 for both implementations,
since the tenants' load slows C and mzstd alike; compare ratios, not MiB/s.
Every mzstd frame passed byte-for-byte comparison with zstd and decoded back
to the original input through zstd before timing. The
[level pages](encoders/README.md) expose all datasets, timings, throughput and
95% mean confidence bounds.

## Measurement contract

| Setting | Recorded value |
| --- | --- |
| Host | AMD Ryzen 9 7950X, Linux KVM guest with two vCPUs, host shared with other tenants |
| Affinity | The encoder suite's Criterion process on vCPU 1, then the decoder suite's; nothing else ran in the guest |
| Rust | rustc 1.99.1 (LLVM 23.1.1); Cargo release defaults; runtime SIMD dispatch (AVX-512 here) |
| C compiler | GCC 15.2.0; `cc` crate defaults (`-O3`) |
| zstd | 1.5.7; submodule `f8745da6ff1ad1e7bab384bd1f9d742439278e99`; single-threaded, no legacy formats, no Huffman assembly |
| Compression | Level only, complete known input, no dictionary or explicit flush |
| API | Cold one-shot: `ZSTD_createCCtx` + `ZSTD_compress2` into a `ZSTD_compressBound` buffer against `Compressor::new(level).compress`; construction, allocation, compression and disposal timed |
| Sampling | 50 samples; 3 s warmup; 6 s requested measurement per case; Criterion's default 100,000 bootstrap resamples; 95% confidence intervals |
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
cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench encoders --locked --no-run
cd benchmarks/comparison
taskset -c 1 target/release/deps/encoders-<hash> --bench --save-baseline my-encoders --sample-size 50 --warm-up-time 3 --measurement-time 6 --noplot
cd ../..
python3 benchmarks/comparison/report.py --baseline my-encoders --csv /tmp/my-encoders.csv
cp benchmarks/comparison/target/criterion/sizes.csv /tmp/my-encoders-sizes.csv
```

On a quiet host the shorter settings of the
[benchmark guide](../benchmarking.md#encoder-comparison) (20 samples, 0.5 s
warmup, 1 s measurement) reach intervals below 1%. Criterion extends slow
cases to collect the requested samples. The exporter rejects incomplete
matrices and mismatched input lengths. Export immediately after timing and
archive `sizes.csv` with the named baseline: a later preflight replaces that
shared manifest. The archived gzip file holds each case's `benchmark`,
`estimates` and `sample` JSON records from Criterion.

Use the [benchmark guide](../benchmarking.md#encoder-comparison) to regenerate
the checked-in pages and charts from the published CSV and environment
record. Plotting uses Matplotlib 3.10.8 and does not rerun timings.

## Interpretation limits

This is one complete sweep on one KVM guest whose host other tenants loaded
throughout: a fixed C decode, probed every 10 minutes from 08:28 to 15:34
CET, varied by 8-43% between its 10th and 90th percentile wall times, and
identical instruction counts took up to 24% more cycles. Single cases are
therefore noisy (median 95% interval 6.2%, 90th percentile 9.6%; C's own
timings moved by a median 11% against the 2026-10-30 run), while the
per-dataset medians agree with that run to within a few percent. C and
mzstd run consecutively for each case in a fixed order. Affinity restricts
CPU migration but does not control frequency, thermal state or host
scheduling.

Every dataset/level pair has equal weight, regardless of input size or execution
time. The median describes this corpus, not a production workload distribution.
Cold APIs include allocation and allocator state; timings include the effects
of glibc's changing mmap and trim thresholds within a process.

Streaming, retained contexts and parallel compression have separate root
benchmarks (`cargo bench --bench compress`, `--bench parallel`) and are outside
this comparison; [lzbench](lzbench-2026-11-09.md) measures whole-archive
throughput on `silesia.tar`. See the
[harness specification](../../architecture/benchmarking.md).
