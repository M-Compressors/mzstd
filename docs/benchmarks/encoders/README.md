# Encoder comparison

[Benchmark index](../README.md)

Recorded run: **full-20261030** (2026-10-30).
[Raw results](../encoder-comparison.csv) · [Environment](../encoder-comparison-environment.json) · [Run analysis](../encoder-comparison.md).

![Median compression speed and size relative to C](charts/overview.svg)

All 8 datasets contribute equally to each median, including empty and tiny input.
For each dataset, speed / C = C mean latency / mzstd mean latency;
output / C = mzstd bytes / C bytes. Medians are taken over those ratios.
Levels are ordered by mzstd median speed / C, highest first. Frames are byte-identical,
so output / C is 1× at every level.

mzstd main is the same matrix measured on `0a4fd87` in a process beside this run's; its ratios use that run's C timings.

| Level | mzstd speed / C ↑ | mzstd main speed / C | mzstd output / C ↓ | Datasets |
| --- | ---: | ---: | ---: | ---: |
| [Level -5](l-5.md) | 1.704× | 1.698× | 1.000× | 8 |
| [Level -1](l-1.md) | 1.632× | 1.637× | 1.000× | 8 |
| [Level 1](l1.md) | 1.441× | 1.527× | 1.000× | 8 |
| [Level 2](l2.md) | 1.377× | 1.413× | 1.000× | 8 |
| [Level 14](l14.md) | 1.373× | 1.396× | 1.000× | 8 |
| [Level 17](l17.md) | 1.370× | 1.374× | 1.000× | 8 |
| [Level 15](l15.md) | 1.369× | 1.377× | 1.000× | 8 |
| [Level 16](l16.md) | 1.350× | 1.352× | 1.000× | 8 |
| [Level 8](l8.md) | 1.324× | 1.327× | 1.000× | 8 |
| [Level 3](l3.md) | 1.322× | 1.320× | 1.000× | 8 |
| [Level 13](l13.md) | 1.320× | 1.370× | 1.000× | 8 |
| [Level 18](l18.md) | 1.313× | 1.307× | 1.000× | 8 |
| [Level 7](l7.md) | 1.276× | 1.305× | 1.000× | 8 |
| [Level 21](l21.md) | 1.271× | 1.326× | 1.000× | 8 |
| [Level 19](l19.md) | 1.266× | 1.329× | 1.000× | 8 |
| [Level 9](l9.md) | 1.256× | 1.316× | 1.000× | 8 |
| [Level 6](l6.md) | 1.245× | 1.249× | 1.000× | 8 |
| [Level 22](l22.md) | 1.230× | 1.262× | 1.000× | 8 |
| [Level 10](l10.md) | 1.224× | 1.323× | 1.000× | 8 |
| [Level 5](l5.md) | 1.219× | 1.225× | 1.000× | 8 |
| [Level 20](l20.md) | 1.217× | 1.272× | 1.000× | 8 |
| [Level 12](l12.md) | 1.197× | 1.245× | 1.000× | 8 |
| [Level 11](l11.md) | 1.192× | 1.233× | 1.000× | 8 |
| [Level 4](l4.md) | 1.127× | 1.131× | 1.000× | 8 |

## [Level -5](l-5.md)

![Level -5: median across all datasets](charts/l-5-summary.svg)

## [Level -1](l-1.md)

![Level -1: median across all datasets](charts/l-1-summary.svg)

## [Level 1](l1.md)

![Level 1: median across all datasets](charts/l1-summary.svg)

## [Level 2](l2.md)

![Level 2: median across all datasets](charts/l2-summary.svg)

## [Level 14](l14.md)

![Level 14: median across all datasets](charts/l14-summary.svg)

## [Level 17](l17.md)

![Level 17: median across all datasets](charts/l17-summary.svg)

## [Level 15](l15.md)

![Level 15: median across all datasets](charts/l15-summary.svg)

## [Level 16](l16.md)

![Level 16: median across all datasets](charts/l16-summary.svg)

## [Level 8](l8.md)

![Level 8: median across all datasets](charts/l8-summary.svg)

## [Level 3](l3.md)

![Level 3: median across all datasets](charts/l3-summary.svg)

## [Level 13](l13.md)

![Level 13: median across all datasets](charts/l13-summary.svg)

## [Level 18](l18.md)

![Level 18: median across all datasets](charts/l18-summary.svg)

## [Level 7](l7.md)

![Level 7: median across all datasets](charts/l7-summary.svg)

## [Level 21](l21.md)

![Level 21: median across all datasets](charts/l21-summary.svg)

## [Level 19](l19.md)

![Level 19: median across all datasets](charts/l19-summary.svg)

## [Level 9](l9.md)

![Level 9: median across all datasets](charts/l9-summary.svg)

## [Level 6](l6.md)

![Level 6: median across all datasets](charts/l6-summary.svg)

## [Level 22](l22.md)

![Level 22: median across all datasets](charts/l22-summary.svg)

## [Level 10](l10.md)

![Level 10: median across all datasets](charts/l10-summary.svg)

## [Level 5](l5.md)

![Level 5: median across all datasets](charts/l5-summary.svg)

## [Level 20](l20.md)

![Level 20: median across all datasets](charts/l20-summary.svg)

## [Level 12](l12.md)

![Level 12: median across all datasets](charts/l12-summary.svg)

## [Level 11](l11.md)

![Level 11: median across all datasets](charts/l11-summary.svg)

## [Level 4](l4.md)

![Level 4: median across all datasets](charts/l4-summary.svg)
