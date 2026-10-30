# Compression comparison — 2026-10-30

[Benchmark index](README.md) · [Results by level](encoders/README.md)

The [384-row CSV](encoder-comparison.csv) records the working tree on
`0a4fd87` (mzstd 0.2.7 plus the Unreleased changes in the
[changelog](../../CHANGELOG.md)) against zstd 1.5.7, under the Criterion
baseline `full-20261030`. The median speed relative to C is **1.256×** over
all 192 dataset/level pairs, and mzstd has the lower mean latency in
**179** of them; in all 179 its 95% interval for mean latency lies wholly
below C's. The main branch (`0a4fd87`), measured in a second process beside
it ([its CSV](encoder-comparison-2026-10-30/main.csv)), reached 1.254× and
178 pairs. Every frame is byte-identical to zstd's.

The [environment record](encoder-comparison-environment.json) identifies the
measured sources and binaries by SHA-256. The
[size manifest](encoder-comparison-2026-10-30/sizes.csv) and the raw
Criterion samples and estimates of the
[working tree](encoder-comparison-2026-10-30/criterion-samples.json.gz) and
[main](encoder-comparison-2026-10-30/main-criterion-samples.json.gz) are
archived with this run.

## Results

![Median speed and output relative to C by level](encoders/charts/overview.svg)

Speed / C is C mean latency divided by mzstd mean latency for the same input,
in the same process. Each level takes the median of these ratios over eight
equally weighted datasets, including empty and tiny input; higher is better.
mzstd main is normalized to the C timings of its own process. The
[level overview](encoders/README.md) lists every level's medians.

The medians over all 24 levels for each dataset:

| Dataset | mzstd speed / C ↑ | mzstd main speed / C | Range across levels (mzstd) |
| --- | ---: | ---: | ---: |
| empty | 8.267× | 8.721× | 5.63–11.22× |
| tiny-text | 3.618× | 3.604× | 1.51–8.72× |
| alice29 | 1.019× | 0.985× | 0.94–1.10× |
| text-1m | 1.085× | 1.079× | 1.03–1.25× |
| binary-64k | 1.230× | 1.238× | 1.08–1.82× |
| random-64k | 1.100× | 1.143× | 1.02–1.59× |
| random-1m | 1.218× | 1.225× | 0.97–2.43× |
| repeated-1m | 2.390× | 2.104× | 1.47–3.39× |

Thirteen pairs remain slower than C: Alice at levels 6 (0.943×), 7
(0.949×), 4 (0.961×), 8 and 12 (0.967×), 5 (0.972×), 11 (0.976×), 2
(0.982×), 1 (0.985×), -1 (0.990×) and 9 (0.992×), and random 1 MiB at
levels 4 (0.972×) and 3 (0.979×). Single cold Alice cells move by about 3%
between sweeps. Text results at selected levels:

| Dataset | Level | mzstd, MiB/s ↑ | C, MiB/s ↑ | Speed / C ↑ | Frame bytes, both |
| --- | ---: | ---: | ---: | ---: | ---: |
| alice29 | -5 | 533.6 | 498.2 | 1.071× | 85,658 |
| alice29 | 1 | 419.2 | 425.6 | 0.985× | 59,657 |
| alice29 | 3 | 305.2 | 285.4 | 1.069× | 56,970 |
| alice29 | 9 | 58.4 | 58.8 | 0.992× | 52,188 |
| alice29 | 19 | 6.3 | 5.8 | 1.082× | 49,211 |
| text-1m | -5 | 4,742 | 3,780 | 1.255× | 96,657 |
| text-1m | 1 | 3,008 | 2,405 | 1.251× | 61,206 |
| text-1m | 3 | 1,977 | 1,684 | 1.174× | 55,878 |
| text-1m | 9 | 454.1 | 436.1 | 1.041× | 52,381 |
| text-1m | 19 | 41.3 | 37.8 | 1.092× | 49,285 |

Every mzstd frame passed byte-for-byte comparison with zstd and decoded back
to the original input through zstd before timing. The
[level pages](encoders/README.md) expose all datasets, timings, throughput and
95% mean confidence bounds.

## Against the main branch

Separate processes per case: the main branch's `profile` driver against
the working tree's, five seeded AB/BA pairs of 0.1 s per side on one vCPU,
with iteration counts calibrated from this run's CSV (encoder pairs on
vCPU 0 while the decoder pairs ran on vCPU 1). Each value is the median
over pairs of main time / working-tree time; above 1, the working tree is
faster. [Every pair](encoder-comparison-2026-10-30/paired-main.json.gz)
is archived.

| Dataset | Median over levels | Lowest level | Highest level |
| --- | ---: | ---: | ---: |
| empty | 0.963 | 0.922 | 1.006 |
| tiny-text | 1.005 | 0.941 | 1.033 |
| alice29 | 1.007 | 0.995 | 1.092 |
| text-1m | 1.018 | 0.995 | 1.118 |
| binary-64k | 1.003 | 0.986 | 1.015 |
| random-64k | 1.000 | 0.889 | 1.011 |
| random-1m | 0.999 | 0.969 | 1.019 |
| repeated-1m | 1.060 | 1.034 | 1.350 |

Over all 192 cases the median is 1.005: 119 are faster, 54 by more than
2%, and 44 are more than 2% slower. Alice is faster at every level but 12
(0.995): 9.2% at level -5, 7.3% at -1, 8.2% at 1 and 2-3% at levels 5, 9
and 10; cyclic Alice 1-12% at levels -5 to 12. The slower cases have two
causes, both described in the
[encoder core's known gaps](../../architecture/encoder-core.md#known-gaps).
Per-call setup costs about 70 instructions more than on main: empty input
compresses 3.7% slower in the median, the 44-byte text 2-6% slower at
levels -5 to 10 (and 2-3% faster above). The lazy loop's block-relative
positions, kept because they make Alice faster with reused contexts, cost
incompressible data at the row levels through register pressure: random
64 KiB 3-11% at levels 7-10 (level 10 0.889), random 1 MiB 2-3%.

## Measurement contract

| Setting | Recorded value |
| --- | --- |
| Host | AMD Ryzen 9 7950X, Linux KVM guest with two dedicated vCPUs |
| Affinity | The working tree's Criterion process on vCPU 0 and the main branch's on vCPU 1, side by side; then the decoder suites the same way |
| Rust | rustc 1.99.1 (LLVM 23.1.1); Cargo release defaults; runtime SIMD dispatch (AVX-512 here) |
| C compiler | GCC 15.2.0; `cc` crate defaults (`-O3`) |
| zstd | 1.5.7; submodule `f8745da6ff1ad1e7bab384bd1f9d742439278e99`; single-threaded, no legacy formats, no Huffman assembly |
| Compression | Level only, complete known input, no dictionary or explicit flush |
| API | Cold one-shot: `ZSTD_createCCtx` + `ZSTD_compress2` into a `ZSTD_compressBound` buffer against `Compressor::new(level).compress`; construction, allocation, compression and disposal timed |
| Sampling | 20 samples; 0.5 s warmup; 1 s requested measurement per case; Criterion's default 100,000 bootstrap resamples; 95% confidence intervals |
| Validation | All 192 mzstd frames of both trees byte-identical to zstd and decoded to the original input through zstd before timing |

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
taskset -c 0 target/release/deps/encoders-<hash> --bench --save-baseline my-encoders --sample-size 20 --warm-up-time 0.5 --measurement-time 1 --noplot
cd ../..
python3 benchmarks/comparison/report.py --baseline my-encoders --csv /tmp/my-encoders.csv
cp benchmarks/comparison/target/criterion/sizes.csv /tmp/my-encoders-sizes.csv
```

The [benchmark guide](../benchmarking.md#main-branch-series) describes the
main branch's run beside it and the paired check. Criterion extends slow
cases to collect the requested samples. The exporter rejects incomplete
matrices and mismatched input lengths. Export immediately after timing and
archive `sizes.csv` with the named baseline: a later preflight replaces that
shared manifest. The archived gzip files hold each case's `benchmark`,
`estimates` and `sample` JSON records from Criterion.

Use the [benchmark guide](../benchmarking.md#encoder-comparison) to regenerate
the checked-in pages and charts from the published CSVs and environment
record. Plotting uses Matplotlib 3.10.8 and does not rerun timings.

## Interpretation limits

This is one complete sweep on one KVM guest. C and mzstd run consecutively
for each case in a fixed order. Affinity restricts CPU migration but does not
control frequency, thermal state or host scheduling. Confidence bounds
do not capture every source of host or allocator variation.

Every dataset/level pair has equal weight, regardless of input size or execution
time. The median describes this corpus, not a production workload distribution.
Cold APIs include allocation and allocator state; timings include the effects
of glibc's changing mmap and trim thresholds within a process. On empty input,
on the 1 MiB inputs and at levels 13 and above, where each call allocates
several MiB of tables, the same case's speed / C differs between the two
processes by up to 40% in either direction, so per-level medians of the
two trees differ by more than their code does. The paired runs above, not the side-by-side
medians, measure the change from main.

Streaming, retained contexts and parallel compression have separate root
benchmarks (`cargo bench --bench compress`, `--bench parallel`) and are outside
this comparison; [Silesia](silesia.md) measures retained contexts on large
files. See the [harness specification](../../architecture/benchmarking.md).
