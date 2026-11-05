# mzstd for Python

Zstandard (RFC 8878) compression and decompression, implemented in safe Rust
by [mzstd](https://github.com/M-Compressors/mzstd).

- Compressed frames are byte-identical to zstd 1.5.7's: one-shot calls match
  `ZSTD_compress2`, streams match `ZSTD_compressStream2` fed the same chunks.
- The decoder reads every frame zstd writes, with an optional output cap for
  untrusted input.
- One-shot calls release the GIL, so threads compress in parallel.
- One `abi3` wheel per platform covers Python 3.11 and newer.

## Installation

```sh
pip install mzstd
```

Wheels are published for Linux (x86-64 and Arm64, glibc), macOS (Intel and
Apple silicon) and Windows (x86-64).

## Usage

```python
import mzstd

frame = mzstd.compress(b"hello, hello, hello", level=19)
assert mzstd.decompress(frame) == b"hello, hello, hello"

# Untrusted input: refuse to inflate past 1 MiB.
data = mzstd.decompress(frame, max_output_size=1 << 20)
```

Levels run from `-131072` (fastest) to `22` (strongest); the default, also
selected by `0`, is `3`.

### Streams

```python
encoder = mzstd.StreamCompressor.new(level=3)
frame = encoder.compress(b"first part, ")
frame += encoder.compress(b"second part")
frame += encoder.flush()   # optional: make everything so far decodable
frame += encoder.finish()  # ends the frame; the stream is closed afterwards

decoder = mzstd.StreamDecompressor.new(max_output_size=None)
data = decoder.decompress(frame[:10]) + decoder.decompress(frame[10:])
data += decoder.finish()   # raises if the input stopped inside a frame
```

### Dictionaries

```python
dictionary = b"..."  # a trained zstd dictionary, or raw shared content
frame = mzstd.compress_with_dictionary(b"payload", dictionary)
assert mzstd.decompress_with_dictionary(frame, dictionary) == b"payload"

encoder = mzstd.StreamCompressor.with_dictionary(dictionary, level=9)
decoder = mzstd.StreamDecompressor.with_dictionary(dictionary)
```

### Errors

Every failure raises a subclass of `mzstd.MzstdError`:
`InvalidLevelError`, `CompressionError`, `DecompressionError`,
`DictionaryError`, `InputTooLargeError` and `StreamClosedError`.

## License

MIT OR Apache-2.0.
