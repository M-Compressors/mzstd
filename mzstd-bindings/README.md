# mzstd-bindings

The binding-friendly facade over [`mzstd`](../README.md) that
[alef](https://github.com/xberg-io/alef) turns into the
[`mzstd` Python package](../bindings/packages/python/README.md), the
[`mzstd` Node.js package](../bindings/crates/mzstd-bindings-node/README.md) and
the [`mzstd-wasm` package](../bindings/crates/mzstd-bindings-wasm/README.md).

It exposes only what crosses a language boundary cleanly: byte slices in,
owned byte vectors out, optional integers, and two stateful stream types.

| Item | Purpose |
| --- | --- |
| `compress`, `compress_with_dictionary` | one frame, byte-identical to `ZSTD_compress2` |
| `decompress`, `decompress_with_dictionary` | every frame, with an optional output cap |
| `compress_bound` | `ZSTD_compressBound` |
| `StreamCompressor` | `compress` / `flush` / `finish`, byte-identical to `ZSTD_compressStream2` |
| `StreamDecompressor` | `decompress` / `finish` |
| `MzstdError` | the one error type each binding maps to its exceptions |

It is not published: Rust callers use `mzstd`, which offers the same codecs
with borrowed output, reusable contexts and every parameter. See
[`architecture/bindings.md`](../architecture/bindings.md) for how the
packages are generated, tested and released.
