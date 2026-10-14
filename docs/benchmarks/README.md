# Benchmark results

The published comparisons cover eight datasets at 24 compression levels,
with 384 measurements per suite. Both suites were last run on
**2026-10-14**. Each report identifies
its measured sources, sampling settings and limitations.

| Compare | Recorded date | Results | Data |
| --- | --- | --- | --- |
| Compression speed and size | 2026-10-14 | [Levels -5 to 22](encoders/README.md) | [CSV](encoder-comparison.csv), [conditions](encoder-comparison.md), [size manifest](encoder-comparison-2026-10-14-fix/sizes.csv), [Criterion samples](encoder-comparison-2026-10-14-fix/criterion-samples.json.gz) |
| Decompression speed on identical frames | 2026-10-14 | [Source levels -5 to 22](decoders/README.md) | [CSV](decoder-comparison.csv), [conditions](decoder-comparison.md), [size manifest](decoder-comparison-2026-10-14/sizes.csv), [Criterion samples](decoder-comparison-2026-10-14/criterion-samples.json.gz) |

## Compression

![Median compression speed and size relative to C](encoders/charts/overview.svg)

mzstd against zstd 1.5.7. Each speed/size bar is the median of eight
per-dataset ratios to C. Higher speed and lower size are better. Every mzstd
frame is byte-identical to zstd's, so the size ratio is 1× at every level.
The median over all 192 dataset/level pairs is **1.092×**, with lower mean
latency in **158 pairs** (1.000× and 96 pairs in the previous run). Across
levels, Alice text measures a median **1.000×**, cyclic Alice at 1 MiB
**1.009×**, and every other dataset's median is above 1. Text at the fastest
levels remains slower (Alice 0.87–0.88× at levels -5 to 1). Cases slower than
in the previous run were rechecked with paired runs
([regression checks](encoder-comparison.md#regression-checks)).
[Exact results and confidence bounds](encoders/README.md).

## Decompression

![Median decompression speed relative to C](decoders/charts/overview.svg)

Both decoders restore the same C-written frames. Level is the source
encoder's setting. Each bar is the median of eight C-time/decoder-time
ratios; higher is faster. The median over all 192 pairs is **1.022×**: much
faster on empty and tiny frames and on random data, and 0.79–0.97× on
compressible text (Alice median 0.92×, cyclic Alice 0.85×, up from 0.89× and
0.82×). Repeated bytes measured 0.91× in this sweep but 1.38× in separate
processes; their cold timing depends on the allocator state of the long-running
process ([details](decoder-comparison.md)).
[Exact results](decoders/README.md).

## How to interpret the results

Both suites use an Intel Core i7-13700KF under WSL2 and cold one-shot APIs
including construction, allocation and disposal. Both measured the working
tree on `cd91929`: the encoder run `enc-20261014-fix` pinned to logical CPU 2
with 0.2 s warmup, 0.5 s requested measurement and 20 samples, alone; the
decoder run `dec-20261014-speed` pinned to logical CPU 6 with 1 s warmup, 3 s
requested measurement and 20 samples, beside an earlier encoder sweep of the
same tree. The
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
