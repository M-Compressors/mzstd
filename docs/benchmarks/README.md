# Benchmark results

Two kinds of runs, both recorded on **2026-11-12** on an AMD Ryzen 9 7950X
(Linux KVM guest with two vCPUs, its host shared with other tenants): the
cold library comparison of mzstd 0.2.14 against zstd 1.5.7, and lzbench on
`silesia.tar`. Every report identifies its measured sources, sampling
settings and limitations. Other tenants load the host: the medians are
stable, single cells are good to about ±10%.

| Compare | Results | Data |
| --- | --- | --- |
| Compression speed and size, 384 cold cases | [Levels -5 to 22](encoders/README.md), [report](encoder-comparison.md) | [CSV](encoder-comparison.csv), [environment](encoder-comparison-environment.json), [size manifest](encoder-comparison-2026-11-12/sizes.csv), [Criterion samples](encoder-comparison-2026-11-12/criterion-samples.json.gz) |
| Decompression speed on identical frames, 384 cold cases | [Source levels -5 to 22](decoders/README.md), [report](decoder-comparison.md) | [CSV](decoder-comparison.csv), [environment](decoder-comparison-environment.json), [size manifest](decoder-comparison-2026-11-12/sizes.csv), [Criterion samples](decoder-comparison-2026-11-12/criterion-samples.json.gz) |
| lzbench on `silesia.tar`, one frame | [Report](lzbench-2026-11-12.md) | Raw output of every round in the report |

## Compression

![Median compression speed and size relative to C](encoders/charts/overview.svg)

Each speed bar is the median of eight per-dataset ratios to C, timed in
the same process. Higher speed and lower size are better. Every mzstd frame
is byte-identical to zstd's, so the size ratio is 1× at every level. The
median over all 192 dataset/level pairs is **1.216×**, with lower mean
latency in **174 pairs** (2026-11-09: 1.248×, 174 pairs). The per-dataset
medians agree with that run to within 5%: Alice 1.022×, cyclic Alice
1.091×, structured binary 1.230×, repeated bytes 2.459×. The 18 slower
pairs are Alice at nine levels, cyclic Alice at four, structured binary at
two and random data at three, between 0.82× and 0.999×.
[Report](encoder-comparison.md) · [exact results and confidence bounds](encoders/README.md).

## Decompression

![Median decompression speed relative to C](decoders/charts/overview.svg)

Both decoders restore the same C-written frames; level is the source
encoder's setting. The median over all 192 pairs is **1.521×**
(2026-11-09: 1.517×), faster in 152 pairs: tens of times faster on empty
and tiny frames, 3.2× on random 64 KiB, 1.58× on repeated bytes and 1.44×
on structured binary, but 0.911× on Alice and 0.963× on cyclic Alice, the
datasets of 39 of the 40 slower pairs.
[Report](decoder-comparison.md) · [exact results](decoders/README.md).

## lzbench

lzbench master `a6fa8aa` (2.4.1), gcc 15.2 with `-march=native`, `silesia.tar`
as one frame on one vCPU, the fastest of three rounds: mzstd 0.2.14 writes
byte-identical output to zstd 1.5.7 at every level. It compresses 4.9-24.7%
faster at levels 11, 15, 18 and 22, within about 1% of zstd's speed at the
`--fast` levels and levels 3 and 8, and 1.1-2.4% slower at levels 1, 2 and
5. It decompresses at zstd's speed at `--fast` 5 and 3, 1.3% slower at
`--fast` 1 and 3.4-6.4% slower from level 1 up. Against 0.2.13 in the same
rounds, relative to zstd, decompression at the `--fast` levels went from
0.92-0.93 to 0.99-1.01 and gained 1.2-1.7 points at levels 1 to 3;
compression gained 3.7-4.4 points at the `--fast` levels and 1.6-3.7
points at levels 1 to 3. Compression from level 5 up moves by up to 33
points between rounds on this host.
[Command, environment, raw output of every round](lzbench-2026-11-12.md).

## How to interpret the results

The cold suites time construction, allocation, the call and disposal.
Empty and tiny inputs have equal weight with large inputs; medians describe
this corpus, not a production workload. C decoding knows the output
capacity from the frame header. The lowest mean is not necessarily a
statistically significant lead, and on this shared host a single cell's
ratio can move by 10% between sweeps.

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
timings, the main branch's series, the Silesia sweep and the
report-generation commands.
