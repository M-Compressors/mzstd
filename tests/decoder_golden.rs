#![cfg(feature = "decompression")]
//! The reference's golden decompression vectors.

mod decoder_support;

use decoder_support::{decode_fixed, golden_dir, reference_decompress};
use mzstd::Decompressor;

fn vectors(directory: &str) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<_> = std::fs::read_dir(golden_dir(directory))
        .expect("golden directory")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "zst"))
        .map(|path| {
            let name = path
                .file_name()
                .expect("name")
                .to_string_lossy()
                .into_owned();
            (name, std::fs::read(&path).expect("readable"))
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "{directory} has vectors");
    files
}

#[test]
fn golden_vectors_decode_like_the_reference() {
    let mut decoder = Decompressor::default();
    for (name, frame) in vectors("golden-decompression") {
        let expected = reference_decompress(&frame, 1 << 22).expect("the reference decodes");
        assert_eq!(decoder.decompress(&frame).expect(&name), expected, "{name}");
        for (in_chunk, out_chunk) in [(1, 1), (7, 4096), (1 << 20, 1 << 20)] {
            let decoded = decode_fixed(&mut decoder, &frame, in_chunk, out_chunk).expect(&name);
            assert_eq!(decoded, expected, "{name} streaming {in_chunk}/{out_chunk}");
        }
    }
}

#[test]
fn golden_error_vectors_are_rejected_like_the_reference() {
    let mut decoder = Decompressor::default();
    for (name, frame) in vectors("golden-decompression-errors") {
        assert!(
            reference_decompress(&frame, 1 << 22).is_err(),
            "{name}: reference rejects"
        );
        let error = decoder.decompress(&frame).expect_err(&name);
        assert!(
            matches!(error, mzstd::DecodeError::InvalidData { .. }),
            "{name}: {error:?}"
        );
        assert!(
            decode_fixed(&mut decoder, &frame, 1, 1).is_err(),
            "{name} streaming"
        );
    }
}
