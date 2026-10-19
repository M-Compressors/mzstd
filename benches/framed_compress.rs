//! Framed containers against zstd 1.5.7's contrib seekable format.
//!
//! Each case compresses identical bytes at the same level with 64 KiB
//! frames: `FramedCompressor::compress_into` with one reused owner (one
//! resource, so one frame per 64 KiB chunk plus header frames, directory,
//! footer and seek table) and `ZSTD_seekable_compressStream` with a 64 KiB
//! `maxFrameSize` (frames and seek table only). Before timing, the container
//! must decode with `FramedDecompressor`, `ZSTD_decompress` and the contrib
//! seekable reader, and the reference's output with the seekable reader;
//! both sizes are printed. A records group stores 512 small records as one
//! resource each, against one `ZSTD_compress2` frame per record.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use mzstd::framing::{
    FramedCompressor, FramedDecompressor, FramedEncodeConfig, FramedItem, FramedResource,
};
use mzstd::{EncoderConfig, Level};
use std::hint::black_box;
use zstd_ffi::corpus;
use zstd_ffi::oracle::{self, Params};

/// Frame size of both formats.
const CHUNK: u32 = 64 << 10;

fn encoder(level: i32) -> FramedCompressor {
    FramedCompressor::new(FramedEncodeConfig::default().with_encoder_config(
        EncoderConfig::default().with_level(Level::try_from(level).expect("level")),
    ))
    .expect("configuration")
}

fn single_resource(c: &mut Criterion) {
    let inputs = [
        ("text-1MiB", corpus::text(1 << 20, 1)),
        ("binary-1MiB", corpus::binary(1 << 20, 2)),
        ("mixed-1MiB", corpus::mixed(1 << 20, 3)),
        ("text-16KiB", corpus::text(16 << 10, 4)),
    ];
    for (name, input) in &inputs {
        for level in [1, 3, 19] {
            let mut ours = encoder(level);
            let items = [FramedItem::Resource(FramedResource::from(input.as_slice()))];
            let container = ours.compress(items.as_slice().into()).expect("compress");
            let decoded = FramedDecompressor::new(Default::default())
                .expect("configuration")
                .decompress(&container)
                .expect("decodes");
            assert_eq!(&decoded.resources[0].data, input);
            assert_eq!(
                &oracle::decompress(&container, input.len()).expect("plain"),
                input
            );
            assert_eq!(
                &oracle::seekable_decompress(&container, 0, input.len())
                    .expect("seekable")
                    .data,
                input
            );
            let reference = oracle::seekable_compress(input, level, CHUNK).expect("reference");
            assert_eq!(
                &oracle::seekable_decompress(&reference, 0, input.len())
                    .expect("seekable")
                    .data,
                input
            );
            println!(
                "{name}/level-{level}: {} -> mzstd {} bytes, zstd-seekable {} bytes",
                input.len(),
                container.len(),
                reference.len()
            );

            let mut group = c.benchmark_group(format!("framed_compress/{name}/level-{level}"));
            group.throughput(Throughput::Bytes(input.len() as u64));
            let mut output = Vec::with_capacity(container.len());
            group.bench_function(BenchmarkId::from_parameter("mzstd"), |b| {
                b.iter(|| {
                    output.clear();
                    ours.compress_into(black_box(items.as_slice().into()), &mut output)
                        .expect("compress")
                });
            });
            group.bench_function(BenchmarkId::from_parameter("zstd-seekable"), |b| {
                b.iter(|| {
                    oracle::seekable_compress(black_box(input), level, CHUNK).expect("reference")
                });
            });
            group.finish();
        }
    }
}

fn records(c: &mut Criterion) {
    let records = corpus::records(512, 5);
    let total: usize = records.iter().map(Vec::len).sum();
    let items: Vec<_> = records
        .iter()
        .map(|record| FramedItem::Resource(FramedResource::from(record.as_slice())))
        .collect();
    let level = 3;
    let mut ours = encoder(level);
    let container = ours.compress(items.as_slice().into()).expect("compress");
    let decoded = FramedDecompressor::new(Default::default())
        .expect("configuration")
        .decompress(&container)
        .expect("decodes");
    for (resource, record) in decoded.resources.iter().zip(&records) {
        assert_eq!(&resource.data, record);
    }
    let params = Params::level(level);
    let frames: usize = records
        .iter()
        .map(|record| oracle::compress2(record, &params).expect("reference").len())
        .sum();
    println!(
        "records-512/level-{level}: {total} -> mzstd {} bytes, zstd frames {frames} bytes",
        container.len()
    );
    let mut group = c.benchmark_group(format!("framed_compress/records-512/level-{level}"));
    group.throughput(Throughput::Bytes(total as u64));
    let mut output = Vec::with_capacity(container.len());
    group.bench_function(BenchmarkId::from_parameter("mzstd"), |b| {
        b.iter(|| {
            output.clear();
            ours.compress_into(black_box(items.as_slice().into()), &mut output)
                .expect("compress")
        });
    });
    group.bench_function(BenchmarkId::from_parameter("zstd-frames"), |b| {
        b.iter(|| {
            records
                .iter()
                .map(|record| {
                    oracle::compress2(black_box(record), &params)
                        .expect("reference")
                        .len()
                })
                .sum::<usize>()
        });
    });
    group.finish();
}

criterion_group!(benches, single_resource, records);
criterion_main!(benches);
