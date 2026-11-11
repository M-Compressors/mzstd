# Decompression and limits

`Decompressor` owns reusable state. `decompress` returns a Vec,
`decompress_into` appends a result, and `decompress_to_slice` writes into caller
storage. Sessions report consumed/produced counts; `reader` and `writer` adapt
synchronous I/O. Construction is infallible: `Decompressor::new(config)`.

## Set limits for untrusted input

The default accepts concatenated Zstandard frames and skips skippable frames.
Content checksums are always verified. The default window limit is 128 MiB;
input and output byte budgets are unlimited. Set limits for your application:

```rust
use mzstd::{DecodeLimits, DecoderConfig, Decompressor};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let limits = DecodeLimits::default()
        .with_max_window_log(23)?
        .with_max_input_bytes(Some(1 << 20))
        .with_max_output_bytes(Some(8 << 20));
    let mut decoder = Decompressor::new(DecoderConfig::default().with_limits(limits));
    let frame = mzstd::Compressor::default().compress(b"bounded payload")?;
    assert_eq!(decoder.decompress(&frame)?, b"bounded payload");
    Ok(())
}
```

The window log accepts 10–31. Input/output budgets count the whole operation,
including concatenated frames; they are not process-memory ceilings.
`FrameMode::Single` stops a session after one Zstandard or skippable frame.
One-shot decoding rejects trailing bytes in that mode. `FrameHeaderInfo` inspects
headers without decoding the payload. See [decoder mechanics](../architecture/decompressor.md).

## Where to go next

| Task | Read |
| --- | --- |
| Decode chunks as they arrive | [Sans I/O decompression](sans-io-decompression.md) |
| Decode from a `Read` source or into a `Write` sink | [Read / Write I/O](io.md) |
| Decode a framing container | [Framing containers](frames.md) |
| Attach a dictionary | [Dictionaries and extended formats](dictionaries.md#decoder-dictionaries) |
