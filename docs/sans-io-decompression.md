# Sans I/O decompression

Feed compressed input and supply writable output space as it becomes available.
Sessions own decoding state and retain no caller input/output slices between
calls. `DecodeOperation::Process` allows more input; `Finish` declares the final
input boundary and enables truncation checks.

| Input | Decoder |
| --- | --- |
| Ordinary raw Zstandard | `Decompressor::start` |
| Output of `ParallelCompressor` | The same `Decompressor::start` |
| Framing container · opt-in | `FramedDecompressor::start` |

There is no separate parallel decoder. Parallel compression assembles a standard
raw stream, decoded through the ordinary decoder APIs. For transport adapters,
see [Read / Write I/O](io.md).

## Decode a raw stream in chunks

This example supplies three compressed bytes at a time, uses a tiny output
buffer, and declares `Finish` only for the last input chunk.

```rust
use mzstd::{Compressor, DecodeOperation, DecoderStatus, Decompressor, EncoderConfig, Level};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = b"incremental decoding preserves every byte\n".repeat(100);
    let mut encoder = Compressor::new(EncoderConfig::default().with_level(Level::try_from(3)?))?;
    let compressed = encoder.compress(&input)?;
    let mut decoder = Decompressor::default();
    let mut session = decoder.start();
    let mut chunks = compressed.chunks(3).peekable();
    let mut output = [0; 11];
    let mut restored = Vec::new();
    let mut finished = false;
    while let Some(chunk) = chunks.next() {
        let operation = if chunks.peek().is_none() {
            DecodeOperation::Finish
        } else {
            DecodeOperation::Process
        };
        let mut remaining = chunk;
        loop {
            let progress = session.process(remaining, &mut output, operation)?;
            remaining = &remaining[progress.consumed..];
            restored.extend_from_slice(&output[..progress.produced]);
            match progress.status {
                DecoderStatus::NeedsInput => {
                    assert!(remaining.is_empty());
                    break;
                }
                DecoderStatus::NeedsOutput => {}
                DecoderStatus::Finished => {
                    assert!(remaining.is_empty());
                    finished = true;
                    break;
                }
            }
        }
        if finished {
            break;
        }
    }
    assert!(finished);
    assert_eq!(restored, input);
    Ok(())
}
```

In an application where EOF is learned from a later read, offer chunks with
`Process` and then call `process(&[], output, Finish)` until `Finished`.
After the first `Finish`, continue with `Finish` and exactly the remaining
input suffix. Do not add new bytes or switch back to `Process`.

`NeedsOutput` can occur after every input byte has been consumed: deliver the
output and call again with an empty input slice and fresh output space.
The default `FrameMode::Concatenated` waits for final input. For protocols
with bytes after one frame, select `FrameMode::Single` and preserve unconsumed
input when `Finished` arrives. The example asserts an empty suffix because its input
contains exactly one complete member.

## Decode a framing container as events

With `framed`, a framed session reports resource boundaries, metadata,
payload fragments, directory entries, and a validated footer. Handle events
before mutating the session again: events can borrow the session and output.

```rust
use mzstd::DecodeOperation;
use mzstd::framing::{
    FramedCompressor, FramedDecoderStatus, FramedDecompressor, FramedEvent, FramedInput,
    FramedItem, FramedResource,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = b"resource decoded one fragment at a time";
    let items = [FramedItem::Resource(FramedResource::from(input.as_slice()))];
    let mut encoder = FramedCompressor::new(Default::default())?;
    let container = encoder.compress(FramedInput::from(items.as_slice()))?;
    let mut decoder = FramedDecompressor::new(Default::default())?;
    let mut session = decoder.start(Default::default())?;
    let mut chunks = container.chunks(3).peekable();
    let mut buffer = [0; 5];
    let mut restored = Vec::new();
    let mut resources = 0;
    let mut finished = false;
    while let Some(chunk) = chunks.next() {
        let operation = if chunks.peek().is_none() {
            DecodeOperation::Finish
        } else {
            DecodeOperation::Process
        };
        let mut remaining = chunk;
        loop {
            let progress = session.process(remaining, &mut buffer, operation)?;
            remaining = &remaining[progress.consumed..];
            match progress.status {
                FramedDecoderStatus::Event(FramedEvent::ResourceStart(_)) => resources += 1,
                FramedDecoderStatus::Event(FramedEvent::ResourceData(data)) => {
                    restored.extend_from_slice(data.bytes);
                }
                FramedDecoderStatus::NeedsInput => {
                    assert!(remaining.is_empty());
                    break;
                }
                FramedDecoderStatus::Finished => {
                    assert!(remaining.is_empty());
                    finished = true;
                    break;
                }
                _ => {}
            }
        }
        if finished {
            break;
        }
    }
    assert!(finished);
    assert_eq!(resources, 1);
    assert_eq!(restored, input);
    Ok(())
}
```

`ResourceDataEnd` ends one resource's payload; it does not validate the rest of
the container. Continue through `Finished` to validate remaining structure.
The recipe has one resource; for multiple resources, route `ResourceData` to
the destination selected by `ResourceStart` rather than concatenating payloads.
See [framed event semantics](../architecture/framed-decoder.md).

## Limits and errors

Set input, output, and window limits in `DecoderConfig` before
starting a raw decoder. For framing, use `FramedDecodeConfig` and its framing
limits in addition to raw decoder limits. See [resource-limit configuration](README.md#decompression).

Output delivered before a later error is partial and has not passed complete
validation. Failure values carry consumed/produced counts for that failing
call; account for them if your transport needs exact progress. Drop a failed
session before starting a new operation. `Finish` reports truncated streams;
an empty `Process` call does not declare EOF.
