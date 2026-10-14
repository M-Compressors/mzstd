# Benchmark results

## Our benchmarks

| Compare | Results | Data |
| --- | --- | --- |
| Compression speed and size | [Levels -5 to 22](encoders/README.md) | [CSV](encoder-comparison.csv), [conditions](encoder-comparison.md), [size manifest](encoder-comparison-2026-10-13/sizes.csv) |
| Decompression speed on identical frames | [Source levels -5 to 22](decoders/README.md) | [CSV](decoder-comparison.csv), [conditions](decoder-comparison.md), [size manifest](decoder-comparison-2026-10-13/sizes.csv) |

### Compression

![Median compression speed and size relative to C](encoders/charts/overview.svg)

mzstd against zstd 1.5.7, 384 cases. Each speed/size bar is the median of eight
per-dataset ratios to C. Higher speed and lower size are better. Every mzstd
frame is byte-identical to zstd's, so the size ratio is 1× at every level.
The median over all 192 dataset/level pairs is 0.975×; mzstd is faster on
empty and tiny input and on run-length data, within 1–3% of C at levels 11–12
and 16–22, and 4–8% behind at levels -5 to 10, where text and
random data take longer in the match finders.
[Exact results and confidence bounds](encoders/README.md).

### Decompression

![Median decompression speed relative to C](decoders/charts/overview.svg)

Both decoders restore the same C-written frames in 384 cases. Level is the
source encoder's setting. Each bar is the median of eight C-time/decoder-time
ratios; higher is faster. mzstd is 1.03–1.04× C's speed at every source level
(median 1.033× over all 192 pairs): much faster on empty and tiny frames,
faster on random and run-length data, and 0.81–0.96× on compressible text.
[Exact results](decoders/README.md), [size manifest](decoder-comparison-2026-10-13/sizes.csv).

### How to interpret the results

Published measurements use an Intel Core i7-13700KF under WSL2 and cold
one-shot APIs including construction, allocation and disposal. Compression
(`enc-2026-10-13`) ran on logical CPU 2 and decompression (`dec-2026-10-13`) on
logical CPU 6 at the same time, each in one process on its own physical core;
C and mzstd alternate within every case, so ratios to C are the comparable
quantity. Both are dated 2026-10-13 and measure `e6db371` plus the unreleased
changes in the [changelog](../../CHANGELOG.md). See each report for
provenance and limits; these are measurements of recorded builds.

Empty and tiny inputs have equal weight with large inputs. Medians describe this
corpus, not a production workload distribution. C decoding knows the output
capacity from the frame header. The lowest mean is not necessarily a
statistically significant lead.

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

### Reproduce

Use [benchmarking and profiling](../benchmarking.md) for validation, fresh timings
and report-generation commands. Published CSVs contain all measured rows;
level pages expose the individual cases rather than only aggregate scores.
