//! mzstd's encoder against zstd 1.5.7 through equivalent calls.
//!
//! Each case compresses the same bytes at the same level with
//! `Compressor::compress_to_slice` and `ZSTD_compress2` into preallocated
//! buffers, after checking that both write identical frames, so every speed
//! figure is read against an identical ratio (printed before timing). A
//! streaming group drives `EncoderSession` and `ZSTD_compressStream2` with the
//! same 64 KiB chunks. The uninitialized-output group drives sessions into
//! page-sized and block-sized outputs through `process` and `process_uninit`,
//! as a web server's response filter does, and times `compress_to_uninit`.
//!
//! Inputs: zstd's own compression regression files (text-like HTTP headers,
//! long literal and match lengths), synthetic text and structured binary,
//! random bytes, and a 64-byte prefix that isolates per-call cost.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use mzstd::{Compressor, EncoderConfig, EncoderStatus, InputSize, Level, Operation};
use std::hint::black_box;
use std::mem::MaybeUninit;
use std::path::Path;
use zstd_ffi::corpus;
use zstd_ffi::oracle::{self, Directive, Params};

/// Levels timed: accelerated, fast, default, lazy, and two optimal ones.
const LEVELS: [i32; 6] = [-5, 1, 3, 9, 16, 19];

fn corpora() -> Vec<(&'static str, Vec<u8>)> {
    let golden =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("zstd-ffi/vendor/zstd/tests/golden-compression");
    let read = |name: &str| {
        std::fs::read(golden.join(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
    };
    let text = corpus::text(1 << 20, 1);
    vec![
        ("tiny-64B", text[..64].to_vec()),
        ("text-1MiB", text),
        ("binary-1MiB", corpus::binary(1 << 20, 2)),
        ("random-256KiB", corpus::random(256 << 10, 3)),
        (
            "large-literal-and-match-lengths",
            read("large-literal-and-match-lengths"),
        ),
        ("http", read("http")),
    ]
}

fn encoder(level: i32) -> Compressor {
    let level = Level::try_from(level).expect("a valid level");
    Compressor::new(EncoderConfig::default().with_level(level)).expect("a valid configuration")
}

/// Drives a session over `input` in `chunk`-sized pieces into `out`.
fn stream(encoder: &mut Compressor, input: &[u8], chunk: usize, out: &mut Vec<u8>) {
    out.clear();
    let mut buffer = vec![0u8; 1 << 17];
    let mut session = encoder.start(InputSize::Unknown.into());
    for piece in input.chunks(chunk) {
        let mut piece = piece;
        while !piece.is_empty() {
            let progress = session
                .process(piece, &mut buffer, Operation::Continue)
                .expect("compresses");
            piece = &piece[progress.consumed..];
            out.extend_from_slice(&buffer[..progress.produced]);
        }
    }
    loop {
        let progress = session
            .process(&[], &mut buffer, Operation::End)
            .expect("ends");
        out.extend_from_slice(&buffer[..progress.produced]);
        if progress.status == EncoderStatus::Finished {
            break;
        }
    }
}

fn compress(c: &mut Criterion) {
    for (name, input) in corpora() {
        let bound = Compressor::max_compressed_size(input.len()).expect("a bound");
        let mut ours = vec![0u8; bound];
        for level in LEVELS {
            let params = Params::level(level);
            let mut mzstd = encoder(level);
            let size = mzstd
                .compress_to_slice(&input, &mut ours)
                .expect("compresses");
            let theirs = oracle::compress2(&input, &params).expect("reference");
            assert_eq!(ours[..size], theirs[..], "{name} level {level}");
            assert_eq!(
                oracle::decompress(&theirs, input.len()).expect("decodes"),
                input
            );
            println!(
                "{name} level {level}: {} -> {size} bytes ({:.4})",
                input.len(),
                size as f64 / input.len() as f64
            );

            let mut group = c.benchmark_group(format!("compress/{name}"));
            group.throughput(Throughput::Bytes(input.len() as u64));
            group.bench_function(BenchmarkId::new("mzstd", level), |b| {
                b.iter(|| {
                    mzstd
                        .compress_to_slice(black_box(&input), &mut ours)
                        .expect("compresses")
                });
            });
            let mut cctx = oracle::Encoder::new(&params);
            group.bench_function(BenchmarkId::new("zstd", level), |b| {
                b.iter(|| cctx.compress_into(black_box(&input), &mut ours));
            });
            group.finish();
        }
    }
}

fn streaming(c: &mut Criterion) {
    let input = corpus::mixed(4 << 20, 9);
    let chunk = 64 << 10;
    let steps: Vec<(&[u8], Directive)> = input
        .chunks(chunk)
        .map(|piece| (piece, Directive::Continue))
        .chain([(&[][..], Directive::End)])
        .collect();
    let mut out = Vec::with_capacity(input.len());
    let mut group = c.benchmark_group("stream/mixed-4MiB");
    group.throughput(Throughput::Bytes(input.len() as u64));
    for level in [1, 3, 9] {
        let mut mzstd = encoder(level);
        stream(&mut mzstd, &input, chunk, &mut out);
        let theirs = oracle::compress_stream(&Params::level(level), &steps).expect("reference");
        assert_eq!(out, theirs, "level {level}");
        group.bench_function(BenchmarkId::new("mzstd", level), |b| {
            b.iter(|| stream(&mut mzstd, black_box(&input), chunk, &mut out));
        });
        group.bench_function(BenchmarkId::new("zstd", level), |b| {
            b.iter(|| oracle::compress_stream(&Params::level(level), black_box(&steps)));
        });
    }
    group.finish();
}

/// Drives a session over `input` in `chunk`-sized pieces, each output call
/// into `buffer`, through `process` or `process_uninit`.
fn stream_into(
    encoder: &mut Compressor,
    input: &[u8],
    chunk: usize,
    buffer: &mut [MaybeUninit<u8>],
    uninit: bool,
    out: &mut Vec<u8>,
) {
    out.clear();
    let mut session = encoder.start(InputSize::Unknown.into());
    let pieces = input
        .chunks(chunk)
        .map(|piece| (piece, Operation::Continue))
        .chain([(&[][..], Operation::End)]);
    for (mut piece, operation) in pieces {
        loop {
            let progress = if uninit {
                session.process_uninit(piece, buffer, operation)
            } else {
                // The initialized case: a buffer zeroed once, as a caller
                // that recycles its output buffers would keep it.
                let buffer = initialized_mut(buffer);
                session.process(piece, buffer, operation)
            }
            .expect("compresses");
            piece = &piece[progress.consumed..];
            out.extend_from_slice(initialized_mut(&mut buffer[..progress.produced]));
            let done = match operation {
                Operation::End => progress.status == EncoderStatus::Finished,
                _ => piece.is_empty(),
            };
            if done {
                break;
            }
        }
    }
}

/// Views bytes that are all initialized: created with `MaybeUninit::new`,
/// or reported written by the encoder.
fn initialized_mut(buffer: &mut [MaybeUninit<u8>]) -> &mut [u8] {
    // SAFETY: callers pass only initialized bytes (see above), and the
    // encoders write only initialized bytes, so none is de-initialized.
    unsafe { &mut *(buffer as *mut [MaybeUninit<u8>] as *mut [u8]) }
}

fn uninit_output(c: &mut Criterion) {
    let input = corpus::mixed(1 << 20, 10);
    let chunk = 16 << 10;
    let steps: Vec<(&[u8], Directive)> = input
        .chunks(chunk)
        .map(|piece| (piece, Directive::Continue))
        .chain([(&[][..], Directive::End)])
        .collect();
    let mut out = Vec::with_capacity(input.len());
    for capacity in [4 << 10, 128 << 10] {
        let mut group =
            c.benchmark_group(format!("stream-output-{}KiB/mixed-1MiB", capacity >> 10));
        group.throughput(Throughput::Bytes(input.len() as u64));
        let mut buffer = vec![MaybeUninit::new(0u8); capacity];
        for level in [1, 3] {
            let params = Params::level(level);
            let theirs =
                oracle::compress_stream_with_output(&params, &steps, capacity).expect("reference");
            let mut mzstd = encoder(level);
            for (id, uninit) in [("mzstd-process", false), ("mzstd-process_uninit", true)] {
                stream_into(&mut mzstd, &input, chunk, &mut buffer, uninit, &mut out);
                assert_eq!(out, theirs, "{id} level {level} output {capacity}");
                group.bench_function(BenchmarkId::new(id, level), |b| {
                    b.iter(|| {
                        stream_into(
                            &mut mzstd,
                            black_box(&input),
                            chunk,
                            &mut buffer,
                            uninit,
                            &mut out,
                        );
                    });
                });
            }
            group.bench_function(BenchmarkId::new("zstd", level), |b| {
                b.iter(|| {
                    oracle::compress_stream_with_output(&params, black_box(&steps), capacity)
                });
            });
        }
        group.finish();
    }

    let mut group = c.benchmark_group("compress_to_uninit/text-1MiB");
    let input = corpus::text(1 << 20, 1);
    group.throughput(Throughput::Bytes(input.len() as u64));
    let bound = Compressor::max_compressed_size(input.len()).expect("a bound");
    let mut destination: Vec<u8> = Vec::with_capacity(bound);
    for level in [1, 3] {
        let mut mzstd = encoder(level);
        let theirs = oracle::compress2(&input, &Params::level(level)).expect("reference");
        let size = mzstd
            .compress_to_uninit(&input, destination.spare_capacity_mut())
            .expect("compresses");
        assert_eq!(
            initialized_mut(&mut destination.spare_capacity_mut()[..size]),
            &theirs[..]
        );
        group.bench_function(BenchmarkId::new("mzstd", level), |b| {
            b.iter(|| {
                mzstd
                    .compress_to_uninit(black_box(&input), destination.spare_capacity_mut())
                    .expect("compresses")
            });
        });
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = compress, streaming, uninit_output
}
criterion_main!(benches);
