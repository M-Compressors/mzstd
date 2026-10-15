# Decompression comparison — 2026-10-15 (mzstd 0.2.5)

[Benchmark index](README.md) · [All level pages](decoders/README.md) ·
[Published CSV](decoder-comparison.csv)

The published 384 rows cover two decoders on eight identical C-written
frames at each source level, recorded as `dec-20261015c` (mzstd 0.2.5, the
working tree on `39d1ae4` identified by the
[source hashes](decoder-comparison-environment.json), zstd 1.5.7).
mzstd's median speed relative to C is **1.024×** across all 192
dataset/level pairs; mzstd is faster in 101 of them, and in 97 its 95%
mean timing interval lies wholly below C's. The decoder's code did not
change in this release, so differences from the previous run are run-to-run
variation. Empty and tiny inputs have the same weight as large inputs.
Per-workload results include cases where mzstd is slower.

| Source level | zstd C | mzstd |
| ---: | ---: | ---: |
| -5 | 1.000× | 1.019× |
| -1 | 1.000× | 1.001× |
| 1 | 1.000× | 0.983× |
| 2 | 1.000× | 0.976× |
| 3 | 1.000× | 0.992× |
| 4 | 1.000× | 0.992× |
| 5 | 1.000× | 0.987× |
| 6 | 1.000× | 0.989× |
| 7 | 1.000× | 0.993× |
| 8 | 1.000× | 0.986× |
| 9 | 1.000× | 1.022× |
| 10 | 1.000× | 0.985× |
| 11 | 1.000× | 0.990× |
| 12 | 1.000× | 0.978× |
| 13 | 1.000× | 0.987× |
| 14 | 1.000× | 0.982× |
| 15 | 1.000× | 1.042× |
| 16 | 1.000× | 0.987× |
| 17 | 1.000× | 0.994× |
| 18 | 1.000× | 0.991× |
| 19 | 1.000× | 0.986× |
| 20 | 1.000× | 1.035× |
| 21 | 1.000× | 1.033× |
| 22 | 1.000× | 0.995× |

Each column is the median of eight C mean time / decoder mean time ratios.
The overall value takes the median of all 192 direct ratios, not an average of
level medians. Higher is faster; medians do not establish statistical
significance.

By dataset, the medians across levels are: empty 21.9× and tiny text
18.5× (mzstd needs no preallocated decoding context), random 64 KiB
1.73×, random 1 MiB 1.04×, structured binary 0.94×, Alice
0.93×, cyclic Alice 0.86× and repeated bytes 0.91×. For Alice at
level 3, mzstd decodes 1,348 MiB/s against C's 1,527 MiB/s. On compressible
text the sequence loop executes about 5% more instructions than the
reference's, and `decompress` zeroes its output before decoding compressed
blocks into it, which C, writing into uninitialized memory, does not pay.

Repeated bytes keep the bimodal cold timing measured before: inside one
long-running process their 1 MiB output allocation either reuses mapped
memory or faults fresh pages, depending on the allocator state the process
has reached; in a separate process per case they decoded at about 1.38× C.
This suite times only `decompress`; `decompress_to_slice` and the streaming
sessions are measured by the root `decompress` benchmark.

## Regression checks

Against the previous publication (`dec-20261014-speed`), empty and tiny
frames measured 5-14% lower speed relative to C at every level. Rerun as
separate processes with the previous release's profile driver against this
release's (eleven interleaved pairs on an idle CPU 4), mzstd decoded an
empty frame in 33.7 ns with both and tiny text in 41.6 against 40.5 ns:
the sweep difference is run-to-run variation of a 30-40 ns call. Changes
to the decoder's table builds were measured during this release and
rejected: they moved the inlining of the sequence loop and decoded text
2.5-4% slower in the same paired runs
([decoder fast paths](../../architecture/decoder-performance.md#known-gaps)).

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
measured sources and binary, the [size manifest](decoder-comparison-2026-10-15/sizes.csv)
lists every frame, and the
[raw Criterion samples and estimates](decoder-comparison-2026-10-15/criterion-samples.json.gz)
are archived. The decoder sweep ran concurrently with the 2026-10-15 encoder
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
