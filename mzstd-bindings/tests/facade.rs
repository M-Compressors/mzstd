//! The facade against zstd 1.5.7: one-shot and streamed frames must be the
//! reference's bytes, and everything the reference writes must decode.

use mzstd_bindings::{
    DEFAULT_LEVEL, MAX_LEVEL, MIN_LEVEL, MzstdError, StreamCompressor, StreamDecompressor,
};
use std::error::Error as _;
use zstd_ffi::corpus;
use zstd_ffi::oracle::{self, Dictionary, Directive, Params};

const LEVELS: [i32; 7] = [-5, -1, 1, 3, 7, 19, 22];

/// Inputs small enough for every level to stay quick.
fn inputs() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("empty", Vec::new()),
        ("byte", vec![42]),
        ("text", corpus::text(40_000, 1)),
        ("binary", corpus::binary(40_000, 2)),
        ("random", corpus::random(20_000, 3)),
    ]
}

/// Feeds `data` to a fresh stream in `chunk`-byte pieces, then finishes it.
fn stream_compress(data: &[u8], level: Option<i32>, chunk: usize) -> Vec<u8> {
    let mut stream = StreamCompressor::new(level).expect("valid level");
    let mut frame = Vec::new();
    for piece in data.chunks(chunk.max(1)) {
        frame.extend(stream.compress(piece).expect("compress"));
    }
    frame.extend(stream.finish().expect("finish"));
    frame
}

/// Feeds `frame` to a fresh stream in `chunk`-byte pieces, then finishes it.
fn stream_decompress(frame: &[u8], chunk: usize) -> Result<Vec<u8>, MzstdError> {
    let mut stream = StreamDecompressor::new(None)?;
    let mut data = Vec::new();
    for piece in frame.chunks(chunk.max(1)) {
        data.extend(stream.decompress(piece)?);
    }
    data.extend(stream.finish()?);
    Ok(data)
}

#[test]
fn one_shot_frames_match_compress2() {
    for (name, data) in inputs() {
        for level in LEVELS {
            let expected = oracle::compress2(&data, &Params::level(level)).expect("reference");
            let actual = mzstd_bindings::compress(&data, Some(level)).expect("compress");
            assert_eq!(actual, expected, "{name} at level {level}");
        }
    }
}

#[test]
fn default_level_is_zstd_default() {
    let data = corpus::text(10_000, 4);
    let expected = oracle::compress2(&data, &Params::level(DEFAULT_LEVEL)).expect("reference");
    assert_eq!(
        mzstd_bindings::compress(&data, None).ok(),
        Some(expected.clone())
    );
    assert_eq!(
        mzstd_bindings::compress(&data, Some(0)).ok(),
        Some(expected)
    );
    assert_eq!((MIN_LEVEL, DEFAULT_LEVEL, MAX_LEVEL), (-131_072, 3, 22));
}

#[test]
fn streamed_frames_match_compress_stream2() {
    for (name, data) in inputs() {
        for level in [1, 3, 19] {
            for chunk in [1, 1000, 1 << 20] {
                let pieces: Vec<_> = data
                    .chunks(chunk)
                    .map(|piece| (piece, Directive::Continue))
                    .collect();
                let expected =
                    oracle::compress_stream(&Params::level(level), &pieces).expect("reference");
                let actual = stream_compress(&data, Some(level), chunk);
                assert_eq!(
                    actual, expected,
                    "{name} at level {level} in {chunk}-byte chunks"
                );
            }
        }
    }
}

#[test]
fn flush_matches_compress_stream2_and_makes_input_decodable() {
    let data = corpus::text(50_000, 5);
    let (head, tail) = data.split_at(20_000);
    let mut stream = StreamCompressor::new(Some(3)).expect("level");
    let mut frame = stream.compress(head).expect("compress");
    frame.extend(stream.flush().expect("flush"));

    let mut decoder = StreamDecompressor::new(None).expect("decoder");
    assert_eq!(decoder.decompress(&frame).expect("prefix"), head);

    frame.extend(stream.compress(tail).expect("compress"));
    frame.extend(stream.finish().expect("finish"));
    let expected = oracle::compress_stream(
        &Params::level(3),
        &[(head, Directive::Flush), (tail, Directive::Continue)],
    )
    .expect("reference");
    assert_eq!(frame, expected);
}

#[test]
fn output_larger_than_one_chunk_streams_completely() {
    // Incompressible input makes every step return more than the first
    // output chunk, so the pump has to grow its buffer.
    let data = corpus::random(3 << 20, 6);
    let frame = stream_compress(&data, Some(1), data.len());
    assert_eq!(stream_decompress(&frame, frame.len()).ok(), Some(data));
}

#[test]
fn reference_frames_decode_one_shot_and_streamed() {
    for (name, data) in inputs() {
        let frame = oracle::compress(&data, 19);
        assert_eq!(
            mzstd_bindings::decompress(&frame, None).ok(),
            Some(data.clone()),
            "{name}"
        );
        for chunk in [1, 7, 4096] {
            assert_eq!(
                stream_decompress(&frame, chunk).ok(),
                Some(data.clone()),
                "{name}/{chunk}"
            );
        }
    }
}

#[test]
fn concatenated_frames_decode_to_concatenated_content() {
    let mut frames = oracle::compress(b"first ", 1);
    frames.extend(oracle::compress(b"second", 1));
    assert_eq!(
        mzstd_bindings::decompress(&frames, None).ok(),
        Some(b"first second".to_vec())
    );
    assert_eq!(
        stream_decompress(&frames, 3).ok(),
        Some(b"first second".to_vec())
    );
}

#[test]
fn empty_input_decodes_to_nothing() {
    assert_eq!(mzstd_bindings::decompress(&[], None).ok(), Some(Vec::new()));
    assert_eq!(
        mzstd_bindings::decompress_with_dictionary(&[], b"raw", None).ok(),
        Some(Vec::new())
    );
}

#[test]
fn output_cap_rejects_larger_content() {
    let data = vec![0u8; 100_000];
    let frame = mzstd_bindings::compress(&data, None).expect("compress");
    assert_eq!(
        mzstd_bindings::decompress(&frame, Some(100_000)).ok(),
        Some(data)
    );
    let error = mzstd_bindings::decompress(&frame, Some(99_999)).unwrap_err();
    assert!(matches!(error, MzstdError::Decompression(_)), "{error:?}");
    assert!(error.source().is_some());

    let mut stream = StreamDecompressor::new(Some(1000)).expect("decoder");
    assert!(matches!(
        stream.decompress(&frame),
        Err(MzstdError::Decompression(_))
    ));
    assert!(!stream.is_open());
}

#[test]
fn corrupt_input_is_a_decompression_error() {
    let mut frame = mzstd_bindings::compress(&corpus::text(5000, 7), None).expect("compress");
    let error = mzstd_bindings::decompress(b"not zstd", None).unwrap_err();
    assert!(
        error.to_string().starts_with("decompression failed: "),
        "{error}"
    );
    frame.truncate(frame.len() - 3);
    assert!(matches!(
        mzstd_bindings::decompress(&frame, None),
        Err(MzstdError::Decompression(_))
    ));
    // A stream reports truncation only once it is told the input ended.
    let mut stream = StreamDecompressor::new(None).expect("decoder");
    stream.decompress(&frame).expect("prefix is valid");
    assert!(matches!(stream.finish(), Err(MzstdError::Decompression(_))));
}

#[test]
fn invalid_levels_are_rejected_everywhere() {
    for level in [MAX_LEVEL + 1, MIN_LEVEL - 1, i32::MIN, i32::MAX] {
        let error = mzstd_bindings::compress(b"x", Some(level)).unwrap_err();
        assert!(matches!(error, MzstdError::InvalidLevel { level: l, .. } if l == level));
        assert!(error.source().is_some());
        assert!(matches!(
            mzstd_bindings::compress_with_dictionary(b"x", b"d", Some(level)),
            Err(MzstdError::InvalidLevel { .. })
        ));
        assert!(matches!(
            StreamCompressor::new(Some(level)),
            Err(MzstdError::InvalidLevel { .. })
        ));
        assert!(matches!(
            StreamCompressor::with_dictionary(b"d", Some(level)),
            Err(MzstdError::InvalidLevel { .. })
        ));
    }
}

#[test]
fn raw_dictionary_frames_match_reference_and_round_trip() {
    let dictionary = corpus::text(8_000, 8);
    let data = corpus::text(20_000, 8);
    for level in [1, 3, 19] {
        let params = Params::level(level).with_dictionary(Dictionary::auto(&dictionary));
        let expected = oracle::compress2(&data, &params).expect("reference");
        let frame = mzstd_bindings::compress_with_dictionary(&data, &dictionary, Some(level))
            .expect("compress");
        assert_eq!(frame, expected, "level {level}");
        let restored = mzstd_bindings::decompress_with_dictionary(&frame, &dictionary, None);
        assert_eq!(restored.ok(), Some(data.clone()));
    }
}

#[test]
fn trained_dictionary_round_trips_with_reference() {
    let records = corpus::records(400, 9);
    let samples: Vec<&[u8]> = records.iter().map(Vec::as_slice).collect();
    let dictionary = oracle::train_dictionary(&samples, 4096).expect("train");
    let data = &records[7];

    let frame =
        mzstd_bindings::compress_with_dictionary(data, &dictionary, None).expect("compress");
    let restored =
        oracle::decompress_with_dictionary(&frame, data.len(), &Dictionary::auto(&dictionary));
    assert_eq!(restored.as_ref(), Ok(data));

    let reference = oracle::compress2(
        data,
        &Params::level(DEFAULT_LEVEL).with_dictionary(Dictionary::auto(&dictionary)),
    )
    .expect("reference");
    assert_eq!(
        mzstd_bindings::decompress_with_dictionary(&reference, &dictionary, None).ok(),
        Some(data.clone())
    );
}

#[test]
fn dictionary_streams_round_trip() {
    let dictionary = corpus::text(4_000, 10);
    let data = corpus::text(30_000, 10);
    let mut encoder = StreamCompressor::with_dictionary(&dictionary, Some(5)).expect("encoder");
    let mut frame = Vec::new();
    for piece in data.chunks(999) {
        frame.extend(encoder.compress(piece).expect("compress"));
    }
    frame.extend(encoder.finish().expect("finish"));
    let expected = oracle::compress2(
        &data,
        &Params::level(5).with_dictionary(Dictionary::auto(&dictionary)),
    )
    .expect("reference");
    let reference_restored =
        oracle::decompress_with_dictionary(&frame, data.len(), &Dictionary::auto(&dictionary));
    assert_eq!(reference_restored.as_ref(), Ok(&data));
    assert_eq!(
        mzstd_bindings::decompress_with_dictionary(&expected, &dictionary, None).ok(),
        Some(data.clone())
    );

    let mut decoder = StreamDecompressor::with_dictionary(&dictionary, None).expect("decoder");
    let mut restored = Vec::new();
    for piece in frame.chunks(13) {
        restored.extend(decoder.decompress(piece).expect("decompress"));
    }
    restored.extend(decoder.finish().expect("finish"));
    assert_eq!(restored, data);
}

#[test]
fn malformed_trained_dictionary_is_a_dictionary_error() {
    // The magic number marks a trained dictionary, whose tables follow.
    let malformed = [0x37, 0xa4, 0x30, 0xec, 1, 0, 0, 0, 0xff];
    let checks: [(&str, Result<(), MzstdError>); 4] = [
        (
            "compress",
            mzstd_bindings::compress_with_dictionary(b"x", &malformed, None).map(drop),
        ),
        (
            "decompress",
            mzstd_bindings::decompress_with_dictionary(b"x", &malformed, None).map(drop),
        ),
        (
            "encoder",
            StreamCompressor::with_dictionary(&malformed, None).map(drop),
        ),
        (
            "decoder",
            StreamDecompressor::with_dictionary(&malformed, None).map(drop),
        ),
    ];
    for (name, result) in checks {
        let error = result.unwrap_err();
        assert!(
            matches!(error, MzstdError::Dictionary(_)),
            "{name}: {error:?}"
        );
        assert!(
            error.to_string().starts_with("invalid dictionary: "),
            "{name}: {error}"
        );
    }
}

#[test]
fn streams_close_after_finish() {
    let mut encoder = StreamCompressor::new(None).expect("encoder");
    let frame = encoder.finish().expect("finish");
    assert!(!encoder.is_open());
    for result in [encoder.compress(b"x"), encoder.flush(), encoder.finish()] {
        assert!(matches!(result, Err(MzstdError::StreamClosed)));
    }
    assert_eq!(
        mzstd_bindings::decompress(&frame, None).ok(),
        Some(Vec::new())
    );

    let mut decoder = StreamDecompressor::new(None).expect("decoder");
    decoder.decompress(&frame).expect("decompress");
    assert_eq!(decoder.finish().ok(), Some(Vec::new()));
    assert!(!decoder.is_open());
    for result in [decoder.decompress(&frame), decoder.finish()] {
        assert!(matches!(result, Err(MzstdError::StreamClosed)));
    }
    assert_eq!(
        MzstdError::StreamClosed.to_string(),
        "the stream is finished or failed; create a new one"
    );
}

#[test]
fn streams_close_after_a_failure() {
    let mut decoder = StreamDecompressor::new(None).expect("decoder");
    assert!(matches!(
        decoder.decompress(b"garbage!"),
        Err(MzstdError::Decompression(_))
    ));
    assert!(matches!(
        decoder.decompress(b""),
        Err(MzstdError::StreamClosed)
    ));
}

#[test]
fn compress_bound_matches_reference() {
    for size in [0, 1, 1000, 1 << 20, 1 << 30] {
        assert_eq!(
            mzstd_bindings::compress_bound(size).ok(),
            Some(oracle::compress_bound(size))
        );
    }
    let error = mzstd_bindings::compress_bound(usize::MAX).unwrap_err();
    assert!(matches!(
        error,
        MzstdError::InputTooLarge {
            input_size: usize::MAX
        }
    ));
    assert_eq!(
        error.to_string(),
        format!("input of {} bytes is too large to compress", usize::MAX)
    );
}
