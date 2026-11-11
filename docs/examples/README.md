# Examples

The recipes below include complete programs and round-trip assertions. Examples
use the default `std`, compression, and decompression features unless stated
otherwise. The documentation workflow compiles and runs their Rust code against
the current checkout.

| Task | Recipe | Features / extra dependency |
| --- | --- | --- |
| Compress and decompress bytes | [First round trip](../getting-started.md#compress-and-decompress) | Default |
| Reuse codec and destination memory | [Reuse the workspace](../getting-started.md#reuse-the-workspace-and-output) | Default |
| Write a raw stream incrementally | [Stream through Write](../getting-started.md#stream-through-write) | Default |
| Drive a raw or framed encoder with tiny buffers | [Sans I/O compression](../sans-io-compression.md) | Default; `framed` for frames |
| Decode arriving chunks and resource events | [Sans I/O decompression](../sans-io-decompression.md) | Default; `framed` for frames |
| Read or write raw, parallel, and framed streams | [Read / Write I/O](../io.md) | Default; `framed` for frames |
| Use scoped CPU workers | [Compressing a slice](../parallel.md#compressing-a-slice) | Default |
| Reuse a Rayon thread pool | [Running tasks with Rayon](../parallel.md#running-tasks-with-rayon) | Default + `rayon` |
| Stream through parallel workers | [Reader and writer adapters](../parallel.md#reader-and-writer-adapters) | Default |
| Package compressed and stored resources | [One container, two resources](../frames.md#one-container-two-resources) | `framed` |
| Add resource names and write incrementally | [Write resources and metadata](../frames.md#write-resources-and-metadata) | `framed` |
| Read just the selected resource | [Indexed resource reads](../frames.md#indexed-resource-reads) | `framed` |

## Run from the repository

The runnable user recipes are complete programs embedded in the guides.
To compile and run all of them against this checkout:

```sh
python3 scripts/docs.py test-examples
```

The repository also provides a [profiling driver](../../examples/profile_compressor.rs):

```sh
git submodule update --init --recursive
cargo run --release --example profile_compressor -- testdata/alice29.txt
```

See [benchmarking](../benchmarking.md) for profiling options and the C comparison.

For a parallel batch, all taken tasks must run or be dropped before finalization.
For a streaming adapter, keep workers running while the adapter blocks.
For writers, finish each resource and the enclosing stream or container explicitly.

## Verify the documented recipes

```sh
python3 scripts/docs.py test-examples
```

This creates a temporary Cargo consumer under `target/`, depends on the local
crate with `framed` enabled and on Rayon, and runs each fenced Rust program
from getting started, parallel, frames, Sans I/O compression/decompression,
the I/O guide, user guide, and dictionaries. It does not
exercise the codec with `--all-features`, because that would also enable `no_std`
and remove the std APIs these recipes demonstrate.
