#![cfg(feature = "decompression")]
//! Malformed input: truncated and corrupted frames must be rejected without
//! panicking, and wherever the reference decoder accepts corrupted input,
//! this decoder must produce the same bytes.

mod decoder_support;

use decoder_support::{
    Rng, compress, decode_fixed, mixed, params, records, reference_decompress, text,
};
use mzstd::{DecodeError, Decompressor, InvalidDataKind};

/// A few frames that between them use every block and table mode.
fn sample_frames() -> Vec<(String, Vec<u8>)> {
    let mut frames = Vec::new();
    for (name, input) in [
        ("text", text(3_000, 1)),
        ("records", records(2_000, 2)),
        ("mixed", mixed(5_000, 3)),
        ("tiny", b"abcabcabcabc".to_vec()),
    ] {
        for (level, checksum) in [(1, true), (3, false), (19, true), (-3, true)] {
            frames.push((
                format!("{name} level {level}"),
                compress(&input, &params(level, checksum, true)),
            ));
        }
    }
    frames
}

#[test]
fn every_truncation_is_rejected() {
    let mut decoder = Decompressor::default();
    for (name, frame) in sample_frames() {
        for cut in 0..frame.len() {
            let truncated = &frame[..cut];
            let error = decoder
                .decompress(truncated)
                .expect_err(&format!("{name} cut at {cut}"));
            assert!(
                matches!(error, DecodeError::UnexpectedEndOfInput),
                "{name} cut at {cut}: {error:?}"
            );
            let streamed = decode_fixed(&mut decoder, truncated, 3, 64);
            assert!(
                matches!(streamed, Err(DecodeError::UnexpectedEndOfInput)),
                "{name} cut at {cut} streaming: {streamed:?}"
            );
        }
    }
}

/// Decodes `input` with both decoders and checks they agree wherever both
/// can: identical output when both succeed. Returns whether each succeeded.
fn compare(decoder: &mut Decompressor, input: &[u8], label: &str) -> (bool, bool) {
    let ours = decoder.decompress(input);
    let streamed = decode_fixed(decoder, input, 7, 33);
    match (&ours, &streamed) {
        (Ok(one_shot), Ok(streaming)) => assert_eq!(one_shot, streaming, "{label}: modes agree"),
        (Err(_), Err(_)) => {}
        _ => panic!("{label}: one-shot {ours:?} vs streaming {streamed:?}"),
    }
    let reference = reference_decompress(input, 1 << 20);
    if let (Ok(ours), Ok(reference)) = (&ours, &reference) {
        assert_eq!(ours, reference, "{label}: outputs agree");
    }
    (ours.is_ok(), reference.is_ok())
}

#[test]
fn corrupted_frames_never_panic_and_agree_with_the_reference() {
    let mut decoder = Decompressor::default();
    let mut rng = Rng::new(0xc0ffee);
    let mut stricter = 0;
    let mut trials = 0;
    for (name, frame) in sample_frames() {
        // Every single-bit flip of the first 64 bytes, then random damage.
        let header_bits = frame.len().min(64) * 8;
        let mut cases: Vec<Vec<u8>> = (0..header_bits)
            .map(|bit| {
                let mut damaged = frame.clone();
                damaged[bit / 8] ^= 1 << (bit % 8);
                damaged
            })
            .collect();
        for _ in 0..300 {
            let mut damaged = frame.clone();
            for _ in 0..1 + rng.below(3) {
                let at = rng.below(damaged.len());
                damaged[at] = rng.next_u64() as u8;
            }
            cases.push(damaged);
        }
        for (index, damaged) in cases.iter().enumerate() {
            trials += 1;
            let label = format!("{name} case {index}");
            match compare(&mut decoder, damaged, &label) {
                (ours, reference) if ours == reference => {}
                (true, false) => panic!("{label}: accepted what the reference rejects"),
                _ => {
                    // The reference's fast four-stream Huffman decoder only
                    // checks the regenerated length, not that each stream
                    // is consumed exactly as RFC 8878 requires; that is the
                    // one check this decoder makes and it does not.
                    let error = decoder.decompress(damaged).expect_err(&label);
                    assert!(
                        matches!(
                            error,
                            DecodeError::InvalidData {
                                kind: InvalidDataKind::HuffmanStream
                            }
                        ),
                        "{label}: rejected with {error:?} where the reference decodes"
                    );
                    stricter += 1;
                }
            }
        }
    }
    // The leniency only shows in frames without a checksum, and rarely.
    assert!(
        stricter * 20 < trials,
        "{stricter} of {trials} stricter verdicts"
    );
}
