//! The C ABI's sessions against zstd's streaming C API, through identical
//! calls.
//!
//! Both sides keep one context per side across iterations, as a C program
//! streaming many payloads does, and reset it before each payload: the
//! encoder takes 64 KiB input chunks with `continue` and then `end`, into a
//! 64 KiB output buffer; the decoder takes the frame in 64 KiB chunks into a
//! 128 KiB buffer. Output is appended to a vector whose capacity was reserved
//! before timing.
//!
//! Before timing, every frame is checked to be byte-identical between the two
//! encoders and to decode back to its payload through both decoders, and the
//! compressed sizes are printed, so a speed figure is always read against an
//! identical ratio.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use mzstd_ffi::{
    MZSTD_CONTENT_SIZE_UNKNOWN, MzstdDecodeOperation, MzstdDecoder, MzstdEncoder, MzstdOperation,
    MzstdResult, MzstdStatus, mzstd_compress_bound, mzstd_decoder_create, mzstd_decoder_free,
    mzstd_decoder_process, mzstd_decoder_reset, mzstd_encoder_create, mzstd_encoder_free,
    mzstd_encoder_process, mzstd_encoder_reset,
};
use std::ffi::c_int;
use std::hint::black_box;
use std::path::Path;
use zstd_ffi::{ZSTD_CCtx, ZSTD_DCtx, ZSTD_inBuffer, ZSTD_outBuffer, ZSTD_reset_session_only};

/// Levels timed: a negative one, the default, a lazy one and the densest
/// regular one.
const LEVELS: [c_int; 4] = [-5, 3, 9, 19];
/// Input bytes offered per encoder call.
const CHUNK: usize = 64 << 10;
/// Output room per encoder call.
const ENCODE_ROOM: usize = 64 << 10;
/// Output room per decoder call.
const DECODE_ROOM: usize = 128 << 10;

/// Named inputs: text, structured binary and incompressible.
fn corpora() -> Vec<(&'static str, Vec<u8>)> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../zstd-ffi/vendor/zstd/tests/golden-compression");
    let read = |name: &str| {
        std::fs::read(directory.join(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
    };
    vec![
        ("text-1MiB", zstd_ffi::corpus::text(1 << 20, 1)),
        (
            "large-literal-and-match-lengths",
            read("large-literal-and-match-lengths"),
        ),
        ("random-256KiB", zstd_ffi::corpus::random(256 << 10, 2)),
    ]
}

/// A reused mzstd encoder and decoder.
struct Ours {
    encoder: *mut MzstdEncoder,
    decoder: *mut MzstdDecoder,
    room: Vec<u8>,
}

impl Ours {
    fn new(level: c_int) -> Self {
        let mut encoder = std::ptr::null_mut();
        let mut decoder = std::ptr::null_mut();
        // SAFETY: both are writable pointer slots.
        unsafe {
            assert_eq!(mzstd_encoder_create(level, &mut encoder), MzstdResult::Ok);
            assert_eq!(mzstd_decoder_create(0, &mut decoder), MzstdResult::Ok);
        }
        Self {
            encoder,
            decoder,
            room: vec![0; DECODE_ROOM],
        }
    }

    /// One encoder call; returns the bytes consumed and the status.
    fn encode_step(
        &mut self,
        input: &[u8],
        operation: MzstdOperation,
        frame: &mut Vec<u8>,
    ) -> (usize, MzstdStatus) {
        let (mut input_len, mut output_len) = (input.len(), ENCODE_ROOM);
        let mut status = MzstdStatus::NeedsInput;
        // SAFETY: the handle is live and every buffer has its stated length.
        let result = unsafe {
            mzstd_encoder_process(
                self.encoder,
                input.as_ptr(),
                &mut input_len,
                self.room.as_mut_ptr(),
                &mut output_len,
                operation as c_int,
                &mut status,
            )
        };
        assert_eq!(result, MzstdResult::Ok);
        frame.extend_from_slice(&self.room[..output_len]);
        (input_len, status)
    }

    fn compress(&mut self, input: &[u8], frame: &mut Vec<u8>) {
        frame.clear();
        // SAFETY: the handle is live.
        let reset = unsafe { mzstd_encoder_reset(self.encoder, MZSTD_CONTENT_SIZE_UNKNOWN) };
        assert_eq!(reset, MzstdResult::Ok);
        for mut chunk in input.chunks(CHUNK) {
            while !chunk.is_empty() {
                let (consumed, _) = self.encode_step(chunk, MzstdOperation::Continue, frame);
                chunk = &chunk[consumed..];
            }
        }
        while self.encode_step(&[], MzstdOperation::End, frame).1 != MzstdStatus::Finished {}
    }

    fn decompress(&mut self, frame: &[u8], output: &mut Vec<u8>) {
        output.clear();
        // SAFETY: the handle is live.
        assert_eq!(
            unsafe { mzstd_decoder_reset(self.decoder) },
            MzstdResult::Ok
        );
        let (mut position, mut offered) = (0, 0);
        loop {
            if position == offered {
                offered = (offered + CHUNK).min(frame.len());
            }
            let operation = if offered == frame.len() {
                MzstdDecodeOperation::Finish
            } else {
                MzstdDecodeOperation::Process
            };
            let input = &frame[position..offered];
            let (mut input_len, mut output_len) = (input.len(), DECODE_ROOM);
            let mut status = MzstdStatus::NeedsInput;
            // SAFETY: the handle is live and every buffer has its stated
            // length.
            let result = unsafe {
                mzstd_decoder_process(
                    self.decoder,
                    input.as_ptr(),
                    &mut input_len,
                    self.room.as_mut_ptr(),
                    &mut output_len,
                    operation as c_int,
                    &mut status,
                )
            };
            assert_eq!(result, MzstdResult::Ok);
            position += input_len;
            output.extend_from_slice(&self.room[..output_len]);
            if status == MzstdStatus::Finished {
                return;
            }
        }
    }
}

impl Drop for Ours {
    fn drop(&mut self) {
        // SAFETY: both handles are live and released once.
        unsafe {
            mzstd_encoder_free(self.encoder);
            mzstd_decoder_free(self.decoder);
        }
    }
}

/// Fails on a zstd error code.
fn check(code: usize) -> usize {
    // SAFETY: a pure function of its argument.
    assert_eq!(unsafe { zstd_ffi::ZSTD_isError(code) }, 0);
    code
}

/// A reused zstd compression and decompression context.
struct Theirs {
    cctx: *mut ZSTD_CCtx,
    dctx: *mut ZSTD_DCtx,
    room: Vec<u8>,
}

impl Theirs {
    fn new(level: c_int) -> Self {
        // SAFETY: allocation only; null results are checked.
        let (cctx, dctx) = unsafe { (zstd_ffi::ZSTD_createCCtx(), zstd_ffi::ZSTD_createDCtx()) };
        assert!(!cctx.is_null() && !dctx.is_null());
        // SAFETY: the context is live.
        check(unsafe {
            zstd_ffi::ZSTD_CCtx_setParameter(cctx, zstd_ffi::ZSTD_c_compressionLevel, level)
        });
        Self {
            cctx,
            dctx,
            room: vec![0; DECODE_ROOM],
        }
    }

    /// One `ZSTD_compressStream2` call; returns the bytes consumed and what
    /// remains to flush.
    fn encode_step(
        &mut self,
        input: &[u8],
        directive: zstd_ffi::ZSTD_EndDirective,
        frame: &mut Vec<u8>,
    ) -> (usize, usize) {
        let mut source = ZSTD_inBuffer {
            src: input.as_ptr().cast(),
            size: input.len(),
            pos: 0,
        };
        let mut destination = ZSTD_outBuffer {
            dst: self.room.as_mut_ptr().cast(),
            size: ENCODE_ROOM,
            pos: 0,
        };
        // SAFETY: the buffers describe live, disjoint memory of the stated
        // sizes, and the context is live.
        let remaining = check(unsafe {
            zstd_ffi::ZSTD_compressStream2(self.cctx, &mut destination, &mut source, directive)
        });
        frame.extend_from_slice(&self.room[..destination.pos]);
        (source.pos, remaining)
    }

    fn compress(&mut self, input: &[u8], frame: &mut Vec<u8>) {
        frame.clear();
        // SAFETY: the context is live.
        check(unsafe { zstd_ffi::ZSTD_CCtx_reset(self.cctx, ZSTD_reset_session_only) });
        for mut chunk in input.chunks(CHUNK) {
            while !chunk.is_empty() {
                let (consumed, _) = self.encode_step(chunk, zstd_ffi::ZSTD_e_continue, frame);
                chunk = &chunk[consumed..];
            }
        }
        while self.encode_step(&[], zstd_ffi::ZSTD_e_end, frame).1 != 0 {}
    }

    fn decompress(&mut self, frame: &[u8], output: &mut Vec<u8>) {
        output.clear();
        // SAFETY: the context is live.
        check(unsafe { zstd_ffi::ZSTD_DCtx_reset(self.dctx, ZSTD_reset_session_only) });
        let mut source = ZSTD_inBuffer {
            src: frame.as_ptr().cast(),
            size: 0,
            pos: 0,
        };
        loop {
            if source.pos == source.size {
                source.size = (source.size + CHUNK).min(frame.len());
            }
            let mut destination = ZSTD_outBuffer {
                dst: self.room.as_mut_ptr().cast(),
                size: DECODE_ROOM,
                pos: 0,
            };
            // SAFETY: the buffers describe live, disjoint memory of the
            // stated sizes, and the context is live.
            let hint = check(unsafe {
                zstd_ffi::ZSTD_decompressStream(self.dctx, &mut destination, &mut source)
            });
            output.extend_from_slice(&self.room[..destination.pos]);
            if hint == 0 {
                return;
            }
        }
    }
}

impl Drop for Theirs {
    fn drop(&mut self) {
        // SAFETY: both contexts are live and freed once.
        unsafe {
            zstd_ffi::ZSTD_freeCCtx(self.cctx);
            zstd_ffi::ZSTD_freeDCtx(self.dctx);
        }
    }
}

fn session(c: &mut Criterion) {
    for (name, input) in corpora() {
        let capacity = mzstd_compress_bound(input.len());
        for level in LEVELS {
            let mut ours = Ours::new(level);
            let mut theirs = Theirs::new(level);
            let (mut our_frame, mut their_frame) =
                (Vec::with_capacity(capacity), Vec::with_capacity(capacity));
            ours.compress(&input, &mut our_frame);
            theirs.compress(&input, &mut their_frame);
            assert_eq!(our_frame, their_frame, "{name} level {level}");
            let mut restored = Vec::with_capacity(input.len());
            ours.decompress(&their_frame, &mut restored);
            assert_eq!(restored, input);
            theirs.decompress(&our_frame, &mut restored);
            assert_eq!(restored, input);
            println!(
                "{name} level {level}: {} -> {} bytes ({:.3})",
                input.len(),
                our_frame.len(),
                our_frame.len() as f64 / input.len().max(1) as f64
            );

            let mut group = c.benchmark_group(format!("ffi-session-compress/{name}"));
            group.throughput(Throughput::Bytes(input.len() as u64));
            group.bench_function(BenchmarkId::new("mzstd", level), |b| {
                b.iter(|| ours.compress(black_box(&input), &mut our_frame));
            });
            group.bench_function(BenchmarkId::new("zstd", level), |b| {
                b.iter(|| theirs.compress(black_box(&input), &mut their_frame));
            });
            group.finish();

            let frame = their_frame.clone();
            let mut group = c.benchmark_group(format!("ffi-session-decompress/{name}"));
            group.throughput(Throughput::Bytes(input.len() as u64));
            group.bench_function(BenchmarkId::new("mzstd", level), |b| {
                b.iter(|| ours.decompress(black_box(&frame), &mut restored));
            });
            group.bench_function(BenchmarkId::new("zstd", level), |b| {
                b.iter(|| theirs.decompress(black_box(&frame), &mut restored));
            });
            group.finish();
        }
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = session
}
criterion_main!(benches);
