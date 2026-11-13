# Decoder comparison

[Benchmark index](../README.md)

Recorded run: **long-20261112** (2026-11-12).
[CSV](../decoder-comparison.csv) · [Environment](../decoder-comparison-environment.json) · [Methodology and limits](../decoder-comparison.md).

![Median decompression speed relative to C](charts/overview.svg)

Each value is the median of C mean time / decoder mean time across all 8 datasets,
equally weighted. Higher is faster; 1× matches C.
Levels are ordered by mzstd median speed / C. Medians do not imply statistical significance.

Both decoders restore identical C-written frames. The level is an encoder setting,
not a decoder setting. Compressed sizes and ratios are properties of those shared inputs,
so there is no decoder size ranking.

| Source level | zstd C | mzstd |
| --- | ---: | ---: |
| [-5](l-5.md) | 1.000× | 1.832× |
| [1](l1.md) | 1.000× | 1.614× |
| [6](l6.md) | 1.000× | 1.584× |
| [20](l20.md) | 1.000× | 1.576× |
| [13](l13.md) | 1.000× | 1.548× |
| [22](l22.md) | 1.000× | 1.544× |
| [8](l8.md) | 1.000× | 1.536× |
| [10](l10.md) | 1.000× | 1.524× |
| [14](l14.md) | 1.000× | 1.521× |
| [3](l3.md) | 1.000× | 1.517× |
| [11](l11.md) | 1.000× | 1.510× |
| [7](l7.md) | 1.000× | 1.507× |
| [-1](l-1.md) | 1.000× | 1.503× |
| [9](l9.md) | 1.000× | 1.502× |
| [2](l2.md) | 1.000× | 1.501× |
| [18](l18.md) | 1.000× | 1.500× |
| [19](l19.md) | 1.000× | 1.495× |
| [15](l15.md) | 1.000× | 1.492× |
| [12](l12.md) | 1.000× | 1.488× |
| [21](l21.md) | 1.000× | 1.479× |
| [17](l17.md) | 1.000× | 1.454× |
| [5](l5.md) | 1.000× | 1.448× |
| [16](l16.md) | 1.000× | 1.428× |
| [4](l4.md) | 1.000× | 1.291× |

Open a source level for all datasets, compressed/restored byte counts,
mean latency, confidence bounds and throughput. Measurements include cold construction,
allocation, decoding and disposal. C receives the frame's declared output capacity.
See the linked methodology for reproducible commands and the limits of this single-host comparison.
