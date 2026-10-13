//! The streaming decoder oracle's end-of-input check.

use zstd_ffi::oracle;

/// A frame of one run-length block of `len` bytes of `byte`.
fn rle_frame(len: u32, byte: u8) -> Vec<u8> {
    let header = (len << 3) | (1 << 1) | 1;
    let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x58];
    frame.extend_from_slice(&header.to_le_bytes()[..3]);
    frame.push(byte);
    frame
}

#[test]
fn a_frame_ending_at_a_full_output_buffer_is_complete() {
    // 128 KiB is exactly the oracle's output buffer.
    let frame = rle_frame(1 << 17, b'z');
    for chunk in [1, 3, frame.len()] {
        let decoded = oracle::decompress_stream(&frame, chunk, Some(31)).expect("complete");
        assert_eq!(decoded, vec![b'z'; 1 << 17]);
    }
}

#[test]
fn a_frame_cut_short_is_still_truncated() {
    let frame = rle_frame(1 << 17, b'z');
    for cut in [4, 6, frame.len() - 1] {
        assert!(
            oracle::decompress_stream(&frame[..cut], cut, Some(31)).is_err(),
            "{cut}"
        );
    }
}
