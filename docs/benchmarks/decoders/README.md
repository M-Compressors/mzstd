# Decoder comparison

[Benchmark index](../README.md)

Recorded run: **full-20261030** (2026-10-30).
[CSV](../decoder-comparison.csv) · [Environment](../decoder-comparison-environment.json) · [Methodology and limits](../decoder-comparison.md).

![Median decompression speed relative to C](charts/overview.svg)

Each value is the median of C mean time / decoder mean time across all 8 datasets,
equally weighted. Higher is faster; 1× matches C.
Levels are ordered by mzstd median speed / C. Medians do not imply statistical significance.

Both decoders restore identical C-written frames. The level is an encoder setting,
not a decoder setting. Compressed sizes and ratios are properties of those shared inputs,
so there is no decoder size ranking.

mzstd main is the same matrix measured on `0a4fd87` in a process beside this run's; its ratios use that run's C timings.

| Source level | zstd C | mzstd main | mzstd |
| --- | ---: | ---: | ---: |
| [-5](l-5.md) | 1.000× | 1.471× | 1.642× |
| [-1](l-1.md) | 1.000× | 1.439× | 1.615× |
| [1](l1.md) | 1.000× | 1.462× | 1.565× |
| [2](l2.md) | 1.000× | 1.442× | 1.557× |
| [3](l3.md) | 1.000× | 1.433× | 1.497× |
| [8](l8.md) | 1.000× | 1.377× | 1.490× |
| [6](l6.md) | 1.000× | 1.395× | 1.480× |
| [10](l10.md) | 1.000× | 1.380× | 1.479× |
| [5](l5.md) | 1.000× | 1.374× | 1.479× |
| [7](l7.md) | 1.000× | 1.379× | 1.475× |
| [9](l9.md) | 1.000× | 1.363× | 1.474× |
| [12](l12.md) | 1.000× | 1.378× | 1.472× |
| [14](l14.md) | 1.000× | 1.378× | 1.469× |
| [18](l18.md) | 1.000× | 1.369× | 1.468× |
| [16](l16.md) | 1.000× | 1.362× | 1.468× |
| [19](l19.md) | 1.000× | 1.394× | 1.466× |
| [11](l11.md) | 1.000× | 1.377× | 1.466× |
| [13](l13.md) | 1.000× | 1.381× | 1.462× |
| [15](l15.md) | 1.000× | 1.368× | 1.461× |
| [20](l20.md) | 1.000× | 1.377× | 1.453× |
| [22](l22.md) | 1.000× | 1.361× | 1.453× |
| [17](l17.md) | 1.000× | 1.378× | 1.452× |
| [21](l21.md) | 1.000× | 1.370× | 1.448× |
| [4](l4.md) | 1.000× | 1.274× | 1.322× |

Open a source level for all datasets, compressed/restored byte counts,
mean latency, confidence bounds and throughput. Measurements include cold construction,
allocation, decoding and disposal. C receives the frame's declared output capacity.
See the linked methodology for reproducible commands and the limits of this single-host comparison.
