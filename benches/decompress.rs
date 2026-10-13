//! mzstd's decoder against zstd 1.5.7 on identical frames.
//!
//! Frames are written by the reference at several levels (and with LDM and a
//! 2^27 window for one case), checked to decode to their input through both
//! decoders, then decoded into preallocated buffers with
//! `Decompressor::decompress_to_slice` and `ZSTD_decompressDCtx`, each with a
//! reused context. A streaming group feeds `DecoderSession` 64 KiB of input
//! at a time.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
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

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = one_shot, streaming
}
criterion_main!(benches);
