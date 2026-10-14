# Decompression comparison — 2026-10-14

[Benchmark index](README.md) · [All level pages](decoders/README.md) ·
[Published CSV](decoder-comparison.csv)

The published 384 rows cover two decoders on eight identical C-written
frames at each source level, recorded as `dec-20261014-speed` (mzstd at
`cd91929` plus the unreleased changes, identified by the
[source hashes](decoder-comparison-environment.json),
zstd 1.5.7). mzstd's median speed relative to C is **1.022×** across all 192
dataset/level pairs; mzstd is faster in 99 of them, and in 97 its 95%
mean timing interval lies wholly below C's. Empty and tiny inputs have the
same weight as large inputs. Per-workload results include cases where mzstd
is slower.

| Source level | zstd C | mzstd |
| ---: | ---: | ---: |
| -5 | 1.000× | 1.027× |
| -1 | 1.000× | 0.992× |
| 1 | 1.000× | 0.998× |
| 2 | 1.000× | 0.981× |
| 3 | 1.000× | 0.985× |
| 4 | 1.000× | 0.971× |
| 5 | 1.000× | 0.995× |
| 6 | 1.000× | 0.989× |
| 7 | 1.000× | 0.994× |
| 8 | 1.000× | 1.007× |
| 9 | 1.000× | 0.984× |
| 10 | 1.000× | 0.991× |
| 11 | 1.000× | 0.975× |
| 12 | 1.000× | 0.983× |
| 13 | 1.000× | 0.990× |
| 14 | 1.000× | 0.999× |
| 15 | 1.000× | 1.001× |
| 16 | 1.000× | 0.992× |
| 17 | 1.000× | 0.998× |
| 18 | 1.000× | 0.987× |
| 19 | 1.000× | 0.998× |
| 20 | 1.000× | 1.029× |
| 21 | 1.000× | 1.032× |
| 22 | 1.000× | 1.023× |

Each column is the median of eight C mean time / decoder mean time ratios.
The overall value takes the median of all 192 direct ratios, not an average of
level medians. Higher is faster; medians do not establish statistical
significance.

By dataset, the medians across levels are: empty 23.7× and tiny text 20.4×
(mzstd needs no preallocated decoding context), random 64 KiB 1.73×, random
1 MiB 1.04×, structured binary 0.95×, Alice 0.92×, cyclic Alice
0.85× and repeated bytes 0.91×. Against the 2026-10-13 run, text gained
(Alice 0.89× → 0.92×, cyclic Alice 0.82× → 0.85×). For Alice at level 3, mzstd
decodes 1,353 MiB/s against C's 1,529 MiB/s. On compressible text the
sequence loop still executes about 5% more instructions than the
reference's, and `decompress` zeroes its output before decoding compressed
blocks into it, which C, writing into uninitialized memory, does not pay.

Repeated bytes measured 1.03× in the previous run and 0.91× here, with no
change to the code that decodes them. Their cold timing is bimodal inside one
long-running process: rerun alone through the same Criterion binary, levels 1
and 3 decoded in 15.9 µs (C 25.2 µs) and level 19 in 27.3 µs (C 25.3 µs), and
in a separate process per case (`paired.py`, ten pairs) every tested level
decoded in about 18 µs against C's 25 µs (1.38×). The 1 MiB output allocation
either reuses mapped memory or faults fresh pages, depending on the allocator
state the process has reached. This suite times only `decompress`;
`decompress_to_slice` and the streaming sessions are measured by the root
`decompress` benchmark.

## Regression checks

Cases slower than in the 2026-10-13 run were rerun with `paired.py`: one
process per case, randomized AB/BA order, ten pairs, the previous tree's
driver against this tree's, and C against this tree's. None is a regression
of the decoder:

| Cases | Previous tree → this tree | C → this tree | Verdict |
| --- | ---: | ---: | --- |
| empty l-5, l2, l6; tiny-text l5, l9, l20 | 0.98-1.02× | 20-24× | noise: 30-38 ns either way |
| repeated-1m l-5, l5, l12, l17 | 1.01-1.02× | 1.37-1.39× | allocator state of the sweep's process (see above) |
| binary-64k l4; text-1m l19 | 1.06×; 1.02× | 0.87×; 0.85× | faster than before; still slower than C |

## Measurement contract

Cold one-shot APIs include construction, allocation, decoding and disposal: C
reads the frame's content size (`ZSTD_getFrameContentSize`) and decodes with
`ZSTD_decompress` into an uninitialized buffer of exactly that size; mzstd
runs `Decompressor::default().decompress`, which returns its own vector.
Throughput counts restored bytes. The shared frame sizes belong to the C
encoder, not to a decoder. Every case must restore the original bytes through
both decoders before timing.

The corpus is empty input, tiny text, Alice, cyclic Alice at 1 MiB, structured
binary at 64 KiB, random bytes at 64 KiB and 1 MiB, and repeated `a` at 1 MiB.
The host is an i7-13700KF under WSL2, one process pinned to logical CPU 6.
Sampling uses the harness defaults: 20 samples, 1 s warmup and at least 3 s
measurement per case.

## Provenance and limits

The [environment record](decoder-comparison-environment.json) hashes the
measured sources and binary, the [size manifest](decoder-comparison-2026-10-14/sizes.csv)
lists every frame, and the
[raw Criterion samples and estimates](decoder-comparison-2026-10-14/criterion-samples.json.gz)
are archived. The decoder sweep ran concurrently with the 2026-10-14 encoder
sweep on another physical core, sharing the L3 cache. This is one WSL2 host;
frequency, thermal state and scheduling are not controlled, and confidence
bounds do not capture every source of host or allocator variation.

Reproduce from the repository root with a fresh baseline name:

```sh
cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench decoders --locked -- --test
taskset -c 6 cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench decoders --locked -- --save-baseline my-decoders
python3 benchmarks/comparison/report.py --decoding --baseline my-decoders --csv /tmp/my-decoders.csv
cp benchmarks/comparison/target/criterion-decoders/sizes.csv /tmp/my-decoders-sizes.csv
```

Use the [benchmark guide](../benchmarking.md#decoder-comparison) to regenerate
the published pages from the recorded CSV and environment metadata.
