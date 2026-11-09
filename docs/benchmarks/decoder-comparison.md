# Decompression comparison — 2026-11-09

[Benchmark index](README.md) · [All level pages](decoders/README.md) ·
[Published CSV](decoder-comparison.csv)

The published 384 rows cover two decoders on eight identical C-written
frames at each source level, recorded as `long-20261109` (mzstd 0.2.12, the
working tree on `5ecb52f`, identified by the
[source hashes](decoder-comparison-environment.json), zstd 1.5.7). mzstd's
median speed relative to C is **1.517×** across all 192 dataset/level
pairs; mzstd is faster in 151 of them, and in 144 of those its 95% mean
timing interval lies wholly below C's. Empty and tiny inputs have the same
weight as large inputs. Per-workload results include cases where mzstd is
slower.

The host was loaded by other tenants throughout the run. Sampling was
lengthened to 50 samples of 6 s after a 3 s warmup, which keeps the medians
stable but leaves a median 95% interval of about 5% per case, against 0.5%
in the 2026-10-30 run. Read single cells as ±10%.

![Median decompression speed relative to C](decoders/charts/overview.svg)

Each bar is the median of eight C mean time / decoder mean time ratios, in
the same process. The [decoder overview](decoders/README.md) lists every
source level. Higher is faster; medians do not establish statistical
significance.

| Dataset | mzstd speed / C ↑ | 2026-10-30 | Range across levels |
| --- | ---: | ---: | ---: |
| empty | 53.4× | 48.9× | 47.0–57.2× |
| tiny-text | 39.8× | 38.1× | 36.4–43.6× |
| alice29 | 0.948× | 0.944× | 0.812–1.023× |
| text-1m | 0.937× | 0.965× | 0.842–1.032× |
| binary-64k | 1.453× | 1.440× | 1.148–1.835× |
| random-64k | 3.218× | 3.126× | 2.991–3.631× |
| random-1m | 1.142× | 1.137× | 1.090–1.185× |
| repeated-1m | 1.532× | 1.506× | 1.456–1.696× |

Empty and tiny frames decode tens of times faster because mzstd needs no
preallocated decoding context. Compressible text still trails: all 41
slower pairs are Alice (22 of 24 levels, all but 1 and 20) and cyclic
Alice (19 of 24, all but 5, 11, 13, 15 and 16). At level 1 Alice decodes
at 1,539 MiB/s against C's 1,505 MiB/s; cyclic Alice at 8,939 MiB/s
against 10,016. Most of that gap is the Huffman literal loop
([decoder fast paths](../../architecture/decoder-performance.md#known-gaps)).
Cyclic Alice's median fell from 0.965× to 0.937×, well inside the
0.84–1.03× spread its cases show across levels in this run; this run cannot
tell that change from the host's noise.

This suite times only `decompress`; `decompress_to_slice` and the streaming
sessions are measured by the root `decompress` benchmark, and
[lzbench](lzbench-2026-11-09.md) measures whole-archive throughput on
`silesia.tar`.

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
The host is an AMD Ryzen 9 7950X KVM guest with two vCPUs, its host shared
with other tenants; the decoder suite ran on vCPU 1 after the encoder suite,
with nothing else in the guest. Sampling: 50 samples, 3 s warmup and 6 s
requested measurement per case (Criterion extends slow cases to collect
every sample). Rust 1.99.1, runtime AVX-512 dispatch with the block loops
at the AVX2 level; zstd built by `zstd-ffi` with GCC 15.2 `-O3`, without
its Huffman assembly.

## Provenance and limits

The [environment record](decoder-comparison-environment.json) hashes the
measured sources and binaries, the [size manifest](decoder-comparison-2026-11-09/sizes.csv)
lists every frame, and the raw
[Criterion samples and estimates](decoder-comparison-2026-11-09/criterion-samples.json.gz)
are archived. This is one sweep on one KVM guest; frequency, thermal state
and scheduling are not controlled. Other tenants loaded the host
throughout: a fixed C decode varied by 8-43% between its 10th and 90th
percentile wall times during the day, the median case's 95% interval is
5.2% (90th percentile 9.1%), and C's own timings moved by a median 8%
against the 2026-10-30 run.

Reproduce from the repository root with a fresh baseline name:

```sh
cargo bench --manifest-path benchmarks/comparison/Cargo.toml --bench decoders --locked --no-run
cd benchmarks/comparison
taskset -c 1 target/release/deps/decoders-<hash> --bench --save-baseline my-decoders --sample-size 50 --warm-up-time 3 --measurement-time 6 --noplot
cd ../..
python3 benchmarks/comparison/report.py --decoding --baseline my-decoders --csv /tmp/my-decoders.csv
cp benchmarks/comparison/target/criterion-decoders/sizes.csv /tmp/my-decoders-sizes.csv
```

Use the [benchmark guide](../benchmarking.md#decoder-comparison) to regenerate
the published pages from the recorded CSV and environment metadata.
