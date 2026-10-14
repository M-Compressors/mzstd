# Test data

| File | Bytes | SHA-256 | Source |
| --- | ---: | --- | --- |
| `alice29.txt` | 152,089 | `7467306ee0feed4971260f3c87421154a05be571d944e9cb021a5713700c38f0` | Lewis Carroll's *Alice's Adventures in Wonderland* (public domain) as distributed in the [Canterbury corpus](https://corpus.canterbury.ac.nz/descriptions/#cantrbry), copied from Google Brotli's `tests/testdata` by way of the sibling mbrotli project. |

`alice29.txt` is the real-world text of the
[implementation comparison](../benchmarks/comparison/Cargo.toml): the `alice29`
dataset as is, and `text-1m` repeated cyclically to 1 MiB. The same bytes back
mbrotli's comparison, so the two projects' results describe one corpus.
