# Buffers and memory reuse

Every call involves two kinds of memory: the **destination** you hand to the
codec, and the **workspace** the codec keeps for itself. You control both.

| You want to | Use |
| --- | --- |
| Get a fresh `Vec<u8>` | `compress` / `decompress` |
| Append to a Vec you already own | `compress_into` / `decompress_into` |
| Write into a fixed slice | `compress_to_slice` / `decompress_to_slice` |
| Write into uninitialized memory | `compress_to_uninit` / `decompress_to_uninit` |
| Cap what the codec keeps between calls | A [retention policy](#reusing-memory) |

## Output buffers

`compress` returns a new Vec. `compress_into` appends to a supplied vector and
returns the range containing the new frame. Existing bytes remain intact and
its original length is restored on failure.

`compress_to_slice` returns the number of bytes written. Allocate a conservative
buffer using `Compressor::max_compressed_size`:

```rust
use mzstd::Compressor;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = b"data to compress";
    let mut encoder = Compressor::default();
    let mut output = vec![0; Compressor::max_compressed_size(input.len())?];
    let written = encoder.compress_to_slice(input, &mut output)?;
    output.truncate(written);
    assert_eq!(output, encoder.compress(input)?);
    Ok(())
}
```

An undersized slice returns an error and can contain a partial prefix. Retry
the complete operation with sufficient storage. `compress_to_uninit` and
`decompress_to_uninit` accept caller-owned `MaybeUninit<u8>` destinations;
read only the prefix reported as initialized. See [decoder output contracts](../architecture/decoder-uninit-output.md).

## Reusing memory

Keep a compressor or decompressor across requests to reuse workspace. Caller
output capacity is managed separately. A retention policy controls storage
kept between operations:

| Retention policy | Effect |
| --- | --- |
| `Aggressive` (default) | Keeps allocated buffers |
| `Bounded { max_bytes }` | Releases storage when retained accounting exceeds the ceiling |
| `ReleaseAll` | Releases storage after an operation |

Use `Compressor::builder(config).with_retention(policy).build()` or the decoder
builder, and `trim(policy)` for immediate cleanup. `retained_bytes()` excludes
caller output and allocator overhead. A retention ceiling is not a peak memory
limit. Use a separate codec per simultaneous independent operation, or
[parallel compression](parallel.md) to split one input across workers.
