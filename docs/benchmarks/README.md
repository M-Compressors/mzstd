# Benchmark results

The published comparisons cover eight datasets at 24 compression levels,
with 384 measurements per suite. Both suites were last run on
**2026-10-18**, on mzstd 0.2.7. Each report identifies
its measured sources, sampling settings and limitations.

| Compare | Recorded date | Results | Data |
| --- | --- | --- | --- |
| Compression speed and size | 2026-10-18 | [Levels -5 to 22](encoders/README.md) | [CSV](encoder-comparison.csv), [conditions](encoder-comparison.md), [size manifest](encoder-comparison-2026-10-18/sizes.csv), [Criterion samples](encoder-comparison-2026-10-18/criterion-samples.json.gz) |
| Decompression speed on identical frames | 2026-10-18 | [Source levels -5 to 22](decoders/README.md) | [CSV](decoder-comparison.csv), [conditions](decoder-comparison.md), [size manifest](decoder-comparison-2026-10-18/sizes.csv), [Criterion samples](decoder-comparison-2026-10-18/criterion-samples.json.gz) |

## Compression

![Median compression speed and size relative to C](encoders/charts/overview.svg)

mzstd against zstd 1.5.7. Each speed/size bar is the median of eight
per-dataset ratios to C. Higher speed and lower size are better. Every mzstd
frame is byte-identical to zstd's, so the size ratio is 1× at every level.
The median over all 192 dataset/level pairs is **1.112×**, with lower mean
latency in **166 pairs** (1.086× and 159 pairs for the tree before this
release, swept the same day with the same settings). Every dataset's median
across levels is now above 1×: Alice text **1.021×**, cyclic Alice at 1 MiB
**1.041×**. Alice at levels -5 to 1 remains slower (0.92–0.94×, up from
0.87–0.89×). Cases that looked slower than before in the sweep were rechecked
with paired runs and were not
([regression checks](encoder-comparison.md#regression-checks)).
[Exact results and confidence bounds](encoders/README.md).

## Decompression

![Median decompression speed relative to C](decoders/charts/overview.svg)

Both decoders restore the same C-written frames. Level is the source
encoder's setting. Each bar is the median of eight C-time/decoder-time
ratios; higher is faster. The median over all 192 pairs is **1.111×**
(1.032× before this release), faster in 147 pairs: much faster on empty and
tiny frames and on random data, 1.10× on structured binary, 1.12× on
repeated bytes, and 0.95× (Alice) and 0.98× (cyclic Alice) on compressible
text. `decompress` no longer zeroes its output a block ahead, and Huffman
literals decode faster ([details](decoder-comparison.md)).
[Exact results](decoders/README.md).

## How to interpret the results

Both suites use an Intel Core i7-13700KF under WSL2 and cold one-shot APIs
including construction, allocation and disposal. Both measured mzstd 0.2.7
(the working tree on `eb10fd0`) side by side: the encoder run `enc-20261018`
pinned to logical CPU 2 and the decoder run `dec-20261018` pinned to logical
CPU 6, each with 0.2 s warmup, 0.5 s requested measurement and 20 samples.
The [encoder environment](encoder-comparison-environment.json) and
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
