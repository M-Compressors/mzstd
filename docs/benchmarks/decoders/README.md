# Decoder comparison

[Benchmark index](../README.md)

Recorded run: **dec-20261015c** (2026-10-15).
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
| [15](l15.md) | 1.000× | 1.042× |
| [20](l20.md) | 1.000× | 1.035× |
| [21](l21.md) | 1.000× | 1.033× |
| [9](l9.md) | 1.000× | 1.022× |
| [-5](l-5.md) | 1.000× | 1.019× |
| [-1](l-1.md) | 1.000× | 1.001× |
| [22](l22.md) | 1.000× | 0.995× |
| [17](l17.md) | 1.000× | 0.994× |
| [7](l7.md) | 1.000× | 0.993× |
| [4](l4.md) | 1.000× | 0.992× |
| [3](l3.md) | 1.000× | 0.992× |
| [18](l18.md) | 1.000× | 0.991× |
| [11](l11.md) | 1.000× | 0.990× |
| [6](l6.md) | 1.000× | 0.989× |
| [5](l5.md) | 1.000× | 0.987× |
| [16](l16.md) | 1.000× | 0.987× |
| [13](l13.md) | 1.000× | 0.987× |
| [8](l8.md) | 1.000× | 0.986× |
| [19](l19.md) | 1.000× | 0.986× |
| [10](l10.md) | 1.000× | 0.985× |
| [1](l1.md) | 1.000× | 0.983× |
| [14](l14.md) | 1.000× | 0.982× |
| [12](l12.md) | 1.000× | 0.978× |
| [2](l2.md) | 1.000× | 0.976× |

Open a source level for all datasets, compressed/restored byte counts,
mean latency, confidence bounds and throughput. Measurements include cold construction,
allocation, decoding and disposal. C receives the frame's declared output capacity.
See the linked methodology for reproducible commands and the limits of this single-host comparison.
