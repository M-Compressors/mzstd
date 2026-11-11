# Compatibility and validation

This page describes the contracts checked by the repository. Tests, fuzzing,
coverage, and memory tools provide bounded evidence. Lean proofs cover selected
format primitives, rather than the entire codec. For commands, see
[development](development.md).

## What is claimed

| Surface | Claim | Oracle |
| --- | --- | --- |
| Serial one-shot encoding, levels -131072–22 | Byte identity for equivalent input, parameters, and dictionary | zstd 1.5.7 `ZSTD_compress2` |
| Serial sessions | Byte identity with the same chunks, capacities, flushes, parameters, and pledged size | `ZSTD_compressStream2` |
| Decoding | Standard Zstandard frames, concatenated/skippable frames, raw/trained dictionaries, checksums, and configured limits | Independent Rust decoder, C differential tests, upstream golden vectors |
| Parallel encoding | Valid deterministic frames for fixed input/configuration/segment size | Rust API comparisons and reference decoding; no ZSTDMT byte identity |
| Framing | Resources, metadata, directory, footer, seek table, and indexed reads | Round trips, independent wire fixtures, zstd regular and contrib seekable decoders |
| SIMD selection and output destinations | Equivalent observable results | Baseline/backend differential and output-capacity tests |

The development oracle is the vendored submodule at `zstd-ffi/vendor/zstd`.
It is not a production dependency of the main crate. The library forbids
`unsafe` in production code; SIMD and uninitialized-output dependencies still
need memory-tool checks. The separate `mzstd-ffi` crate provides a C ABI.

## Validation layers

- Encoder differential tests cover levels, random advanced parameters, long
  distance matching, dictionaries, short destinations, pledged sizes, target
  compressed block sizes, and streaming schedules with flushes and window wrap.
- The overflow-correction build forces index rescaling often on both sides.
- Decoder tests cover reference frames, golden vectors, truncation, corruption,
  limits, checksums, and all public input/output shapes.
- AFL++ and ClusterFuzzLite exercise byte-consuming and stateful boundaries;
  minimized findings become committed regressions.
- Miri and AddressSanitizer exercise memory-sensitive paths, including SIMD
  and uninitialized destinations. Function coverage is checked separately.
- Lean 4 compares selected translated Rust functions with a transcription of
  RFC 8878. See [formal verification](../architecture/formal-verification.md)
  for the trusted base and coverage checklist.

## Scheduling and partial output

Serial session byte identity requires equivalent C and Rust call schedules.
A different chunk or flush schedule can produce different valid bytes.
Parallel output is not a serial or multithreaded C byte oracle. Performance
comparisons must record size as well as speed and validate decoded bytes first.

Streaming output delivered before an error is partial and may not have passed
checksum or structural validation. Account for a failure's consumed/produced
counts and do not replay input already accepted by a writer. Complete readers
to EOF and explicitly finish writers and sessions.

## Scope and known gaps

The codecs do not support legacy zstd v0.x frames or external sequence
producers. Parallel compression and the C ABI do not accept dictionaries;
magicless framing is limited to ordinary codec APIs. Container resource checksum
fields are recorded without verification. See the current
[architecture gaps](../architecture/README.md#known-gaps),
[encoder verification](../architecture/encoder-verification.md),
[decoder verification](../architecture/decoder-verification.md), and
[CI checks](../architecture/ci.md).
