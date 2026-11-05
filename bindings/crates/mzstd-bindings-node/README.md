# mzstd for Node.js

Zstandard (RFC 8878) compression and decompression, implemented in safe Rust
by [mzstd](https://github.com/M-Compressors/mzstd) and loaded as a native
addon.

- Compressed frames are byte-identical to zstd 1.5.7's: one-shot calls match
  `ZSTD_compress2`, streams match `ZSTD_compressStream2` fed the same chunks.
- The decoder reads every frame zstd writes, with an optional output cap for
  untrusted input.
- TypeScript declarations, CommonJS and ES modules are included.

## Installation

```sh
npm install mzstd
```

Prebuilt addons are published for Linux (x86-64 and Arm64, glibc), macOS
(Intel and Apple silicon) and Windows (x86-64); npm installs only the one for
the current platform. Elsewhere, use the WebAssembly build,
[`mzstd-wasm`](https://www.npmjs.com/package/mzstd-wasm), which has the same
API.

## Usage

```js
import { compress, decompress } from "mzstd";

const frame = compress(Buffer.from("hello, hello, hello"), 19);
console.log(decompress(frame).toString());

// Untrusted input: refuse to inflate past 1 MiB.
const data = decompress(frame, 1 << 20);
```

Levels run from `-131072` (fastest) to `22` (strongest); the default, also
selected by `0` or `null`, is `3`. Functions accept any `Uint8Array`
(including `Buffer`) and return a `Buffer`.

### Streams

```js
import { StreamCompressor, StreamDecompressor } from "mzstd";

const encoder = StreamCompressor.new(3);
const parts = [encoder.compress(chunk1), encoder.compress(chunk2)];
parts.push(encoder.flush());  // optional: make everything so far decodable
parts.push(encoder.finish()); // ends the frame; the stream is closed afterwards

const decoder = StreamDecompressor.new();
const out = parts.map((part) => decoder.decompress(part));
out.push(decoder.finish());   // throws if the input stopped inside a frame
```

### Dictionaries

```js
import { compressWithDictionary, decompressWithDictionary, StreamCompressor } from "mzstd";

const frame = compressWithDictionary(payload, dictionary);
const restored = decompressWithDictionary(frame, dictionary);
const encoder = StreamCompressor.withDictionary(dictionary, 9);
```

`dictionary` is a trained zstd dictionary or raw shared content.

Failures throw an `Error` whose message names the cause, such as
`compression level 23 is outside -131072..=22` or
`decompression failed: ...`.

## License

MIT OR Apache-2.0.
