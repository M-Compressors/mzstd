# Decompression comparison — 2026-10-18 (mzstd 0.2.7)

[Benchmark index](README.md) · [All level pages](decoders/README.md) ·
[Published CSV](decoder-comparison.csv)

The published 384 rows cover two decoders on eight identical C-written
frames at each source level, recorded as `dec-20261018` (mzstd 0.2.7, the
working tree on `eb10fd0` identified by the
[source hashes](decoder-comparison-environment.json), zstd 1.5.7).
mzstd's median speed relative to C is **1.111×** across all 192
dataset/level pairs; mzstd is faster in 147 of them, and in 146 its 95%
mean timing interval lies wholly below C's. The tree before this release's
changes measured 1.032× and 122 pairs in the same sweep run earlier the
same day (`dec-before`). Empty and tiny inputs have the same weight as
large inputs. Per-workload results include cases where mzstd is slower.

| Source level | zstd C | mzstd |
| ---: | ---: | ---: |
| -5 | 1.000× | 1.186× |
| -1 | 1.000× | 1.104× |
| 1 | 1.000× | 1.096× |
| 2 | 1.000× | 1.096× |
| 3 | 1.000× | 1.135× |
| 4 | 1.000× | 1.081× |
| 5 | 1.000× | 1.117× |
| 6 | 1.000× | 1.123× |
| 7 | 1.000× | 1.108× |
| 8 | 1.000× | 1.119× |
| 9 | 1.000× | 1.119× |
| 10 | 1.000× | 1.114× |
| 11 | 1.000× | 1.107× |
| 12 | 1.000× | 1.080× |
| 13 | 1.000× | 1.102× |
| 14 | 1.000× | 1.099× |
| 15 | 1.000× | 1.103× |
| 16 | 1.000× | 1.115× |
| 17 | 1.000× | 1.108× |
| 18 | 1.000× | 1.112× |
| 19 | 1.000× | 1.087× |
| 20 | 1.000× | 1.114× |
| 21 | 1.000× | 1.116× |
| 22 | 1.000× | 1.114× |

Each column is the median of eight C mean time / decoder mean time ratios.
The overall value takes the median of all 192 direct ratios, not an average of
level medians. Higher is faster; medians do not establish statistical
significance.

By dataset, the medians across levels are: empty 20.9× and tiny text
18.0× (mzstd needs no preallocated decoding context), random 64 KiB
1.70×, repeated bytes 1.12×, structured binary 1.10× (0.94× before),
random 1 MiB 1.04×, cyclic Alice 0.98× (0.86× before) and Alice 0.95×
(0.93× before). For Alice at level 3, mzstd decodes 1,416 MiB/s against
C's 1,515 MiB/s. `decompress` no longer zeroes its output a block ahead:
sequences of 1 KiB or more and trailing literals are appended, and only
the room of shorter sequences is zeroed, 4 KiB at a time. On Alice the
sequence loop runs at the reference's instruction count; the rest of the
gap is the Huffman literal loops (about 13 instructions per pair lookup
against 11) and that zeroing
([decoder fast paths](../../architecture/decoder-performance.md#known-gaps)).

This suite times only `decompress`; `decompress_to_slice` and the streaming
sessions are measured by the root `decompress` benchmark.

## Regression checks

Every case was compared with the sweep of the tree before this release's
changes (`dec-before`). Cases whose own time rose by more than 3% were
rerun as separate processes, the previous tree's profile driver against
this release's, nine interleaved pairs each on an idle CPU 2. None was
slower: Alice level 17 (+12% in the sweep) took 2.3% less time, cyclic
Alice level 5 (+9%) 11.2% less, random 1 MiB at levels -5, 12, 14 and 22
(+8% to +11%) and random 64 KiB level 5 (+5%) moved by -0.7% to +0.8%.
The same runs time cyclic Alice 11-16% faster at levels 1, 5, 10 and 18,
structured binary 19% faster at level 1, repeated bytes 12% at level 3,
Alice 4.8% at level 1, tiny text 4.4% and empty input 2.1%.

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
Sampling: 20 samples, 0.2 s warmup and 0.5 s requested measurement per case
(Criterion extends slow cases to collect every sample).

## Provenance and limits

The [environment record](decoder-comparison-environment.json) hashes the
measured sources and binary, the [size manifest](decoder-comparison-2026-10-18/sizes.csv)
lists every frame, and the
[raw Criterion samples and estimates](decoder-comparison-2026-10-18/criterion-samples.json.gz)
are archived. The decoder sweep ran concurrently with the 2026-10-18 encoder
sweep on another physical core, sharing the L3 cache. This is one WSL2 host;
frequency, thermal state and scheduling are not controlled, and confidence
bounds do not capture every source of host or allocator variation.

Reproduce from the repository root with a fresh baseline name:

```sh
cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench decoders --locked -- --test
taskset -c 6 cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench decoders --locked -- --save-baseline my-decoders --warm-up-time 0.2 --measurement-time 0.5 --sample-size 20
python3 benchmarks/comparison/report.py --decoding --baseline my-decoders --csv /tmp/my-decoders.csv
cp benchmarks/comparison/target/criterion-decoders/sizes.csv /tmp/my-decoders-sizes.csv
```

Use the [benchmark guide](../benchmarking.md#decoder-comparison) to regenerate
the published pages from the recorded CSV and environment metadata.
