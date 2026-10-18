# Decoder comparison

[Benchmark index](../README.md)

Recorded run: **dec-20261018** (2026-10-18).
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
| [-5](l-5.md) | 1.000× | 1.186× |
| [3](l3.md) | 1.000× | 1.135× |
| [6](l6.md) | 1.000× | 1.123× |
| [8](l8.md) | 1.000× | 1.119× |
| [9](l9.md) | 1.000× | 1.119× |
| [5](l5.md) | 1.000× | 1.117× |
| [21](l21.md) | 1.000× | 1.116× |
| [16](l16.md) | 1.000× | 1.115× |
| [10](l10.md) | 1.000× | 1.114× |
| [22](l22.md) | 1.000× | 1.114× |
| [20](l20.md) | 1.000× | 1.114× |
| [18](l18.md) | 1.000× | 1.112× |
| [7](l7.md) | 1.000× | 1.108× |
| [17](l17.md) | 1.000× | 1.108× |
| [11](l11.md) | 1.000× | 1.107× |
| [-1](l-1.md) | 1.000× | 1.104× |
| [15](l15.md) | 1.000× | 1.103× |
| [13](l13.md) | 1.000× | 1.102× |
| [14](l14.md) | 1.000× | 1.099× |
| [1](l1.md) | 1.000× | 1.096× |
| [2](l2.md) | 1.000× | 1.096× |
| [19](l19.md) | 1.000× | 1.087× |
| [4](l4.md) | 1.000× | 1.081× |
| [12](l12.md) | 1.000× | 1.080× |

Open a source level for all datasets, compressed/restored byte counts,
mean latency, confidence bounds and throughput. Measurements include cold construction,
allocation, decoding and disposal. C receives the frame's declared output capacity.
See the linked methodology for reproducible commands and the limits of this single-host comparison.
