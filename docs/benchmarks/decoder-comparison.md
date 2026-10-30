# Decompression comparison — 2026-10-30

[Benchmark index](README.md) · [All level pages](decoders/README.md) ·
[Published CSV](decoder-comparison.csv)

The published 384 rows cover two decoders on eight identical C-written
frames at each source level, recorded as `full-20261030` (the working tree
on `0a4fd87`, identified by the
[source hashes](decoder-comparison-environment.json), zstd 1.5.7). mzstd's
median speed relative to C is **1.502×** across all 192 dataset/level
pairs; mzstd is faster in 143 of them, and in all 143 its 95% mean timing
interval lies wholly below C's. The main branch, measured in a second
process beside it ([its CSV](decoder-comparison-2026-10-30/main.csv)),
reached 1.400× and 143 pairs. Empty and tiny inputs have the same weight as
large inputs. Per-workload results include cases where mzstd is slower.

![Median decompression speed relative to C](decoders/charts/overview.svg)

Each bar is the median of eight C mean time / decoder mean time ratios,
each tree against the C timings of its own process. The
[decoder overview](decoders/README.md) lists every source level. Higher is
faster; medians do not establish statistical significance.

| Dataset | mzstd speed / C ↑ | mzstd main speed / C | Range across levels (mzstd) |
| --- | ---: | ---: | ---: |
| empty | 48.9× | 48.1× | 46.2–50.0× |
| tiny-text | 38.1× | 36.5× | 36.0–38.6× |
| alice29 | 0.944× | 0.900× | 0.927–0.955× |
| text-1m | 0.965× | 0.913× | 0.936–0.979× |
| binary-64k | 1.440× | 1.342× | 0.986–1.763× |
| random-64k | 3.126× | 3.171× | 2.778–3.517× |
| random-1m | 1.137× | 1.143× | 1.108–1.148× |
| repeated-1m | 1.506× | 1.409× | 1.492–1.619× |

Empty and tiny frames decode tens of times faster because mzstd needs no
preallocated decoding context. Compressible text still trails at every
level: Alice 0.927–0.955× (1,588 MiB/s against C's 1,713 MiB/s at level 1)
and cyclic Alice 0.936–0.979×. Most of that gap is the Huffman literal
loop, which executes about 242 instructions per four-stream round against
the reference's 221
([decoder fast paths](../../architecture/decoder-performance.md#known-gaps)).

This suite times only `decompress`; `decompress_to_slice` and the streaming
sessions are measured by the root `decompress` benchmark, and
[Silesia](silesia.md) measures reused contexts on large files.

## Against the main branch

Separate processes per case: the main branch's `profile` driver against
the working tree's, five seeded AB/BA pairs of 0.1 s per side on one vCPU,
with iteration counts calibrated from this run's CSV (encoder pairs on
vCPU 0 while the decoder pairs ran on vCPU 1). Each value is the median
over pairs of main time / working-tree time; above 1, the working tree is
faster. [Every pair](decoder-comparison-2026-10-30/paired-main.json.gz)
is archived.

| Dataset | Median over levels | Lowest level | Highest level |
| --- | ---: | ---: | ---: |
| empty | 0.987 | 0.967 | 1.000 |
| tiny-text | 1.050 | 1.006 | 1.107 |
| alice29 | 1.055 | 1.038 | 1.084 |
| text-1m | 1.064 | 1.039 | 1.114 |
| binary-64k | 1.068 | 1.056 | 1.093 |
| random-64k | 1.000 | 0.994 | 1.003 |
| random-1m | 0.999 | 0.979 | 1.012 |
| repeated-1m | 1.301 | 1.249 | 1.359 |

Over all 192 cases the median is 1.049: 139 are faster, 116 by more than
2%, and 8 are more than 2% slower, seven of them empty frames (1.3% slower
in the median, at most 3.3%). Alice decodes 3.8-8.4% faster at every
level, cyclic Alice 3.9-11.4%, structured binary 5.6-9.3% and repeated
bytes 25-36%; random data, whose blocks are stored raw, is unchanged.

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
The host is an AMD Ryzen 9 7950X KVM guest with two dedicated vCPUs: the
working tree's process on vCPU 0, the main branch's on vCPU 1. Sampling: 20
samples, 0.5 s warmup and 1 s requested measurement per case (Criterion
extends slow cases to collect every sample). Rust 1.99.1, runtime AVX-512
dispatch with the block loops at the AVX2 level; zstd built by `zstd-ffi`
with GCC 15.2 `-O3`, without its Huffman assembly.

## Provenance and limits

The [environment record](decoder-comparison-environment.json) hashes the
measured sources and binaries, the [size manifest](decoder-comparison-2026-10-30/sizes.csv)
lists every frame, and the raw Criterion samples and estimates of the
[working tree](decoder-comparison-2026-10-30/criterion-samples.json.gz) and
[main](decoder-comparison-2026-10-30/main-criterion-samples.json.gz) are
archived. The two trees' sweeps ran at the same time on the guest's two
vCPUs. This is one KVM guest; frequency, thermal state and scheduling are
not controlled, and confidence bounds do not capture every source of host
or allocator variation.

Reproduce from the repository root with a fresh baseline name:

```sh
cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench decoders --locked --no-run
cd benchmarks/comparison
taskset -c 0 target/release/deps/decoders-<hash> --bench --save-baseline my-decoders --sample-size 20 --warm-up-time 0.5 --measurement-time 1 --noplot
cd ../..
python3 benchmarks/comparison/report.py --decoding --baseline my-decoders --csv /tmp/my-decoders.csv
cp benchmarks/comparison/target/criterion-decoders/sizes.csv /tmp/my-decoders-sizes.csv
```

Use the [benchmark guide](../benchmarking.md#decoder-comparison) to regenerate
the published pages from the recorded CSVs and environment metadata.
