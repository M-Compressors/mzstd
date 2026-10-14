# Decompression comparison — 2026-10-13

[Benchmark index](README.md) · [All level pages](decoders/README.md) ·
[Published CSV](decoder-comparison.csv)

The published 384 rows cover two decoders on eight identical C-written
frames at each source level, recorded as `dec-2026-10-13` (mzstd at
`e6db371` plus the then-unreleased changes, identified by the
[source hashes](decoder-comparison-environment.json),
zstd 1.5.7). mzstd's median speed relative to C is **1.033×** across all 192
dataset/level pairs; mzstd is faster in 120 of them, and in all 120 its 95%
mean timing interval lies wholly below C's. Empty and tiny inputs have the
same weight as large inputs. Per-workload results include cases where mzstd
is slower.

This remains the latest complete decoder run. The 2026-10-14
[encoder update](encoder-comparison.md) uses a separate run and source state.

| Source level | zstd C | mzstd |
| ---: | ---: | ---: |
| -5 | 1.000× | 1.037× |
| -1 | 1.000× | 1.034× |
| 1 | 1.000× | 1.036× |
| 2 | 1.000× | 1.029× |
| 3 | 1.000× | 1.032× |
| 4 | 1.000× | 1.034× |
| 5 | 1.000× | 1.037× |
| 6 | 1.000× | 1.032× |
| 7 | 1.000× | 1.029× |
| 8 | 1.000× | 1.041× |
| 9 | 1.000× | 1.034× |
| 10 | 1.000× | 1.034× |
| 11 | 1.000× | 1.031× |
| 12 | 1.000× | 1.035× |
| 13 | 1.000× | 1.031× |
| 14 | 1.000× | 1.035× |
| 15 | 1.000× | 1.031× |
| 16 | 1.000× | 1.032× |
| 17 | 1.000× | 1.034× |
| 18 | 1.000× | 1.029× |
| 19 | 1.000× | 1.031× |
| 20 | 1.000× | 1.031× |
| 21 | 1.000× | 1.041× |
| 22 | 1.000× | 1.040× |

Each column is the median of eight C mean time / decoder mean time ratios.
The overall value takes the median of all 192 direct ratios, not an average of
level medians. Higher is faster; medians do not establish statistical
significance.

By dataset, the medians across levels are: empty 25.2× and tiny text 20.8×
(mzstd needs no preallocated decoding context), random 64 KiB 1.75×, random
1 MiB 1.04×, repeated bytes 1.03×, structured binary 0.94×, Alice 0.89× and
cyclic Alice 0.82×. For Alice at level 3, mzstd decodes 1,309 MiB/s against
C's 1,511 MiB/s. On compressible text the sequence loop executes more
instructions than the reference's, and `decompress` zeroes its output before
decoding compressed blocks into it, which C, writing into uninitialized
memory, does not pay. This suite times only `decompress`;
`decompress_to_slice` and the streaming sessions are measured by the root
`decompress` benchmark.

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
measured sources, and the [size manifest](decoder-comparison-2026-10-13/sizes.csv)
lists every frame. The decoder sweep ran concurrently with the earlier
2026-10-13 encoder sweep on another physical core, sharing the L3 cache.
This is one WSL2 host;
frequency, thermal state and scheduling are not controlled, and confidence
bounds do not capture every source of host or allocator variation.

Reproduce from the repository root with a fresh baseline name:

```sh
cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench decoders --locked -- --test
taskset -c 6 cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench decoders --locked -- --save-baseline my-decoders
python3 benchmarks/comparison/report.py --decoding --baseline my-decoders --csv /tmp/my-decoders.csv
cp benchmarks/comparison/target/criterion-decoders/sizes.csv /tmp/my-decoders-sizes.csv
```

Raw samples remain local under `benchmarks/comparison/target/criterion-decoders/`.
Use the [benchmark guide](../benchmarking.md#decoder-comparison) to regenerate
the published pages from the recorded CSV and environment metadata.
