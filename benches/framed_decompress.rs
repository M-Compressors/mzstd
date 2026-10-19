//! Decoding framed containers against zstd 1.5.7's decoders.
//!
//! One container per input, written by `FramedCompressor` at level 3 with
//! 64 KiB chunks (one resource), is checked to decode to its input through
//! every path below, then decoded with: `FramedDecompressor::decompress_into`
//! into a reused vector (full validation: chunk headers, directory, footer
//! and seek table); `FramedSeekReader` reading the resource through a
//! `Cursor` (index opened once, outside the timing); `ZSTD_decompressDCtx`
//! of the whole container with a reused context (skips the skippable
//! frames, validates nothing of the container); and the contrib seekable
//! reader (`ZSTD_seekable_initBuff`, then `ZSTD_seekable_decompress` of the
//! full range, including its allocation of the output).

/// The seek reader needs `std`.
#[cfg(not(feature = "no_std"))]
mod framed {
    use criterion::{BenchmarkId, Criterion, Throughput};
    use mzstd::framing::{
        FramedCompressor, FramedDecompressor, FramedItem, FramedResource, ResourceIndex,
    };
    use std::hint::black_box;
    use std::io::{Cursor, Read};
    use zstd_ffi::corpus;
    use zstd_ffi::oracle;

    fn containers() -> Vec<(&'static str, Vec<u8>, Vec<u8>)> {
        let mut encoder = FramedCompressor::new(Default::default()).expect("configuration");
        [
            ("text-1MiB", corpus::text(1 << 20, 1)),
            ("binary-1MiB", corpus::binary(1 << 20, 2)),
            ("mixed-1MiB", corpus::mixed(1 << 20, 3)),
            ("random-256KiB", corpus::random(256 << 10, 4)),
        ]
        .into_iter()
        .map(|(name, input)| {
            let items = [FramedItem::Resource(FramedResource::from(input.as_slice()))];
            let container = encoder.compress(items.as_slice().into()).expect("compress");
            (name, input, container)
        })
        .collect()
    }

    pub fn decompress(c: &mut Criterion) {
        for (name, input, container) in containers() {
            let mut ours = FramedDecompressor::new(Default::default()).expect("configuration");
            let mut output = Vec::with_capacity(input.len());
            let decoded = ours
                .decompress_into(&container, &mut output)
                .expect("decodes");
            assert_eq!(output[decoded.resources[0].data.clone()], input[..]);
            let mut theirs = oracle::Decoder::new();
            let mut plain = vec![0u8; input.len()];
            assert_eq!(theirs.decompress_into(&container, &mut plain), input.len());
            assert_eq!(plain, input);
            let seekable =
                oracle::seekable_decompress(&container, 0, input.len()).expect("seekable");
            assert_eq!(seekable.data, input);
            let mut seek = FramedDecompressor::new(Default::default()).expect("configuration");
            let mut reader = seek
                .framed_seek_reader(Cursor::new(container.as_slice()))
                .expect("index");
            let mut indexed = Vec::with_capacity(input.len());
            reader
                .resource(ResourceIndex(0))
                .expect("resource")
                .read_to_end(&mut indexed)
                .expect("reads");
            assert_eq!(indexed, input);
            println!("{name}: {} -> {} bytes", input.len(), container.len());

            let mut group = c.benchmark_group(format!("framed_decompress/{name}"));
            group.throughput(Throughput::Bytes(input.len() as u64));
            group.bench_function(BenchmarkId::from_parameter("mzstd"), |b| {
                b.iter(|| {
                    output.clear();
                    ours.decompress_into(black_box(&container), &mut output)
                        .expect("decodes")
                });
            });
            group.bench_function(BenchmarkId::from_parameter("mzstd-seek"), |b| {
                b.iter(|| {
                    indexed.clear();
                    reader
                        .resource(ResourceIndex(0))
                        .expect("resource")
                        .read_to_end(&mut indexed)
                        .expect("reads")
                });
            });
            group.bench_function(BenchmarkId::from_parameter("zstd"), |b| {
                b.iter(|| theirs.decompress_into(black_box(&container), &mut plain));
            });
            group.bench_function(BenchmarkId::from_parameter("zstd-seekable"), |b| {
                b.iter(|| {
                    oracle::seekable_decompress(black_box(&container), 0, input.len())
                        .expect("seekable")
                });
            });
            group.finish();
        }
    }
}

#[cfg(not(feature = "no_std"))]
criterion::criterion_group!(benches, framed::decompress);
#[cfg(not(feature = "no_std"))]
criterion::criterion_main!(benches);

/// The seek reader needs `std`.
#[cfg(feature = "no_std")]
fn main() {}
