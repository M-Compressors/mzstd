# Test data

| File | Bytes | SHA-256 | Source |
| --- | ---: | --- | --- |
| `alice29.txt` | 152,089 | `7467306ee0feed4971260f3c87421154a05be571d944e9cb021a5713700c38f0` | Lewis Carroll's *Alice's Adventures in Wonderland* (public domain) as distributed in the [Canterbury corpus](https://corpus.canterbury.ac.nz/descriptions/#cantrbry), copied from Google Brotli's `tests/testdata` by way of the sibling mbrotli project. |

| `huffman-stream-ends/fuzz-0da72832.zst` | 3,006 | `747ff5cc4920ae4b0451dae1d28645280fd066ddb67fd4a520a890604c691255` | A fuzz-mutated frame found by a downstream crate's fuzzer, as found. |
| `huffman-stream-ends/fuzz-3c67529f.zst` | 5,151 | `7a2dbd64cc2f257c783ca66e1b52782911e19e754299f35957f08cc934c77670` | The same, another mutation. |
| `huffman-stream-ends/min-0da72832.zst` | 1,522 | `855db1b04ba79471c8a9a7cb9f79eda42ef6e650c458268b137d3ddf5c869e58` | A fuzz find cut down to its literals section and zeroed byte by byte. |
| `huffman-stream-ends/min-0e5e6439.zst` | 4,539 | `27929db05fce5dace05f20ff57661b4f93d08b0f4e23a9ae6cb4e6ccd7cd5cbe` | The same, from another find. |
| `huffman-stream-ends/min-3c67529f.zst` | 1,879 | `3d347d01a16dafc2d73652035117036f14c4f5e9d0ee18d5ee24155c50321fe5` | The same, from another find. |
| `huffman-stream-ends/min-460c45ad.zst` | 3,787 | `97e8a85fedd8b4df7fb819954c3eda92b5b654a401fe70ab0a5f3960ab526fbb` | The same, from another find. |

The `huffman-stream-ends` frames are single compressed blocks without a
checksum whose four Huffman streams are not consumed exactly. zstd 1.5.7
accepts them through its fast four-stream decoder, which checks only the
regenerated length; `tests/decoder_huffman_ends.rs` requires this decoder
to produce the reference's bytes for them.

`alice29.txt` is the real-world text of the
[implementation comparison](../benchmarks/comparison/Cargo.toml): the `alice29`
dataset as is, and `text-1m` repeated cyclically to 1 MiB. The same bytes back
mbrotli's comparison, so the two projects' results describe one corpus.
