# mzstd for WebAssembly

Zstandard (RFC 8878) compression and decompression, implemented in safe Rust
by [mzstd](https://github.com/M-Compressors/mzstd) and compiled to
WebAssembly, for browsers, bundlers, Node.js and Deno.

- Compressed frames are byte-identical to zstd 1.5.7's: one-shot calls match
  `ZSTD_compress2`, streams match `ZSTD_compressStream2` fed the same chunks.
- The decoder reads every frame zstd writes, with an optional output cap for
  untrusted input.
- No native code: one package for every platform. Where a native addon is
  available, [`mzstd`](https://www.npmjs.com/package/mzstd) has the same API
  and runs faster.

## Installation

```sh
npm install mzstd-wasm
```

The package carries builds for the `web`, `bundler`, `nodejs` and `deno`
targets of wasm-bindgen. Its `exports` map resolves to the Node.js build in
Node.js and to the `web` build under the `browser` condition; the other
builds can be imported by path, for example
`mzstd-wasm/pkg/deno/mzstd_bindings_wasm.js` from Deno.

## Usage

```js
import { compress, decompress, StreamCompressor, StreamDecompressor } from "mzstd-wasm";

const frame = compress(new TextEncoder().encode("hello, hello, hello"), 19);
console.log(new TextDecoder().decode(decompress(frame)));

// Untrusted input: refuse to inflate past 1 MiB.
const data = decompress(frame, 1 << 20);

const encoder = StreamCompressor.new(3);
const parts = [encoder.compress(chunk), encoder.finish()];
const decoder = StreamDecompressor.new();
```

In a browser, the `web` build has to fetch and instantiate its module first:

```js
import init, { compress } from "mzstd-wasm";

await init();
```

Levels run from `-131072` to `22`;
the default, also selected by `0` or `null`, is `3`. Functions take and
return `Uint8Array`; dictionaries work as in the native package
(`compressWithDictionary`, `decompressWithDictionary`,
`StreamCompressor.withDictionary`, `StreamDecompressor.withDictionary`).

## License

MIT OR Apache-2.0.
