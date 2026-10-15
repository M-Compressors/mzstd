#![cfg(feature = "compression")]
//! Compression with dictionaries through the public API writes the
//! reference's bytes: `ZSTD_CCtx_loadDictionary_advanced` followed by
//! `ZSTD_compress2` or `ZSTD_compressStream2`.

use std::sync::Arc;

use mzstd::dictionary::{
    DictionaryBuilder, DictionaryContent, DictionaryError, PreparedDictionary,
};
use mzstd::{
    CompressionParams, Compressor, ConfigError, EncodeError, EncoderConfig, EncoderStatus,
    InputSize, Level, Operation, Strategy, StreamConfig,
};
use zstd_ffi::corpus;
use zstd_ffi::oracle::{self, Dictionary, Directive, Params};
use zstd_ffi::*;

fn level(value: i32) -> EncoderConfig {
    EncoderConfig::default().with_level(Level::try_from(value).expect("a level"))
}

fn content_type(content: DictionaryContent) -> i32 {
    match content {
        DictionaryContent::Raw => ZSTD_dct_rawContent,
        DictionaryContent::Zstd => ZSTD_dct_fullDict,
        _ => ZSTD_dct_auto,
    }
}

/// The reference's parameters for `config` with `dictionary` loaded.
fn params(config: &EncoderConfig, dictionary: &[u8], content: DictionaryContent) -> Params {
    let mut params = Params::level(config.level().get())
        .with(ZSTD_c_checksumFlag, i32::from(config.checksum()))
        .with(ZSTD_c_contentSizeFlag, i32::from(config.content_size()));
    if let Some(strategy) = config.params().strategy() {
        params = params.with(ZSTD_c_strategy, i32::from(u8::from(strategy)));
    }
    if let Some(window_log) = config.params().window_log() {
        params = params.with(ZSTD_c_windowLog, window_log as i32);
    }
    params.with_dictionary(Dictionary {
        bytes: dictionary.to_vec(),
        content_type: content_type(content),
    })
}

/// A dictionary trained on JSON-like records, and records outside its
/// training set.
fn trained() -> (Vec<u8>, Vec<Vec<u8>>) {
    let records = corpus::records(4000, 3);
    let samples: Vec<&[u8]> = records[..3000].iter().map(Vec::as_slice).collect();
    let dictionary = oracle::train_dictionary(&samples, 24 << 10).expect("trains");
    (dictionary, records[3000..].to_vec())
}

fn inputs(records: &[Vec<u8>]) -> Vec<Vec<u8>> {
    vec![
        Vec::new(),
        records[0].clone(),
        records[..20].concat(),
        records[..400].concat(),
        records.concat(),
        corpus::text(200_000, 4),
    ]
}

fn prepare(bytes: &[u8], content: DictionaryContent, config: EncoderConfig) -> PreparedDictionary {
    DictionaryBuilder::new(bytes)
        .with_content(content)
        .with_config(config)
        .build()
        .expect("prepares")
}

#[test]
fn one_shot_frames_match_the_reference_at_every_level() {
    let (trained, records) = trained();
    let raw = corpus::text(30_000, 8);
    let inputs = inputs(&records);
    for value in [-7, -1, 1, 2, 3, 4, 5, 7, 9, 12, 13, 15, 16, 18, 19, 20, 22] {
        for (bytes, content) in [
            (&trained, DictionaryContent::Auto),
            (&trained, DictionaryContent::Raw),
            (&raw, DictionaryContent::Auto),
        ] {
            let config = level(value).with_checksum(value % 2 == 0);
            let dictionary = prepare(bytes, content, config);
            let mut encoder = Compressor::new(config).expect("builds");
            for input in &inputs {
                let expected =
                    oracle::compress2(input, &params(&config, bytes, content)).expect("oracle");
                let frame = encoder
                    .compress_with_dictionary(&dictionary, input)
                    .expect("compresses");
                assert!(
                    frame == expected,
                    "level {value} {content:?} input {} bytes",
                    input.len()
                );
                let mut slice = vec![0u8; Compressor::max_compressed_size(input.len()).unwrap()];
                let written = encoder
                    .compress_with_dictionary_to_slice(&dictionary, input, &mut slice)
                    .expect("compresses");
                assert_eq!(slice[..written], expected[..]);
                let mut uninit = Vec::with_capacity(slice.len());
                let written = encoder
                    .compress_with_dictionary_to_uninit(
                        &dictionary,
                        input,
                        uninit.spare_capacity_mut(),
                    )
                    .expect("compresses");
                // SAFETY: the call initialized the first `written` bytes.
                unsafe { uninit.set_len(written) };
                assert_eq!(uninit, expected);
                let mut appended = b"prefix".to_vec();
                let range = encoder
                    .compress_with_dictionary_into(&dictionary, input, &mut appended)
                    .expect("compresses");
                assert_eq!(appended[range], expected[..]);
            }
        }
    }
}

#[test]
fn frames_name_the_dictionary_and_decode_with_the_reference() {
    let (trained, records) = trained();
    let dictionary = prepare(&trained, DictionaryContent::Auto, EncoderConfig::default());
    let id = dictionary.id().expect("a trained dictionary has an ID");
    assert_eq!(u32::from(id), oracle::dict_id_from_dict(&trained));
    let mut encoder = Compressor::default();
    let input = records[..50].concat();
    let frame = encoder
        .compress_with_dictionary(&dictionary, &input)
        .expect("compresses");
    assert_eq!(oracle::dict_id_from_frame(&frame), u32::from(id));
    let restored =
        oracle::decompress_with_dictionary(&frame, input.len(), &Dictionary::auto(&trained))
            .expect("decodes");
    assert_eq!(restored, input);
    #[cfg(feature = "decompression")]
    {
        let mut decoder = mzstd::Decompressor::default();
        let ours = decoder
            .decompress_with_dictionary(&dictionary, &frame)
            .expect("decodes with the prepared dictionary");
        assert_eq!(ours, input);
        let decode =
            mzstd::dictionary::DecodeDictionary::try_from(trained.as_slice()).expect("parses");
        assert_eq!(decode.id(), dictionary.id());
        assert_eq!(
            decoder
                .decompress_with_dictionary(&decode, &frame)
                .expect("decodes"),
            input
        );
    }
    // The dictionary pays off on small records.
    assert!(frame.len() < encoder.compress(&input).expect("compresses").len());
    // A raw dictionary names no ID.
    let raw = prepare(&trained, DictionaryContent::Raw, EncoderConfig::default());
    assert_eq!(raw.id(), None);
    let frame = encoder
        .compress_with_dictionary(&raw, &input)
        .expect("compresses");
    assert_eq!(oracle::dict_id_from_frame(&frame), 0);
}

/// Streams `input` through a session in `chunk`-byte calls with a small
/// output buffer.
fn stream(
    encoder: &mut Compressor,
    dictionary: &PreparedDictionary,
    input: &[u8],
    chunk: usize,
    size: InputSize,
) -> Vec<u8> {
    let mut session = encoder
        .start_with_dictionary(dictionary, StreamConfig::default().with_input_size(size))
        .expect("starts");
    let mut out = Vec::new();
    let mut buffer = vec![0u8; 1 << 17];
    for piece in input.chunks(chunk.max(1)) {
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
        let progress = session.finish(&mut buffer).expect("ends");
        out.extend_from_slice(&buffer[..progress.produced]);
        if progress.status == EncoderStatus::Finished {
            return out;
        }
    }
}

#[test]
fn sessions_match_the_reference_stream() {
    let (trained, records) = trained();
    for value in [1, 3, 6, 12, 17, 19] {
        let config = level(value);
        let dictionary = prepare(&trained, DictionaryContent::Auto, config);
        let mut encoder = Compressor::new(config).expect("builds");
        for input in [records[..30].concat(), records.concat()] {
            for chunk in [1000, 70_000] {
                let steps: Vec<(&[u8], Directive)> = input
                    .chunks(chunk)
                    .map(|piece| (piece, Directive::Continue))
                    .collect();
                let reference = params(&config, &trained, DictionaryContent::Auto);
                let expected = oracle::compress_stream(&reference, &steps).expect("oracle");
                let ours = stream(&mut encoder, &dictionary, &input, chunk, InputSize::Unknown);
                assert!(ours == expected, "level {value} chunk {chunk}");
                let pledged = reference.with_pledged(input.len() as u64);
                let expected = oracle::compress_stream(&pledged, &steps).expect("oracle");
                let ours = stream(
                    &mut encoder,
                    &dictionary,
                    &input,
                    chunk,
                    InputSize::Exact(input.len() as u64),
                );
                assert!(ours == expected, "level {value} chunk {chunk}, exact size");
            }
        }
    }
}

#[test]
fn owned_sessions_and_adapters_write_the_one_shot_frame() {
    let (trained, records) = trained();
    let dictionary = Arc::new(prepare(
        &trained,
        DictionaryContent::Auto,
        EncoderConfig::default(),
    ));
    let input = records[..60].concat();
    let mut encoder = Compressor::default();
    let expected = encoder
        .compress_with_dictionary(&dictionary, &input)
        .expect("compresses");
    let stream = StreamConfig::default().with_input_size(InputSize::Exact(input.len() as u64));

    let mut session = Compressor::default()
        .into_session_with_dictionary(Arc::clone(&dictionary), stream)
        .expect("starts");
    let mut out = vec![0u8; expected.len() + 64];
    let progress = session
        .process(&input, &mut out, Operation::End)
        .expect("compresses");
    assert_eq!(progress.status, EncoderStatus::Finished);
    assert_eq!(out[..progress.produced], expected[..]);
    session.reinit(stream);
    let progress = session
        .process(&input, &mut out, Operation::End)
        .expect("compresses again");
    assert_eq!(out[..progress.produced], expected[..]);
    let (_compressor, kept) = session.into_parts();
    assert!(kept.is_some_and(|kept| Arc::ptr_eq(&kept, &dictionary)));

    #[cfg(not(feature = "no_std"))]
    {
        use std::io::{Read, Write};
        let mut sink = encoder
            .writer_with_dictionary(&dictionary, Vec::new(), stream)
            .expect("starts");
        sink.write_all(&input).expect("writes");
        let written = sink.finish().map_err(mzstd::io::FinishError::into_error);
        assert_eq!(written.expect("finishes"), expected);

        let mut read = Vec::new();
        encoder
            .reader_with_dictionary(&dictionary, input.as_slice(), stream)
            .expect("starts")
            .read_to_end(&mut read)
            .expect("reads");
        assert_eq!(read, expected);
    }
}

#[test]
fn a_dictionary_serves_only_its_configuration() {
    let dictionary = prepare(
        b"some dictionary content",
        DictionaryContent::Auto,
        level(3),
    );
    // Frame options do not matter.
    let mut checksummed = Compressor::new(level(3).with_checksum(true)).expect("builds");
    assert!(
        checksummed
            .compress_with_dictionary(&dictionary, b"payload")
            .is_ok()
    );
    let mut other = Compressor::new(level(4)).expect("builds");
    assert_eq!(
        other.compress_with_dictionary(&dictionary, b"payload"),
        Err(EncodeError::DictionaryMismatch)
    );
    let mut out = Vec::from(&b"kept"[..]);
    assert!(
        other
            .compress_with_dictionary_into(&dictionary, b"payload", &mut out)
            .is_err()
    );
    assert_eq!(out, b"kept");
    assert!(
        other
            .compress_with_dictionary_to_slice(&dictionary, b"payload", &mut [0u8; 64])
            .is_err()
    );
    assert!(
        other
            .compress_with_dictionary_to_uninit(
                &dictionary,
                b"payload",
                &mut [std::mem::MaybeUninit::uninit(); 64]
            )
            .is_err()
    );
    assert_eq!(
        other
            .start_with_dictionary(&dictionary, StreamConfig::default())
            .err(),
        Some(EncodeError::DictionaryMismatch)
    );
    assert!(
        Compressor::new(level(5))
            .expect("builds")
            .into_session_with_dictionary(&dictionary, StreamConfig::default())
            .is_err()
    );
    #[cfg(not(feature = "no_std"))]
    assert_eq!(
        std::io::Error::from(mzstd::compressor::parallel::ParallelEncodeError::Encode(
            EncodeError::DictionaryMismatch
        ))
        .kind(),
        std::io::ErrorKind::InvalidInput
    );
    // A compressor reused across dictionaries and plain frames stays exact.
    let mut encoder = Compressor::new(level(3)).expect("builds");
    let plain = encoder.compress(b"payload payload").expect("compresses");
    encoder
        .compress_with_dictionary(&dictionary, b"payload payload")
        .expect("compresses");
    assert_eq!(
        encoder.compress(b"payload payload").expect("compresses"),
        plain
    );
}

#[test]
fn malformed_dictionaries_are_rejected_like_the_reference() {
    let (trained, _) = trained();
    assert_eq!(
        DictionaryBuilder::new(b"plain text")
            .with_content(DictionaryContent::Zstd)
            .build()
            .unwrap_err(),
        DictionaryError::NotZstdFormat
    );
    assert_eq!(
        DictionaryBuilder::new(b"short")
            .with_content(DictionaryContent::Zstd)
            .build()
            .unwrap_err(),
        DictionaryError::NotZstdFormat
    );
    // Truncations and corruptions are rejected exactly when the reference
    // rejects them.
    let mut rng = corpus::Rng::new(5);
    let mut cases = Vec::new();
    for len in [9, 12, 40, 200, trained.len() - 20_000] {
        cases.push(trained[..len.min(trained.len())].to_vec());
    }
    for _ in 0..300 {
        let mut broken = trained.clone();
        let at = 8 + (rng.next_u64() % 600) as usize;
        broken[at] ^= 1 << (rng.next_u64() % 8);
        cases.push(broken);
    }
    for case in cases {
        let ours = DictionaryBuilder::new(&case).build();
        let theirs = oracle::compress2(
            b"payload",
            &Params::level(3).with_dictionary(Dictionary::auto(&case)),
        );
        assert_eq!(ours.is_ok(), theirs.is_ok(), "{:?}", ours.err());
        if let Ok(dictionary) = ours {
            let frame = Compressor::default()
                .compress_with_dictionary(&dictionary, b"payload")
                .expect("compresses");
            assert_eq!(frame, theirs.expect("checked above"));
        }
    }
    let invalid =
        EncoderConfig::default().with_params(CompressionParams::default().with_window_log(40));
    let error = DictionaryBuilder::new(b"history")
        .with_config(invalid)
        .build()
        .unwrap_err();
    assert!(matches!(
        error,
        DictionaryError::Config(ConfigError::OutOfRange { .. })
    ));
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn one_dictionary_serves_threads_at_once() {
    let (trained, records) = trained();
    let dictionary = Arc::new(prepare(
        &trained,
        DictionaryContent::Auto,
        EncoderConfig::default(),
    ));
    let expected: Vec<Vec<u8>> = records[..8]
        .iter()
        .map(|record| {
            Compressor::default()
                .compress_with_dictionary(&dictionary, record)
                .expect("compresses")
        })
        .collect();
    std::thread::scope(|scope| {
        for (record, expected) in records[..8].iter().zip(&expected) {
            let dictionary = Arc::clone(&dictionary);
            scope.spawn(move || {
                let frame = Compressor::default()
                    .compress_with_dictionary(&dictionary, record)
                    .expect("compresses");
                assert_eq!(&frame, expected);
            });
        }
    });
    assert!(dictionary.retained_bytes() > trained.len());
    assert_eq!(dictionary.config(), &EncoderConfig::default());
    assert!(format!("{dictionary:?}").contains("PreparedDictionary"));
    assert_eq!(dictionary.as_bytes(), trained.as_slice());
    assert_eq!(dictionary.content(), DictionaryContent::Auto);
}

#[test]
fn explicit_strategies_with_dictionaries_match_the_reference() {
    let (trained, records) = trained();
    let input = records[..100].concat();
    for strategy in [
        Strategy::Fast,
        Strategy::DoubleFast,
        Strategy::Greedy,
        Strategy::Lazy2,
        Strategy::BtLazy2,
        Strategy::BtUltra2,
    ] {
        for window_log in [None, Some(12)] {
            let mut cparams = CompressionParams::default().with_strategy(strategy);
            if let Some(window_log) = window_log {
                cparams = cparams.with_window_log(window_log);
            }
            let config = EncoderConfig::default().with_params(cparams);
            let dictionary = prepare(&trained, DictionaryContent::Auto, config);
            let frame = Compressor::new(config)
                .expect("builds")
                .compress_with_dictionary(&dictionary, &input)
                .expect("compresses");
            let expected =
                oracle::compress2(&input, &params(&config, &trained, DictionaryContent::Auto))
                    .expect("oracle");
            assert!(frame == expected, "{strategy:?} window {window_log:?}");
        }
    }
}
