# Dictionaries and extended formats

## Prepared dictionaries

A `PreparedDictionary` owns immutable dictionary bytes and compression indexes.
Prepare it once for the encoder configuration and borrow it across operations
or threads. Both raw content and trained Zstandard-format dictionaries work
with the ordinary codec features; framing is not required.

```rust
use mzstd::dictionary::DictionaryBuilder;
use mzstd::{Compressor, Decompressor, EncoderConfig, Level};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = EncoderConfig::default().with_level(Level::try_from(3)?);
    let dictionary = DictionaryBuilder::new(b"HTTP/1.1 200 OK\r\nContent-Type: ")
        .with_config(config)
        .build()?;
    let input = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n";
    let frame = Compressor::new(config)?.compress_with_dictionary(&dictionary, input)?;
    assert_eq!(
        Decompressor::default().decompress_with_dictionary(&dictionary, &frame)?,
        input
    );
    Ok(())
}
```

`DictionaryContent::Auto` recognizes Zstandard dictionary magic, `Raw` forces
raw content, and `Zstd` requires a trained dictionary including valid entropy
tables. The library consumes trained dictionaries; it does not train them.
A trained dictionary carries a nonzero ID checked against the frame. Raw content
has no ID, so applications must supply the correct bytes themselves.

`DictionaryBuilder::with_config` prepares for matching compressor parameters;
frame checksum/content-size options can differ. Incompatible configurations
return `EncodeError::DictionaryMismatch`. `with_level_parameters(level)` instead
prepares a standalone CDict with `ZSTD_createCDict_advanced` /
`ZSTD_CCtx_refCDict` semantics; its level replaces the context level during
parameter resolution. See [dictionary mechanics](../architecture/dictionaries.md).

## Decoder dictionaries

For a decoder-only application, prepare `DecodeDictionary::new(bytes, content)`.
This avoids encoder search indexes and works with only `decompression` enabled.
Decoder entry points borrow a prepared encoder or decoder dictionary through
`DictionaryRef`. Reuse it for one-shot, slice, session, reader, and writer APIs.

Dictionary IDs identify content; the decoder does not fetch missing dictionaries.
See [decoder dictionaries](../architecture/decompressor.md#dictionaries).

## Large windows and long distance matching

Set a window through `CompressionParams::with_window_log` and enable long
distance matching through `LongDistanceMatching::Enabled(LdmParams::default())`.
These remain standard Zstandard frames. Explicit parameter validation follows
the reference; decoder window limits must permit the resulting declaration.
Larger windows can require more retained history and memory. See
[encoder core](../architecture/encoder-core.md) and [configuration](configuration.md#encoder-settings).

## Magicless frames

`EncoderConfig::with_format(FrameFormat::Magicless)` omits the four-byte
Zstandard magic. Select the same format through `DecoderConfig::with_format`:
it cannot be autodetected. This supports integrations such as OpenZL; it does
not apply to parallel compression, framed containers, or the C ABI.
See [OpenZL integration](../architecture/openzl-integration.md).

## Feature flags

| Feature | Surface |
| --- | --- |
| `compression`, `decompression` | Independently selectable codecs and dictionary preparation |
| `std` | Standard error traits and runtime SIMD support |
| `no_std` | Alloc-backed codecs; removes std-only I/O and parallel APIs |
| `framed` | Structured, seekable Zstandard containers; APIs may change in a patch release |
| `hotpath`, `hotpath-cpu`, `hotpath-alloc`, `hotpath-callgrind`, `hotpath-cloud` | Profiling instrumentation |

See [codec features](../architecture/codec-features.md) for checked combinations.

## Framing containers

With `framed`, resources and compressed metadata can refer to external or earlier
internal dictionaries. The caller supplies dictionary identifiers, references,
and a resolver for external content. Ordinary Zstandard decoding concatenates
visible resources and skips the container's other frames; resource-aware decoding
uses `FramedDecompressor`.

Each compressed chunk is a Zstandard frame. Metadata, hidden resources, directory,
and footer use skippable frames; the last frame is a contrib-compatible seek
table. Optional resource IDs/checksums are caller-supplied and not computed or
verified by the container layer; ordinary Zstandard frame checksums are verified
by the codec. See [framing recipes](frames.md), [wire format](../architecture/framing.md),
and [decoder limits and events](../architecture/framed-decoder.md).
