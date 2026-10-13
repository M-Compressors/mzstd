//! The C ABI against zstd's one-shot C API, through identical calls.
//!
//! Both sides are called the way a C program calls them: one function per
//! payload, into a caller-owned buffer of `compress_bound` bytes, with codec
//! state built and released inside the call. Inputs are zstd's own
//! compression regression files plus synthetic text, and a 64-byte prefix,
//! which isolates per-call cost.
//!
//! Before timing, every frame is checked to be byte-identical between the two
//! encoders and to decode back to its payload through both decoders, and the
//! compressed sizes are printed, so a speed figure is always read against an
//! identical ratio.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use mzstd_ffi::{MzstdResult, mzstd_compress, mzstd_compress_bound, mzstd_decompress};
use std::ffi::c_int;
use std::hint::black_box;
use std::path::Path;

/// Levels timed: a negative one, the default, a lazy one and the densest
/// regular one.
const LEVELS: [c_int; 4] = [-5, 3, 9, 19];

/// Named inputs: text, structured binary, incompressible, and a tiny prefix.
fn corpora() -> Vec<(&'static str, Vec<u8>)> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../zstd-ffi/vendor/zstd/tests/golden-compression");
    let read = |name: &str| {
        std::fs::read(directory.join(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
    };
    let text = zstd_ffi::corpus::text(1 << 20, 1);
    vec![
        ("tiny-64B", text[..64].to_vec()),
        ("text-1MiB", text),
        (
            "large-literal-and-match-lengths",
            read("large-literal-and-match-lengths"),
        ),
        ("random-256KiB", zstd_ffi::corpus::random(256 << 10, 2)),
    ]
}

fn ours_compress(input: &[u8], output: &mut [u8], level: c_int) -> usize {
    let mut len = output.len();
    // SAFETY: both buffers are live and have the stated lengths.
    let status = unsafe {
        mzstd_compress(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            &mut len,
            level,
        )
    };
    assert_eq!(status, MzstdResult::Ok);
    len
}

fn reference_compress(input: &[u8], output: &mut [u8], level: c_int) -> usize {
    // SAFETY: both buffers are live and have the stated lengths.
    let len = unsafe {
        zstd_ffi::ZSTD_compress(
            output.as_mut_ptr().cast(),
            output.len(),
            input.as_ptr().cast(),
            input.len(),
            level,
        )
    };
    // SAFETY: a pure function of its argument.
    assert_eq!(unsafe { zstd_ffi::ZSTD_isError(len) }, 0);
    len
}

fn ours_decompress(input: &[u8], output: &mut [u8]) -> usize {
    let mut len = output.len();
    // SAFETY: both buffers are live and have the stated lengths.
    let status =
        unsafe { mzstd_decompress(input.as_ptr(), input.len(), output.as_mut_ptr(), &mut len) };
    assert_eq!(status, MzstdResult::Ok);
    len
}

fn reference_decompress(input: &[u8], output: &mut [u8]) -> usize {
    // SAFETY: both buffers are live and have the stated lengths.
    let len = unsafe {
        zstd_ffi::ZSTD_decompress(
            output.as_mut_ptr().cast(),
            output.len(),
            input.as_ptr().cast(),
            input.len(),
        )
    };
    // SAFETY: a pure function of its argument.
    assert_eq!(unsafe { zstd_ffi::ZSTD_isError(len) }, 0);
    len
}

fn one_shot(c: &mut Criterion) {
    for (name, input) in corpora() {
        let mut ours = vec![0u8; mzstd_compress_bound(input.len())];
        let mut theirs = vec![0u8; ours.len()];
        let mut restored = vec![0u8; input.len()];
        for level in LEVELS {
            let ours_len = ours_compress(&input, &mut ours, level);
            let theirs_len = reference_compress(&input, &mut theirs, level);
            assert_eq!(
                ours[..ours_len],
                theirs[..theirs_len],
                "{name} level {level}"
            );
            assert_eq!(
                ours_decompress(&ours[..ours_len], &mut restored),
                input.len()
            );
            assert_eq!(restored, input);
            assert_eq!(
                reference_decompress(&ours[..ours_len], &mut restored),
                input.len()
            );
            println!(
                "{name} level {level}: {} -> {ours_len} bytes ({:.3})",
                input.len(),
                ours_len as f64 / input.len().max(1) as f64
            );

            let mut group = c.benchmark_group(format!("ffi-compress/{name}"));
            group.throughput(Throughput::Bytes(input.len() as u64));
            group.bench_function(BenchmarkId::new("mzstd", level), |b| {
                b.iter(|| ours_compress(black_box(&input), &mut ours, level));
            });
            group.bench_function(BenchmarkId::new("zstd", level), |b| {
                b.iter(|| reference_compress(black_box(&input), &mut theirs, level));
            });
            group.finish();

            let frame = theirs[..theirs_len].to_vec();
            let mut group = c.benchmark_group(format!("ffi-decompress/{name}"));
            group.throughput(Throughput::Bytes(input.len() as u64));
            group.bench_function(BenchmarkId::new("mzstd", level), |b| {
                b.iter(|| ours_decompress(black_box(&frame), &mut restored));
            });
            group.bench_function(BenchmarkId::new("zstd", level), |b| {
                b.iter(|| reference_decompress(black_box(&frame), &mut restored));
            });
            group.finish();
        }
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = one_shot
}
criterion_main!(benches);
