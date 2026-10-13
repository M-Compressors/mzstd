//! The exported C functions, called through their ABI exactly as C would.

use mzstd_ffi::{MzstdResult, mzstd_compress, mzstd_compress_bound, mzstd_decompress};
use std::ffi::c_int;
use std::ptr;
use zstd_ffi::corpus;
use zstd_ffi::oracle;

const HTTP: &[u8] = include_bytes!("../../zstd-ffi/vendor/zstd/tests/golden-compression/http");

/// Empty, tiny, text, binary, incompressible and multi-block inputs.
fn inputs() -> Vec<Vec<u8>> {
    vec![
        Vec::new(),
        b"a".to_vec(),
        HTTP.to_vec(),
        corpus::text(60_000, 1),
        corpus::binary(40_000, 2),
        corpus::random(20_000, 3),
        corpus::mixed(400_000, 4),
    ]
}

/// Calls `mzstd_compress` into a buffer of `capacity` bytes.
fn compress(input: &[u8], capacity: usize, level: c_int) -> (MzstdResult, Vec<u8>) {
    let mut output = vec![0u8; capacity];
    let mut output_len = capacity;
    // SAFETY: both buffers are live and have the stated lengths.
    let status = unsafe {
        mzstd_compress(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            &mut output_len,
            level,
        )
    };
    output.truncate(output_len);
    (status, output)
}

/// Calls `mzstd_decompress` into a buffer of `capacity` bytes.
fn decompress(input: &[u8], capacity: usize) -> (MzstdResult, Vec<u8>) {
    let mut output = vec![0u8; capacity];
    let mut output_len = capacity;
    // SAFETY: both buffers are live and have the stated lengths.
    let status = unsafe {
        mzstd_decompress(
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            &mut output_len,
        )
    };
    output.truncate(output_len);
    (status, output)
}

#[test]
fn compression_is_byte_identical_to_zstd_compress_at_every_level() {
    for input in inputs() {
        for level in [
            -131_072, -1000, -7, -1, 0, 1, 2, 3, 4, 5, 6, 7, 9, 12, 15, 16, 17, 19, 22,
        ] {
            if input.len() > 100_000 && level > 19 {
                continue;
            }
            let (status, ours) = compress(&input, mzstd_compress_bound(input.len()), level);
            assert_eq!(status, MzstdResult::Ok, "level {level}");
            let theirs = oracle::compress(&input, level);
            assert_eq!(ours, theirs, "level {level}, {} bytes", input.len());
        }
    }
}

#[test]
fn compress_bound_equals_zstd_compress_bound() {
    for len in [
        0,
        1,
        255,
        256,
        1000,
        128 << 10,
        (128 << 10) + 1,
        1 << 20,
        1 << 30,
    ] {
        assert_eq!(
            mzstd_compress_bound(len),
            oracle::compress_bound(len),
            "{len}"
        );
    }
    assert_eq!(mzstd_compress_bound(usize::MAX), 0);
}

#[test]
fn frames_round_trip_through_both_decoders() {
    for input in inputs() {
        let (_, frame) = compress(&input, mzstd_compress_bound(input.len()), 5);
        let (status, ours) = decompress(&frame, input.len());
        assert_eq!(status, MzstdResult::Ok);
        assert_eq!(ours, input);
        assert_eq!(
            oracle::decompress(&frame, input.len()).expect("reference"),
            input
        );
    }
}

#[test]
fn reference_frames_with_checksums_and_long_windows_decompress() {
    let input = corpus::mixed(3 << 20, 9);
    for params in [
        oracle::Params::level(3).with(zstd_ffi::ZSTD_c_checksumFlag, 1),
        oracle::Params::level(19)
            .with(zstd_ffi::ZSTD_c_windowLog, 30)
            .with(zstd_ffi::ZSTD_c_enableLongDistanceMatching, 1),
    ] {
        let frame = oracle::compress2(&input, &params).expect("reference");
        let (status, ours) = decompress(&frame, input.len());
        assert_eq!(status, MzstdResult::Ok);
        assert_eq!(ours, input);
    }
}

#[test]
fn concatenated_and_skippable_frames_decode_in_order() {
    let a = compress(b"first ", 128, 1).1;
    let b = compress(b"second", 128, 22).1;
    let skippable = [0x50, 0x2a, 0x4d, 0x18, 3, 0, 0, 0, 1, 2, 3];
    let input = [&skippable[..], &a, &skippable, &b].concat();
    let (status, output) = decompress(&input, 64);
    assert_eq!(status, MzstdResult::Ok);
    assert_eq!(output, b"first second");
    assert_eq!(decompress(&skippable, 4), (MzstdResult::Ok, Vec::new()));
}

#[test]
fn a_short_destination_fails_exactly_when_zstd_compress_fails() {
    for input in [
        Vec::new(),
        b"a".to_vec(),
        corpus::text(30_000, 7),
        corpus::random(5_000, 8),
    ] {
        let frame = oracle::compress(&input, 3);
        let capacities = (0..=40).chain(frame.len().saturating_sub(40)..=frame.len() + 40);
        for capacity in capacities {
            let (status, ours) = compress(&input, capacity, 3);
            match oracle::compress_capacity(&input, 3, capacity) {
                Ok(theirs) => assert_eq!((status, ours), (MzstdResult::Ok, theirs), "{capacity}"),
                Err(_) => assert_eq!(
                    (status, ours),
                    (MzstdResult::OutputTooSmall, Vec::new()),
                    "{capacity}"
                ),
            }
        }
    }
}

#[test]
fn compress_accepts_a_null_empty_input() {
    let mut output = [0u8; 18];
    let mut output_len = output.len();
    // SAFETY: an empty input may be null; `output` has `output_len` bytes.
    let status = unsafe { mzstd_compress(ptr::null(), 0, output.as_mut_ptr(), &mut output_len, 3) };
    assert_eq!(status, MzstdResult::Ok);
    assert_eq!(&output[..output_len], oracle::compress(b"", 3));
}

#[test]
fn compress_into_a_null_empty_output_is_too_small() {
    let mut output_len = 0;
    // SAFETY: an empty output may be null.
    let status = unsafe { mzstd_compress(b"x".as_ptr(), 1, ptr::null_mut(), &mut output_len, 3) };
    assert_eq!(status, MzstdResult::OutputTooSmall);
    assert_eq!(output_len, 0);
}

#[test]
fn compress_rejects_levels_out_of_range_and_zeroes_the_length() {
    for level in [-131_073, 23, c_int::MIN, c_int::MAX] {
        let (status, output) = compress(b"payload", 128, level);
        assert_eq!(status, MzstdResult::InvalidParameter, "{level}");
        assert!(output.is_empty());
    }
}

#[test]
fn a_null_output_length_is_rejected_without_being_written() {
    let mut output = [0u8; 64];
    // SAFETY: a null length pointer is detected before anything is read.
    let status =
        unsafe { mzstd_compress(b"x".as_ptr(), 1, output.as_mut_ptr(), ptr::null_mut(), 3) };
    assert_eq!(status, MzstdResult::InvalidParameter);
    // SAFETY: as above.
    let status =
        unsafe { mzstd_decompress(b"x".as_ptr(), 1, output.as_mut_ptr(), ptr::null_mut()) };
    assert_eq!(status, MzstdResult::InvalidParameter);
}

#[test]
fn a_misaligned_output_length_is_rejected_without_being_written() {
    let mut storage = [0u8; 16];
    let misaligned = storage.as_mut_ptr().wrapping_add(1).cast::<usize>();
    let mut output = [0u8; 64];
    // SAFETY: a misaligned length pointer is detected before it is read.
    let status = unsafe { mzstd_compress(b"x".as_ptr(), 1, output.as_mut_ptr(), misaligned, 3) };
    assert_eq!(status, MzstdResult::InvalidParameter);
    assert_eq!(storage, [0; 16]);
}

#[test]
fn null_buffers_with_a_length_are_rejected() {
    let mut output = [0u8; 64];
    let mut output_len = output.len();
    // SAFETY: the null input is detected before it is read.
    let status = unsafe { mzstd_compress(ptr::null(), 5, output.as_mut_ptr(), &mut output_len, 3) };
    assert_eq!(status, MzstdResult::InvalidParameter);
    assert_eq!(output_len, 0);
    let mut output_len = 8;
    // SAFETY: the null output is detected before it is written.
    let status = unsafe { mzstd_decompress(b"x".as_ptr(), 1, ptr::null_mut(), &mut output_len) };
    assert_eq!(status, MzstdResult::InvalidParameter);
    assert_eq!(output_len, 0);
}

#[test]
fn lengths_no_allocation_can_have_are_rejected() {
    let mut output = [0u8; 64];
    let mut output_len = output.len();
    // SAFETY: the impossible length is detected before the input is read.
    let status = unsafe {
        mzstd_compress(
            b"x".as_ptr(),
            usize::MAX,
            output.as_mut_ptr(),
            &mut output_len,
            3,
        )
    };
    assert_eq!(status, MzstdResult::InvalidParameter);
    let mut output_len = isize::MAX.unsigned_abs() + 1;
    // SAFETY: as above, for the output.
    let status =
        unsafe { mzstd_decompress(b"x".as_ptr(), 1, output.as_mut_ptr(), &mut output_len) };
    assert_eq!(status, MzstdResult::InvalidParameter);
}

#[test]
fn overlapping_input_and_output_are_rejected() {
    let mut buffer = [0u8; 256];
    let mut output_len = 128;
    let base = buffer.as_mut_ptr();
    // SAFETY: the overlap is detected before either buffer is touched.
    let status = unsafe { mzstd_compress(base, 128, base.wrapping_add(64), &mut output_len, 3) };
    assert_eq!(status, MzstdResult::InvalidParameter);
    assert_eq!(output_len, 0);
}

#[test]
fn a_buffer_overlapping_the_output_length_is_rejected() {
    let mut slots = [0usize; 16];
    slots[0] = 64;
    let base = slots.as_mut_ptr();
    // SAFETY: the overlap with the length slot is detected before any write.
    let status = unsafe { mzstd_compress(b"x".as_ptr(), 1, base.cast::<u8>(), base, 3) };
    assert_eq!(status, MzstdResult::InvalidParameter);
}

#[test]
fn decompress_reports_a_short_buffer() {
    let input = corpus::text(10_000, 5);
    let (_, frame) = compress(&input, mzstd_compress_bound(input.len()), 3);
    assert_eq!(
        decompress(&frame, input.len() - 1),
        (MzstdResult::OutputTooSmall, Vec::new())
    );
}

#[test]
fn decompress_rejects_corrupt_and_truncated_input() {
    let input = corpus::text(10_000, 6);
    let (_, frame) = compress(&input, mzstd_compress_bound(input.len()), 3);
    assert_eq!(
        decompress(&frame[..frame.len() - 1], input.len()).0,
        MzstdResult::Error
    );
    let mut corrupt = frame.clone();
    corrupt[0] ^= 1;
    assert_eq!(decompress(&corrupt, input.len()).0, MzstdResult::Error);
    let trailing = [&frame[..], &[0x28, 0xb5]].concat();
    assert_eq!(decompress(&trailing, input.len()).0, MzstdResult::Error);
}

#[test]
fn decompress_of_an_empty_input_writes_nothing() {
    let mut output_len = 0;
    // SAFETY: both buffers are empty and may be null.
    let status = unsafe { mzstd_decompress(ptr::null(), 0, ptr::null_mut(), &mut output_len) };
    assert_eq!(status, MzstdResult::Ok);
    assert_eq!(output_len, 0);
}

#[test]
fn concurrent_calls_do_not_interfere() {
    let input = corpus::mixed(200_000, 8);
    let expected = oracle::compress(&input, 7);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                let (status, frame) = compress(&input, mzstd_compress_bound(input.len()), 7);
                assert_eq!(status, MzstdResult::Ok);
                assert_eq!(frame, expected);
                assert_eq!(
                    decompress(&frame, input.len()),
                    (MzstdResult::Ok, input.clone())
                );
            });
        }
    });
}

#[test]
fn status_codes_match_the_c_header() {
    let header = include_str!("../include/mzstd.h");
    for (name, status) in [
        ("MZSTD_OK", MzstdResult::Ok),
        ("MZSTD_INVALID_PARAMETER", MzstdResult::InvalidParameter),
        ("MZSTD_OUTPUT_TOO_SMALL", MzstdResult::OutputTooSmall),
        ("MZSTD_ERROR", MzstdResult::Error),
    ] {
        assert!(
            header.contains(&format!("{name} = {}", status as i32)),
            "{name}"
        );
    }
}
