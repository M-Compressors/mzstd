# Benchmark results

The published comparisons cover eight datasets at 24 compression levels,
with 384 measurements per suite. Compression was updated on **2026-10-14**;
the latest decompression run is dated **2026-10-13**. Each report identifies
its measured sources, sampling settings and limitations.

| Compare | Recorded date | Results | Data |
| --- | --- | --- | --- |
| Compression speed and size | 2026-10-14 | [Levels -5 to 22](encoders/README.md) | [CSV](encoder-comparison.csv), [conditions](encoder-comparison.md), [size manifest](encoder-comparison-2026-10-14/sizes.csv), [Criterion samples](encoder-comparison-2026-10-14/criterion-samples.json.gz) |
| Decompression speed on identical frames | 2026-10-13 | [Source levels -5 to 22](decoders/README.md) | [CSV](decoder-comparison.csv), [conditions](decoder-comparison.md), [size manifest](decoder-comparison-2026-10-13/sizes.csv) |

## Compression

![Median compression speed and size relative to C](encoders/charts/overview.svg)

mzstd against zstd 1.5.7. Each speed/size bar is the median of eight
per-dataset ratios to C. Higher speed and lower size are better. Every mzstd
frame is byte-identical to zstd's, so the size ratio is 1× at every level.
The median over all 192 dataset/level pairs is **1.000×**, with lower mean
latency in **96 pairs**. This small aggregate margin does not establish an
overall lead. Across levels, Alice text measures a median **0.925×**, cyclic
Alice at 1 MiB **0.934×**, and repeated bytes **1.127×**.
[Exact results and confidence bounds](encoders/README.md).

## Decompression

![Median decompression speed relative to C](decoders/charts/overview.svg)

Both decoders restore the same C-written frames. Level is the source
encoder's setting. Each bar is the median of eight C-time/decoder-time
ratios; higher is faster. The median at every source level is **1.03–1.04×**
(median **1.033×** over all 192 pairs): much faster on empty and tiny frames,
faster on random and run-length data, and 0.81–0.96× on compressible text.
[Exact results](decoders/README.md).

## How to interpret the results

Both suites use an Intel Core i7-13700KF under WSL2 and cold one-shot APIs
including construction, allocation and disposal. The encoder run
`parity-final-20261014` measured the final working tree on `fb36eb6`, pinned
to logical CPU 2, with 0.2 s warmup, 0.5 s requested measurement and 20 samples.
It ran without a concurrent decoder sweep.

The decoder run `dec-2026-10-13` measured `e6db371` plus the then-unreleased
changes, pinned to logical CPU 6, with 1 s warmup, 3 s requested measurement
and 20 samples. It ran concurrently with the earlier encoder sweep on another
physical core. The two published suites are separate recorded builds; the
[encoder environment](encoder-comparison-environment.json) and
[decoder environment](decoder-comparison-environment.json) retain their source
hashes.

Empty and tiny inputs have equal weight with large inputs. Medians describe
this corpus, not a production workload distribution. C decoding knows the
output capacity from the frame header. The lowest mean is not necessarily a
statistically significant lead. Ratios from sweeps with different conditions
are not controlled before/after measurements.

| Dataset | Input bytes | Content |
| --- | ---: | --- |
| empty | 0 | API overhead without input |
| tiny-text | 44 | Quick-brown-fox sentence |
| alice29 | 152,089 | *Alice in Wonderland* text ([`testdata/alice29.txt`](../../testdata/README.md)) |
| text-1m | 1,048,576 | Alice repeated cyclically |
| binary-64k | 65,536 | Structured binary bytes |
| random-64k | 65,536 | Prefix of the pseudorandom corpus |
| random-1m | 1,048,576 | Deterministic xorshift64 bytes |
| repeated-1m | 1,048,576 | Repeated `a` bytes |

The datasets, generators and seeds are mbrotli's, so the two projects'
comparisons describe one corpus.

## Reproduce

Use [benchmarking and profiling](../benchmarking.md) for validation, fresh
timings and report-generation commands. Published CSVs contain all measured
rows; level pages expose the individual cases and confidence bounds.
