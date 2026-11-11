# Streaming and completion

Use streaming when the payload does not fit in memory, arrives in pieces, or
must leave before the input ends. There are three shapes, and they drive the
same state machine:

| Shape | You call | The library does | Recipes |
| --- | --- | --- | --- |
| Reader | `read` on the adapter | Pulls source bytes and yields transformed bytes | [Read / Write I/O](io.md) |
| Writer | `write_all`, then `finish` | Pushes transformed bytes into your sink | [Read / Write I/O](io.md) |
| Session | `process` with your own buffers | No I/O; reports consumed and produced counts | Sans I/O [compression](sans-io-compression.md) and [decompression](sans-io-decompression.md) |

Readers and writers require std. Sessions fit blocking code, async runtimes
and event loops.

## Readers and writers

`encoder.reader(source, stream)` pulls plain input and yields compressed bytes.
`encoder.writer(sink, stream)` accepts plain input and delivers compressed bytes.
Decoder adapters take just the source or sink: `decoder.reader(source)` and
`decoder.writer(sink)`. Their frame mode and limits come from `DecoderConfig`.

Read a reader to EOF. Call `finish` on a writer: dropping it performs no
finalization. Encoder `flush` creates an intermediate decoding boundary and can
change output size; decoder `flush` drains output without declaring final input.
`try_finish` supports retrying a sink failure. Consuming `finish` returns the sink
or a `FinishError`; `into_parts` recovers the error and writer, while `into_error`
discards the writer. Reader `into_parts` preserves source read-ahead.

## Sessions

For direct control, `encoder.start(stream)` creates a borrowed `EncoderSession`.
`decoder.start()` creates a `DecoderSession`. Neither performs I/O.

| Operation / status | Meaning |
| --- | --- |
| `Operation::Continue` | Accept input without ending the frame |
| `Operation::Flush` | Make accepted input decodable without ending the frame |
| `Operation::End` | End the frame after this input |
| `EncoderStatus::NeedsInput` | Supply more input or explicit flush/finish |
| `EncoderStatus::NeedsOutput` | Supply output space and retain unconsumed input |
| `EncoderStatus::Finished` | All final output has been delivered |

Advance input by `Progress::consumed` and deliver only `output[..produced]`.
Finish can take several calls. Decoder sessions use `DecodeOperation` and
`DecoderStatus`; use `Finish` when no more compressed input can arrive.
Owned session variants move the codec into a session without a borrowed-owner
lifetime. See [Sans I/O compression](sans-io-compression.md) and
[decompression](sans-io-decompression.md) for complete loops.
