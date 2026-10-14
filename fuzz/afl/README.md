# AFL fuzz targets

This package fuzzes the mzstd encoder and decoder against zstd v1.5.7. It is
excluded from the workspace because its binaries link AFL's runtime; the
target bodies also replay the committed regression corpus through
`cargo afl test`.

## Setup

AFL requires a C compiler and `make`. From `fuzz/afl/`:

```sh
cargo install cargo-afl --version 0.18.2 --locked
cargo afl config --build --force
cargo afl build --release
```

Some hosts need shared memory or crash-reporting changes before the first
campaign; `cargo afl system-config` makes them (it runs `sudo`).

## Layout

| Path | Role |
| --- | --- |
| `src/lib.rs` | `decode_config`: the parameter header shared by every target |
| `src/targets.rs` | Target bodies, shared by AFL and the regression replay |
| `src/bin/` | Thin `afl::fuzz!` adapters |
| `tests/regressions.rs` | Committed corpus replay |
| `regressions/<target>/` | Boundary cases and minimised findings, `*.bin` |
| `prepare-seeds.sh` | Builds `seeds/params` and `seeds/frames` from the vendored zstd test data |
| `campaign.sh` | Runs targets in parallel for a fixed time |
| `seeds/`, `findings/` | Local artifacts, never committed |

## Input layout

`decode_config` reads byte 0 as the level (`value % 40`: `0..=32` map to
`-10..=22`, then `-100`, `-1000`, `-131072`, and `13..=19` again) and byte 1
as flags: `1` checksum, `2` no content size, `4`/`8` LDM disabled or enabled
(one byte of LDM minimum match follows), `0x10` window log (one byte),
`0x20` strategy and minimum match (one byte), `0x40` search, hash and chain
logs (one byte), `0x80` source size hint (two bytes). The rest, capped at
256 KiB, is the payload; some targets take their own bytes from its start.

## Targets

| Target | Extra bytes | Checks |
| --- | --- | --- |
| `differential_c` | none | `compress` equals `ZSTD_compress2` with the same parameters; the frame decodes |
| `output_capacity` | 2: capacity | `compress_to_slice` and `compress_to_uninit` into any capacity return the reference's frame, or fail exactly when `ZSTD_compress2` does |
| `encoder_session` | 4: output size, declared size, chunk seed | A session fed random chunks, flushes and output windows, alternating `process` and `process_uninit`, equals `ZSTD_compressStream2` driven the same way |
| `compressor_lifecycle` | a second header mid-payload | Reuse, `reconfigure` and `trim` never change a frame |
| `parallel` | 1: task count (bits 0-2), stream window (3-4), flush (5), chunk size (6-7 with 0-2) | Parallel frames decode and are identical for 1–8 tasks run in reverse order; a session alternating `process` and `process_uninit` and the writer and reader adapters, with and without a declared size, decode and reproduce the batch frame whenever no flush moved a boundary |
| `parameter_parsing` | none | Header decoding never panics; window logs are accepted exactly in `10..=31` |
| `decompress` | raw input, no header | Arbitrary bytes never panic; whatever mzstd accepts, the reference's streaming decoder accepts with the same output; `decompress_to_slice` agrees with `decompress` |
| `decode_streaming` | 2: input and output piece sizes, then a frame | A session fed those pieces agrees with one-shot decoding, never over-reports progress, and stops only when `NeedsInput` has consumed its input |
| `decode_roundtrip` | parameter header | Frames the reference writes with the fuzzed configuration decode to the payload, one-shot and in pieces |

Decoder targets cap output at 16 MiB, so run-length blocks cannot exhaust
memory. The decoder is stricter than the reference in documented places (see
`architecture/decompressor.md`), so `decompress` checks only that mzstd never
accepts what the reference rejects.

There are no experimental targets yet; the `experimental` feature exists so
both configurations are checked as the target list grows.

## Running

```sh
./prepare-seeds.sh
./campaign.sh 600              # every target, ten minutes each, in parallel
./campaign.sh 60 parallel      # one target
cargo afl test --release       # replay regressions/
```

A crash lands in `findings/<target>/default/crashes/`. Minimise it with
`cargo afl tmin`, commit it under `regressions/<target>/` as `*.bin`, and fix
the bug with the replay failing first. `campaign.sh` stops each instance at
the time limit; a "timeout" saved in the last seconds of a campaign is
usually that stop. On macOS, AFL also occasionally records timeouts while the
host is loaded that do not reproduce. Replay any timeout with
`cargo afl showmap -t 20000 -o /dev/null -- target/release/<target> < input`
before treating it as a hang.
