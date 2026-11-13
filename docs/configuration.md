# Configuration and features

Three things are configured separately: the **encoder** (level and parameters),
the **stream** (what you know about one input), and the **Cargo features**
(which codecs and adapters are compiled in). Decoder settings are covered in
[decompression and limits](decompression.md).

## Choose a level first

`EncoderConfig::default()` uses **level 3**. Levels run from -131072 to 22;
negative levels trade size for speed, and level 0 is an alias for 3.

```rust
use mzstd::{Compressor, EncoderConfig, Level};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = EncoderConfig::default().with_level(Level::try_from(19)?);
    let mut encoder = Compressor::new(config)?;
    let compressed = encoder.compress(b"configured payload")?;
    assert!(!compressed.is_empty());
    Ok(())
}
```

Speed and size depend on your data. The
[benchmark pages](benchmarks/README.md) show both for every measured level.
Serial output is byte-identical to zstd 1.5.7 at every level, so compressed
sizes match the reference.

## Encoder settings

`EncoderConfig` holds settings shared by successive frames:

| Setting | Type / method | Behavior |
| --- | --- | --- |
| Compression level | `Level` / `with_level` | -131072–22; default 3; 0 aliases 3 |
| Advanced compression parameters | `CompressionParams` / `with_params` | Window, hash/chain/search logs, match length, target length, strategy |
| Long distance matching | `LongDistanceMatching` / `with_long_distance_matching` | Automatic, disabled, or explicit `LdmParams` |
| Content checksum | `with_checksum` | Add a checksum to the frame |
| Frame format | `with_format` | Standard or magicless |
| Target compressed block size | `with_target_compressed_block_size` | Optional `NonZeroU32` target, like `ZSTD_c_targetCBlockSize` |

Explicit parameters override the level as `ZSTD_CCtx_setParameter` does.
`Compressor::new` validates parameter combinations. `CParameter` provides a
validated parameter-by-ID interface for integrations; see [OpenZL integration](../architecture/openzl-integration.md).

`StreamConfig` carries an `InputSize`: unknown by default, or
`InputSize::Exact(input.len() as u64)` when the full length is known. Declared
sizes are checked. Session scheduling is part of the byte-identity contract.

Explicit parameters, a checksum and long distance matching on top of a level:

```rust
use mzstd::{CompressionParams, Compressor, EncoderConfig, LdmParams, Level, LongDistanceMatching, Strategy};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = EncoderConfig::default()
        .with_level(Level::try_from(9)?)
        .with_checksum(true)
        .with_params(CompressionParams::default().with_window_log(24).with_strategy(Strategy::Lazy2))
        .with_long_distance_matching(LongDistanceMatching::Enabled(LdmParams::default()));
    let mut encoder = Compressor::new(config)?;
    assert!(!encoder.compress(b"tuned payload")?.is_empty());
    Ok(())
}
```

See [dictionaries and extended formats](dictionaries.md) for dictionaries,
large windows, long distance matching and magicless frames.

## Cargo features

The default feature set is `std`, `compression`, and `decompression`.

| Configuration | Dependency |
| --- | --- |
| Default std encoder + decoder | `mzstd = "0.2.14"` |
| Encoder only | `mzstd = { version = "0.2.14", default-features = false, features = ["std", "compression"] }` |
| Decoder only | `mzstd = { version = "0.2.14", default-features = false, features = ["std", "decompression"] }` |
| Framing containers | `mzstd = { version = "0.2.14", features = ["framed"] }` |
| Alloc-backed codecs without std | `mzstd = { version = "0.2.14", default-features = false, features = ["no_std", "compression", "decompression"] }` |

| Feature | Default | Enables |
| --- | --- | --- |
| `compression` | yes | `mzstd::compressor` |
| `decompression` | yes | `mzstd::decompressor` |
| `std` | yes | `std::error::Error`, `mzstd::io`, parallel compression |
| `no_std` | no | Alloc-only build; overrides `std` and removes std-only I/O and parallel APIs |
| `framed` | no | `mzstd::framing`: seekable containers of resources and metadata. Its API may change in a patch release |
| `hotpath`, `hotpath-cpu`, `hotpath-alloc`, `hotpath-callgrind`, `hotpath-cloud` | no | Profiling anchors; Callgrind instruction counts and hotpath Cloud upload |

See [codec features and no_std support](../architecture/codec-features.md) for
the gates and the checked combinations.

## Not supported

- External sequence producers
- Legacy (v0.x) frames
- Byte identity with zstd's multithreaded mode
- Dictionaries in parallel compression and the C ABI
