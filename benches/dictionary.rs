//! Compression with a dictionary against zstd 1.5.7's `ZSTD_compress2` after
//! `ZSTD_CCtx_loadDictionary`.
//!
//! A dictionary is trained with the reference on JSON-like records; inputs
//! are one record (the dictionary is attached), a hundred and a thousand
//! records (attached or copied, depending on the strategy's cutoff). Both
//! encoders reuse their context and dictionary, so the timed region is the
//! frame alone. Frames are checked for byte identity before timing.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use mzstd::dictionary::DictionaryBuilder;
use mzstd::{Compressor, EncoderConfig, Level};
use zstd_ffi::corpus;
use zstd_ffi::oracle::{self, Dictionary, Params};

const LEVELS: [i32; 5] = [1, 3, 9, 16, 19];

fn compress(c: &mut Criterion) {
    let records = corpus::records(6000, 1);
    let samples: Vec<&[u8]> = records[..4000].iter().map(Vec::as_slice).collect();
    let dictionary = oracle::train_dictionary(&samples, 64 << 10).expect("trains");
    let inputs = [
        ("1-record", records[4000].clone()),
        ("100-records", records[4000..4100].concat()),
        ("1000-records", records[4000..5000].concat()),
    ];
    for (name, input) in &inputs {
        let mut group = c.benchmark_group(format!("dictionary/{name}"));
        group.throughput(Throughput::Bytes(input.len() as u64));
        for level in LEVELS {
            let config =
                EncoderConfig::default().with_level(Level::try_from(level).expect("level"));
            let prepared = DictionaryBuilder::new(&dictionary)
                .with_config(config)
                .build()
                .expect("prepares");
            let mut mzstd = Compressor::new(config).expect("builds");
            let bound = Compressor::max_compressed_size(input.len()).expect("a bound");
            let mut ours = vec![0u8; bound];
            let size = mzstd
                .compress_with_dictionary_to_slice(&prepared, input, &mut ours)
                .expect("compresses");
            let params = Params::level(level).with_dictionary(Dictionary::auto(&dictionary));
            let theirs = oracle::compress2(input, &params).expect("reference");
            assert_eq!(ours[..size], theirs[..], "{name} level {level}");
            let plain = mzstd.compress(input).expect("compresses").len();
            println!(
                "{name} level {level}: {} -> {size} bytes with the dictionary, {plain} without",
                input.len()
            );
            group.bench_function(BenchmarkId::new("mzstd", level), |b| {
                b.iter(|| {
                    mzstd
                        .compress_with_dictionary_to_slice(&prepared, black_box(input), &mut ours)
                        .expect("compresses")
                });
            });
            let mut cctx = oracle::Encoder::new(&params);
            let mut dst = vec![0u8; bound];
            group.bench_function(BenchmarkId::new("zstd", level), |b| {
                b.iter(|| cctx.compress_into(black_box(input), &mut dst));
            });
        }
        group.finish();
    }
}

criterion_group!(benches, compress);
criterion_main!(benches);
