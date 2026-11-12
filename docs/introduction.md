# Zstandard compression and decompression in safe Rust

<div class="landing">
<p class="lede">mzstd is a port of the zstd 1.5.7 encoder that writes the same bytes, plus an independent Rust decoder. Reuse working memory between payloads, plug into <code>Read</code> and <code>Write</code>, drive either codec with your own buffers, and schedule parallel compression on your own threads.</p>
<div class="actions">
<a class="primary" href="getting-started.md">Get started</a>
<a href="README.md">Choose your API</a>
<a href="https://docs.rs/mzstd">API reference on docs.rs</a>
</div>
</div>

## A round trip

```toml
[dependencies]
mzstd = "0.2.14"
```

```rust
use mzstd::{Compressor, Decompressor, EncoderConfig, Level};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = "hello, smaller world! ".repeat(1000);
    let mut encoder = Compressor::new(EncoderConfig::default().with_level(Level::try_from(3)?))?;
    let compressed = encoder.compress(input.as_bytes())?;

    let mut decoder = Decompressor::default();
    assert_eq!(decoder.decompress(&compressed)?, input.as_bytes());
    Ok(())
}
```

Requires Rust **1.89 or later**. [Getting started](getting-started.md) continues
from here with memory reuse, streaming and feature selection.

## Pick a level

The level trades speed for size; the default is **3**. Each bar is the
compressed size of the 211 MB Silesia corpus against its original size.

<div class="ladder">
<table>
<thead>
<tr><th>Level</th><th>Compressed size</th><th class="num">Compress</th><th class="num wide">Decompress</th></tr>
</thead>
<tbody>
<tr><td>-5</td><td class="size"><span class="track"><span class="fill" style="width: 48.61%">48.6%</span></span></td><td class="num">886 MB/s</td><td class="num wide">3062 MB/s</td></tr>
<tr><td>1</td><td class="size"><span class="track"><span class="fill" style="width: 34.53%">34.5%</span></span></td><td class="num">640 MB/s</td><td class="num wide">2039 MB/s</td></tr>
<tr><td>3</td><td class="size"><span class="track"><span class="fill" style="width: 31.20%">31.2%</span></span></td><td class="num">379 MB/s</td><td class="num wide">1812 MB/s</td></tr>
<tr><td>8</td><td class="size"><span class="track"><span class="fill" style="width: 28.17%">28.2%</span></span></td><td class="num">102 MB/s</td><td class="num wide">1941 MB/s</td></tr>
<tr><td>15</td><td class="size"><span class="track"><span class="fill" style="width: 26.83%">26.8%</span></span></td><td class="num">8.71 MB/s</td><td class="num wide">2024 MB/s</td></tr>
<tr><td>22</td><td class="size"><span class="track"><span class="fill" style="width: 24.67%">24.7%</span></span></td><td class="num">2.44 MB/s</td><td class="num wide">1524 MB/s</td></tr>
</tbody>
</table>
</div>

<p class="note">mzstd 0.2.14 in lzbench 2.4.1 on one thread of an AMD Ryzen 9 7950X VDS, fastest of three rounds. Your data and machine will differ.</p>

See the [lzbench run](benchmarks/lzbench-2026-11-12.md) for the command and raw
output, and [benchmark results](benchmarks/README.md) for every measured level
against zstd 1.5.7, including the cases where mzstd is slower.

## Find your path

<div class="paths">
<a href="getting-started.md"><strong>First round trip</strong><span>Install, compress, decompress and reuse a codec.</span></a>
<a href="README.md"><strong>Choose your API</strong><span>Every entry point for both codecs in one table.</span></a>
<a href="configuration.md"><strong>Configuration and features</strong><span>Level, parameters, Cargo features and <code>no_std</code>.</span></a>
<a href="decompression.md"><strong>Decode untrusted input</strong><span>Window, input and output limits.</span></a>
<a href="streaming.md"><strong>Streaming</strong><span>Readers, writers, sans-I/O sessions and <code>finish</code>.</span></a>
<a href="parallel.md"><strong>Parallel compression</strong><span>One input across scoped threads, Rayon or your executor.</span></a>
<a href="dictionaries.md"><strong>Dictionaries</strong><span>Raw and trained dictionaries for small inputs.</span></a>
<a href="frames.md"><strong>Framing containers <span class="tag">opt-in</span></strong><span>Several resources and metadata in one seekable container.</span></a>
<a href="../architecture/README.md"><strong>How it works</strong><span>Ownership, dispatch and state machines, with diagrams.</span></a>
</div>

## What you can rely on

- **Byte-identical encoder.** For the same input and parameters, serial
  compression writes exactly the bytes of zstd 1.5.7. See
  [what is claimed and how it is checked](correctness.md).
- **Safe implementation, not a C wrapper.** Production code forbids `unsafe`;
  SIMD goes through `fearless_simd`. The reference C library is only a test
  and benchmark oracle.
- **No hidden I/O or threads.** Sessions touch only the buffers you pass in,
  and parallel compression hands tasks to your scheduler. The library does not
  create a thread pool.
- **Reusable state.** A codec keeps its workspace between payloads, under a
  [retention policy](memory.md#reusing-memory) you choose.

Framing requires the opt-in `framed` feature; its API may change in a patch
release.
