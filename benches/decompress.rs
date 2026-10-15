//! mzstd's decoder against zstd 1.5.7 on identical frames.
//!
//! Frames are written by the reference at several levels (and with LDM and a
//! 2^27 window for one case), checked to decode to their input through both
//! decoders, then decoded into preallocated buffers with
//! `Decompressor::decompress_to_slice` and `ZSTD_decompressDCtx`, each with a
//! reused context. A streaming group feeds `DecoderSession` 64 KiB of input
//! at a time. A dictionary group decodes small records compressed with a
//! trained dictionary, each decoder holding the dictionary already parsed,
//! and a small-frame group the same records compressed without one.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use mzstd::dictionary::DecodeDictionary;
use mzstd::{DecodeOperation, DecoderStatus, Decompressor};
use std::hint::black_box;
use zstd_ffi::corpus;
use zstd_ffi::oracle::{self, Params};

fn frames() -> Vec<(String, Vec<u8>, Vec<u8>)> {
    let text = corpus::text(4 << 20, 1);
    let binary = corpus::binary(4 << 20, 2);
    let random = corpus::random(1 << 20, 3);
    let mut out = Vec::new();
    for (name, input) in [
        ("text-4MiB", &text),
        ("binary-4MiB", &binary),
        ("random-1MiB", &random),
    ] {
        for level in [1, 3, 19] {
            let frame = oracle::compress2(input, &Params::level(level)).expect("reference");
            out.push((format!("{name}/level-{level}"), input.clone(), frame));
        }
    }
    let ldm = Params::level(9)
        .with(zstd_ffi::ZSTD_c_enableLongDistanceMatching, 1)
        .with(zstd_ffi::ZSTD_c_windowLog, 27);
    let long = [&text[..], &binary[..], &text[..]].concat();
    let frame = oracle::compress2(&long, &ldm).expect("reference");
    out.push(("text-binary-text-12MiB/ldm".to_owned(), long, frame));
    out
}

fn one_shot(c: &mut Criterion) {
    for (name, input, frame) in frames() {
        let mut ours = Decompressor::default();
        let mut theirs = oracle::Decoder::new();
        let mut output = vec![0u8; input.len()];
        assert_eq!(
            ours.decompress_to_slice(&frame, &mut output)
                .expect("decodes"),
            input.len()
        );
        assert_eq!(output, input, "{name}");
        assert_eq!(theirs.decompress_into(&frame, &mut output), input.len());
        println!("{name}: {} -> {} bytes", input.len(), frame.len());

        let mut group = c.benchmark_group(format!("decompress/{name}"));
        group.throughput(Throughput::Bytes(input.len() as u64));
        group.bench_function(BenchmarkId::from_parameter("mzstd"), |b| {
            b.iter(|| {
                ours.decompress_to_slice(black_box(&frame), &mut output)
                    .expect("decodes")
            });
        });
        group.bench_function(BenchmarkId::from_parameter("zstd"), |b| {
            b.iter(|| theirs.decompress_into(black_box(&frame), &mut output));
        });
        group.finish();
    }
}

/// Feeds `frame` to a session `chunk` bytes at a time, returning the output
/// size.
fn stream(decoder: &mut Decompressor, frame: &[u8], chunk: usize, buffer: &mut [u8]) -> usize {
    let mut session = decoder.start();
    let mut produced = 0;
    for piece in frame.chunks(chunk) {
        let mut piece = piece;
        loop {
            let progress = session
                .process(piece, buffer, DecodeOperation::Process)
                .expect("decodes");
            piece = &piece[progress.consumed..];
            produced += progress.produced;
            match progress.status {
                DecoderStatus::Finished => return produced,
                DecoderStatus::NeedsInput if piece.is_empty() => break,
                DecoderStatus::NeedsInput | DecoderStatus::NeedsOutput => {}
            }
        }
    }
    produced
}

fn streaming(c: &mut Criterion) {
    let input = corpus::mixed(4 << 20, 9);
    let frame = oracle::compress2(&input, &Params::level(3)).expect("reference");
    let mut decoder = Decompressor::default();
    let mut buffer = vec![0u8; 128 << 10];
    assert_eq!(
        stream(&mut decoder, &frame, 64 << 10, &mut buffer),
        input.len()
    );
    let mut group = c.benchmark_group("decompress-stream/mixed-4MiB");
    group.throughput(Throughput::Bytes(input.len() as u64));
    group.bench_function("mzstd", |b| {
        b.iter(|| stream(&mut decoder, black_box(&frame), 64 << 10, &mut buffer));
    });
    group.finish();
}

/// A thousand JSON-like records, each its own frame compressed at level 3
/// with a dictionary trained on similar records.
fn dictionary(c: &mut Criterion) {
    let training = corpus::records(2_000, 1);
    let samples: Vec<&[u8]> = training.iter().map(Vec::as_slice).collect();
    let bytes = oracle::train_dictionary(&samples, 16 << 10).expect("trains");
    let reference = oracle::Dictionary::auto(&bytes);
    let records = corpus::records(1_000, 2);
    let params = Params::level(3).with_dictionary(reference.clone());
    let frames: Vec<Vec<u8>> = records
        .iter()
        .map(|record| oracle::compress2(record, &params).expect("reference"))
        .collect();
    let total: usize = records.iter().map(Vec::len).sum();
    let compressed: usize = frames.iter().map(Vec::len).sum();
    println!("dictionary/records: {total} -> {compressed} bytes in 1000 frames");

    let dictionary = DecodeDictionary::try_from(&bytes[..]).expect("valid");
    let mut ours = Decompressor::default();
    let mut theirs = oracle::Decoder::with_dictionary(&reference).expect("loads");
    let mut output = vec![0u8; 4 << 10];
    for (record, frame) in records.iter().zip(&frames) {
        let written = ours
            .decompress_with_dictionary_to_slice(&dictionary, frame, &mut output)
            .expect("decodes");
        assert_eq!(&output[..written], &record[..]);
        assert_eq!(theirs.decompress_into(frame, &mut output), record.len());
    }
    let mut group = c.benchmark_group("decompress-dictionary/records-1000");
    group.throughput(Throughput::Bytes(total as u64));
    group.bench_function("mzstd", |b| {
        b.iter(|| {
            for frame in &frames {
                ours.decompress_with_dictionary_to_slice(
                    &dictionary,
                    black_box(frame),
                    &mut output,
                )
                .expect("decodes");
            }
        });
    });
    group.bench_function("zstd", |b| {
        b.iter(|| {
            for frame in &frames {
                theirs.decompress_into(black_box(frame), &mut output);
            }
        });
    });
    group.finish();

    // The same records without a dictionary: the per-frame cost that the
    // dictionary does not add.
    let plain: Vec<Vec<u8>> = records
        .iter()
        .map(|record| oracle::compress2(record, &Params::level(3)).expect("reference"))
        .collect();
    let mut theirs = oracle::Decoder::new();
    let mut group = c.benchmark_group("decompress-small/records-1000");
    group.throughput(Throughput::Bytes(total as u64));
    group.bench_function("mzstd", |b| {
        b.iter(|| {
            for frame in &plain {
                ours.decompress_to_slice(black_box(frame), &mut output)
                    .expect("decodes");
            }
        });
    });
    group.bench_function("zstd", |b| {
        b.iter(|| {
            for frame in &plain {
                theirs.decompress_into(black_box(frame), &mut output);
            }
        });
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = one_shot, streaming, dictionary
}
criterion_main!(benches);
