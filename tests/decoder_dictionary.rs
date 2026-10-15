#![cfg(feature = "decompression")]
//! Dictionary decoding, differential against the reference (zstd 1.5.7):
//! frames the reference writes with raw-content and trained dictionaries
//! decode to their input through every entry point, dictionary IDs are
//! checked as `ZSTD_decodeFrameHeader` checks them, and dictionaries are
//! rejected exactly when `ZSTD_DCtx_loadDictionary_advanced` rejects them.

mod decoder_support;

use std::sync::{Arc, OnceLock};

use decoder_support::{Rng, skippable};
use mzstd::dictionary::{DecodeDictionary, DictionaryContent, DictionaryError, DictionaryRef};
use mzstd::{Backend, DecodeError, DecodeOperation, DecoderStatus, Decompressor, InvalidDataKind};
use zstd_ffi::corpus;
use zstd_ffi::oracle::{self, Dictionary, Params};

/// A dictionary trained by the reference on JSON-like records.
fn trained() -> &'static [u8] {
    static TRAINED: OnceLock<Vec<u8>> = OnceLock::new();
    TRAINED.get_or_init(|| train(1))
}

/// A dictionary trained on records generated from `seed`.
fn train(seed: u64) -> Vec<u8> {
    let samples = corpus::records(2_000, seed);
    let samples: Vec<&[u8]> = samples.iter().map(Vec::as_slice).collect();
    oracle::train_dictionary(&samples, 16 << 10).expect("the reference trains")
}

/// Raw-content dictionary bytes: English-like text.
fn raw_text() -> &'static [u8] {
    static RAW: OnceLock<Vec<u8>> = OnceLock::new();
    RAW.get_or_init(|| corpus::text(48 << 10, 7))
}

/// `count` records generated from `seed`, joined.
fn records(count: usize, seed: u64) -> Vec<u8> {
    corpus::records(count, seed).concat()
}

/// Compresses `input` with the reference, `dictionary` loaded as
/// `content_type`, at `level` with extra `params`.
fn compress(
    input: &[u8],
    dictionary: &Dictionary,
    level: i32,
    extra: &[(zstd_ffi::ZSTD_cParameter, i32)],
) -> Vec<u8> {
    let mut params = Params::level(level);
    for &(param, value) in extra {
        params = params.with(param, value);
    }
    oracle::compress2(input, &params.with_dictionary(dictionary.clone()))
        .expect("the reference compresses")
}

/// Decodes `input` in pieces of random sizes through `session`'s
/// `process`, draining output through a buffer of random size.
fn drive(
    rng: &mut Rng,
    input: &[u8],
    mut process: impl FnMut(
        &[u8],
        &mut [u8],
        DecodeOperation,
    ) -> Result<(usize, usize, DecoderStatus), DecodeError>,
) -> Result<Vec<u8>, DecodeError> {
    let mut output = Vec::new();
    let mut position = 0;
    let mut finishing = false;
    loop {
        let end = if finishing {
            input.len()
        } else {
            (position + 1 + rng.below(4_096)).min(input.len())
        };
        finishing |= end == input.len();
        let operation = if finishing {
            DecodeOperation::Finish
        } else {
            DecodeOperation::Process
        };
        let mut buffer = vec![0u8; 1 + rng.below(8_192)];
        let (consumed, produced, status) = process(&input[position..end], &mut buffer, operation)?;
        position += consumed;
        output.extend_from_slice(&buffer[..produced]);
        if status == DecoderStatus::Finished {
            return Ok(output);
        }
    }
}

/// Decodes `frame` with `dictionary` through every entry point, requiring
/// each to produce `expected`.
fn assert_decodes(dictionary: &Arc<DecodeDictionary>, frame: &[u8], expected: &[u8], label: &str) {
    let mut decoder = Decompressor::default();
    assert_eq!(
        decoder
            .decompress_with_dictionary(&**dictionary, frame)
            .unwrap_or_else(|error| panic!("{label}: {error}")),
        expected,
        "{label}: decompress_with_dictionary"
    );
    let mut appended = b"prefix".to_vec();
    let range = decoder
        .decompress_with_dictionary_into(&**dictionary, frame, &mut appended)
        .expect(label);
    assert_eq!(&appended[range], expected, "{label}: into");
    let mut slice = vec![0xa5u8; expected.len() + 3];
    let written = decoder
        .decompress_with_dictionary_to_slice(&**dictionary, frame, &mut slice)
        .expect(label);
    assert_eq!(&slice[..written], expected, "{label}: to_slice");
    assert_eq!(&slice[written..], &[0xa5; 3], "{label}: slice suffix");

    let mut rng = Rng::new(expected.len() as u64 ^ frame.len() as u64);
    let mut session = decoder.start_with_dictionary(&**dictionary);
    let streamed = drive(&mut rng, frame, |input, output, operation| {
        session
            .process(input, output, operation)
            .map(|progress| (progress.consumed, progress.produced, progress.status))
            .map_err(mzstd::DecodeFailure::into_error)
    })
    .expect(label);
    assert_eq!(streamed, expected, "{label}: session");

    let mut owned = Decompressor::default().into_session_with_dictionary(Arc::clone(dictionary));
    let streamed = drive(&mut rng, frame, |input, output, operation| {
        owned
            .process(input, output, operation)
            .map(|progress| (progress.consumed, progress.produced, progress.status))
            .map_err(mzstd::DecodeFailure::into_error)
    })
    .expect(label);
    assert_eq!(streamed, expected, "{label}: owned session");
    assert!(owned.is_finished());

    assert_adapters(&mut decoder, dictionary, frame, expected, label);
}

/// The `std::io` adapters decode `frame` with `dictionary` to `expected`.
#[cfg(not(feature = "no_std"))]
fn assert_adapters(
    decoder: &mut Decompressor,
    dictionary: &DecodeDictionary,
    frame: &[u8],
    expected: &[u8],
    label: &str,
) {
    use std::io::{Read, Write};
    let mut read = Vec::new();
    decoder
        .reader_with_dictionary(frame, dictionary)
        .read_to_end(&mut read)
        .expect(label);
    assert_eq!(read, expected, "{label}: reader");
    let mut writer = decoder.writer_with_dictionary(Vec::new(), dictionary);
    for piece in frame.chunks(777) {
        writer.write_all(piece).expect(label);
    }
    let written = writer
        .finish()
        .map_err(mzstd::io::FinishError::into_error)
        .expect(label);
    assert_eq!(written, expected, "{label}: writer");
}

/// Without `std` there are no adapters to check.
#[cfg(feature = "no_std")]
fn assert_adapters(_: &mut Decompressor, _: &DecodeDictionary, _: &[u8], _: &[u8], _: &str) {}

/// The levels a matrix covers: every one in release builds.
fn levels() -> Vec<i32> {
    if cfg!(debug_assertions) {
        vec![-5, 1, 3, 9, 19]
    } else {
        (-5..=22).filter(|&level| level != 0).collect()
    }
}

#[test]
fn trained_dictionary_frames_decode_at_every_level() {
    let bytes = trained();
    let dictionary = Arc::new(DecodeDictionary::try_from(bytes).expect("valid"));
    assert_eq!(
        dictionary.id().map(u32::from),
        Some(oracle::dict_id_from_dict(bytes))
    );
    let reference = Dictionary::auto(bytes);
    // Small inputs make the reference attach the dictionary, inputs past
    // 128 KiB make it copy the dictionary into its window.
    let inputs = [
        records(1, 11),
        records(12, 12),
        records(80, 13),
        records(2_500, 14),
    ];
    assert!(inputs[3].len() > 128 << 10);
    for level in levels() {
        for input in &inputs {
            let frame = compress(input, &reference, level, &[]);
            assert_eq!(
                oracle::dict_id_from_frame(&frame),
                oracle::dict_id_from_dict(bytes)
            );
            let label = format!("trained, level {level}, {} bytes", input.len());
            assert_eq!(
                oracle::decompress_with_dictionary(&frame, input.len(), &reference)
                    .expect("the reference decodes"),
                *input
            );
            assert_decodes(&dictionary, &frame, input, &label);
        }
    }
}

#[test]
fn raw_dictionary_frames_decode_at_every_level() {
    let bytes = raw_text();
    let dictionary = Arc::new(DecodeDictionary::try_from(bytes).expect("raw"));
    assert_eq!(dictionary.id(), None);
    let reference = Dictionary::auto(bytes);
    let inputs = [
        corpus::text(200, 21),
        corpus::text(9_000, 22),
        corpus::text(200 << 10, 23),
    ];
    for level in levels() {
        for input in &inputs {
            let frame = compress(input, &reference, level, &[]);
            assert_eq!(oracle::dict_id_from_frame(&frame), 0);
            let label = format!("raw, level {level}, {} bytes", input.len());
            assert_decodes(&dictionary, &frame, input, &label);
        }
    }
}

#[test]
fn attached_copied_and_loaded_dictionaries_decode_alike() {
    let bytes = trained();
    let dictionary = Arc::new(DecodeDictionary::try_from(bytes).expect("valid"));
    let reference = Dictionary::auto(bytes);
    let input = records(300, 31);
    for mode in [
        zstd_ffi::ZSTD_dictForceAttach,
        zstd_ffi::ZSTD_dictForceCopy,
        zstd_ffi::ZSTD_dictForceLoad,
    ] {
        for level in [-1, 1, 5, 12, 19] {
            let frame = compress(
                &input,
                &reference,
                level,
                &[(zstd_ffi::ZSTD_c_forceAttachDict, mode)],
            );
            assert_decodes(
                &dictionary,
                &frame,
                &input,
                &format!("mode {mode} level {level}"),
            );
        }
    }
}

#[test]
fn checksums_unknown_sizes_and_small_windows_decode() {
    let bytes = trained();
    let dictionary = Arc::new(DecodeDictionary::try_from(bytes).expect("valid"));
    let reference = Dictionary::auto(bytes);
    let input = records(1_500, 41);
    for (level, extra) in [
        (3, vec![(zstd_ffi::ZSTD_c_checksumFlag, 1)]),
        (3, vec![(zstd_ffi::ZSTD_c_contentSizeFlag, 0)]),
        // A window smaller than the input: the dictionary is referenced
        // only from the first window's blocks.
        (
            6,
            vec![
                (zstd_ffi::ZSTD_c_windowLog, 10),
                (zstd_ffi::ZSTD_c_contentSizeFlag, 0),
            ],
        ),
        (
            19,
            vec![
                (zstd_ffi::ZSTD_c_windowLog, 12),
                (zstd_ffi::ZSTD_c_checksumFlag, 1),
            ],
        ),
        (
            9,
            vec![
                (zstd_ffi::ZSTD_c_enableLongDistanceMatching, 1),
                (zstd_ffi::ZSTD_c_windowLog, 17),
            ],
        ),
    ] {
        let frame = compress(&input, &reference, level, &extra);
        assert_decodes(
            &dictionary,
            &frame,
            &input,
            &format!("level {level} {extra:?}"),
        );
    }
}

#[test]
fn every_backend_decodes_dictionary_frames_alike() {
    let bytes = trained();
    let dictionary = DecodeDictionary::try_from(bytes).expect("valid");
    let reference = Dictionary::auto(bytes);
    let input = records(400, 51);
    let frames: Vec<Vec<u8>> = [1, 4, 16]
        .into_iter()
        .map(|level| compress(&input, &reference, level, &[]))
        .collect();
    for backend in Backend::available() {
        let mut decoder = Decompressor::builder(Default::default())
            .with_backend(backend)
            .build();
        for frame in &frames {
            assert_eq!(
                decoder
                    .decompress_with_dictionary(&dictionary, frame)
                    .expect("valid"),
                input,
                "{backend:?}"
            );
        }
    }
}

#[test]
fn every_frame_of_a_concatenation_starts_from_the_dictionary() {
    let bytes = trained();
    let dictionary = Arc::new(DecodeDictionary::try_from(bytes).expect("valid"));
    let reference = Dictionary::auto(bytes);
    let parts = [records(3, 61), records(40, 62), records(5, 63)];
    let mut input = Vec::new();
    let mut expected = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        input.extend_from_slice(&compress(part, &reference, 3 + index as i32, &[]));
        input.extend_from_slice(&skippable(index as u8, b"skip"));
        expected.extend_from_slice(part);
    }
    // A frame compressed without any dictionary decodes with one too.
    let plain = records(4, 64);
    input.extend_from_slice(&oracle::compress(&plain, 3));
    expected.extend_from_slice(&plain);
    assert_eq!(
        oracle::decompress_stream_with_dictionary(&input, 1_000, None, Some(&reference))
            .expect("the reference decodes"),
        expected
    );
    assert_decodes(&dictionary, &input, &expected, "concatenated");
}

#[test]
fn a_frame_naming_a_dictionary_needs_that_dictionary() {
    let bytes = trained();
    let other_bytes = train(2);
    assert_ne!(
        oracle::dict_id_from_dict(bytes),
        oracle::dict_id_from_dict(&other_bytes)
    );
    let reference = Dictionary::auto(bytes);
    let input = records(20, 71);
    let frame = compress(&input, &reference, 3, &[]);
    let frame_id = oracle::dict_id_from_frame(&frame);
    let mut decoder = Decompressor::default();

    // Without a dictionary.
    assert!(oracle::decompress(&frame, 1 << 20).is_err());
    assert!(matches!(
        decoder.decompress(&frame),
        Err(DecodeError::DictionaryUnsupported { id }) if id == frame_id
    ));
    // With another trained dictionary.
    let other = DecodeDictionary::try_from(&other_bytes[..]).expect("valid");
    assert!(
        oracle::decompress_with_dictionary(&frame, 1 << 20, &Dictionary::auto(&other_bytes))
            .is_err()
    );
    let error = decoder
        .decompress_with_dictionary(&other, &frame)
        .expect_err("wrong dictionary");
    assert!(matches!(
        error,
        DecodeError::DictionaryMismatch { expected, frame }
            if expected == other.id() && u32::from(frame) == frame_id
    ));
    let mut output = b"keep".to_vec();
    assert!(
        decoder
            .decompress_with_dictionary_into(&other, &frame, &mut output)
            .is_err()
    );
    assert_eq!(output, b"keep");
    let mut slice = [0u8; 64];
    assert!(
        decoder
            .decompress_with_dictionary_to_slice(&other, &frame, &mut slice)
            .is_err()
    );
    // With raw content, which has no ID, and with an empty dictionary.
    for raw in [
        DecodeDictionary::new(bytes, DictionaryContent::Raw).expect("raw"),
        DecodeDictionary::try_from(&[][..]).expect("empty"),
    ] {
        assert!(matches!(
            decoder.decompress_with_dictionary(&raw, &frame),
            Err(DecodeError::DictionaryMismatch { expected: None, .. })
        ));
    }
    let raw_reference = Dictionary {
        bytes: bytes.to_vec(),
        content_type: zstd_ffi::ZSTD_dct_rawContent,
    };
    assert!(oracle::decompress_with_dictionary(&frame, 1 << 20, &raw_reference).is_err());
    // Sessions report it with no progress past the header.
    let mut session = decoder.start_with_dictionary(&other);
    let failure = session
        .process(&frame, &mut [0; 64], DecodeOperation::Finish)
        .expect_err("wrong dictionary");
    assert_eq!(failure.produced, 0);
    assert!(matches!(
        failure.error,
        DecodeError::DictionaryMismatch { .. }
    ));
}

#[test]
fn raw_content_reads_zstd_format_bytes_as_history() {
    let bytes = trained();
    let raw = Arc::new(DecodeDictionary::new(bytes, DictionaryContent::Raw).expect("raw"));
    assert_eq!(raw.id(), None);
    assert_eq!(raw.content_len(), bytes.len());
    let reference = Dictionary {
        bytes: bytes.to_vec(),
        content_type: zstd_ffi::ZSTD_dct_rawContent,
    };
    for (level, input) in [(1, records(10, 81)), (19, records(600, 82))] {
        let frame = compress(&input, &reference, level, &[]);
        assert_eq!(oracle::dict_id_from_frame(&frame), 0);
        assert_decodes(&raw, &frame, &input, &format!("raw content level {level}"));
    }
}

/// Whether the first compressed block of `frame` reuses inherited entropy:
/// treeless literals or a repeated sequence table.
fn first_block_inherits_entropy(frame: &[u8]) -> bool {
    let descriptor = frame[4];
    let single_segment = descriptor & 0x20 != 0;
    let dictionary = [0, 1, 2, 4][usize::from(descriptor & 3)];
    let content_size = match descriptor >> 6 {
        0 => usize::from(single_segment),
        1 => 2,
        2 => 4,
        _ => 8,
    };
    let mut position = 5 + usize::from(!single_segment) + dictionary + content_size;
    loop {
        let header =
            u32::from_le_bytes([frame[position], frame[position + 1], frame[position + 2], 0]);
        let kind = (header >> 1) & 3;
        let size = (header >> 3) as usize;
        let block = &frame[position + 3..];
        if kind == 2 {
            let literals = block[0];
            if literals & 3 == 3 {
                return true;
            }
            let size_format = (literals >> 2) & 3;
            let section = match literals & 3 {
                0 | 1 => {
                    let (header_size, shift) = match size_format {
                        0 | 2 => (1, 3),
                        1 => (2, 4),
                        _ => (3, 4),
                    };
                    let value = u32::from_le_bytes([block[0], block[1], block[2], 0])
                        & ((1 << (8 * header_size)) - 1);
                    let regenerated = (value >> shift) as usize;
                    header_size + if literals & 3 == 0 { regenerated } else { 1 }
                }
                _ => {
                    let (header_size, bits) = match size_format {
                        0 | 1 => (3, 10),
                        2 => (4, 14),
                        _ => (5, 18),
                    };
                    let value = u64::from_le_bytes([
                        block[0], block[1], block[2], block[3], block[4], 0, 0, 0,
                    ]);
                    let compressed = (value >> (4 + bits)) & ((1 << bits) - 1);
                    header_size + compressed as usize
                }
            };
            let sequences = &block[section..];
            let (count, used) = match sequences[0] {
                0 => return false,
                1..128 => (1, 1),
                128..255 => (1, 2),
                255 => (1, 3),
            };
            let _ = count;
            let modes = sequences[used];
            return [modes >> 6, modes >> 4, modes >> 2]
                .iter()
                .any(|mode| mode & 3 == 3);
        }
        if header & 1 != 0 {
            return false;
        }
        position += 3 + if kind == 1 { 1 } else { size };
    }
}

#[test]
fn trained_entropy_seeds_the_first_block() {
    let bytes = trained();
    let dictionary = DecodeDictionary::try_from(bytes).expect("valid");
    // Without its entropy, the same bytes are only history.
    let history_only = DecodeDictionary::new(bytes, DictionaryContent::Raw).expect("raw");
    let reference = Dictionary::auto(bytes);
    let raw_reference = Dictionary {
        bytes: bytes.to_vec(),
        content_type: zstd_ffi::ZSTD_dct_rawContent,
    };
    let mut inheriting = 0;
    let mut decoder = Decompressor::default();
    for (seed, count) in [(91, 1), (92, 2), (93, 4), (94, 8), (95, 16), (96, 64)] {
        let input = records(count, seed);
        for level in [1, 3, 7, 19] {
            // No dictionary ID, so that the history-only dictionary is not
            // rejected before the block.
            let frame = compress(
                &input,
                &reference,
                level,
                &[(zstd_ffi::ZSTD_c_dictIDFlag, 0)],
            );
            assert_eq!(
                decoder
                    .decompress_with_dictionary(&dictionary, &frame)
                    .expect("valid"),
                input
            );
            if !first_block_inherits_entropy(&frame) {
                continue;
            }
            inheriting += 1;
            let ours = decoder.decompress_with_dictionary(&history_only, &frame);
            let theirs = oracle::decompress_with_dictionary(&frame, 1 << 20, &raw_reference);
            assert!(theirs.is_err(), "the reference needs the tables too");
            assert!(
                matches!(
                    ours,
                    Err(DecodeError::InvalidData {
                        kind: InvalidDataKind::LiteralsSection | InvalidDataKind::SequencesSection
                    })
                ),
                "{ours:?}"
            );
        }
    }
    assert!(inheriting > 0, "some frames reuse the dictionary's tables");
}

/// Where a trained dictionary's repeat offsets start: ZDICT writes the
/// format's initial offsets 1, 4 and 8 right after the entropy tables.
fn repeat_offsets_at(bytes: &[u8]) -> usize {
    let initial: Vec<u8> = [1u32, 4, 8]
        .iter()
        .flat_map(|rep| rep.to_le_bytes())
        .collect();
    (8..bytes.len() - 12)
        .find(|&at| bytes[at..at + 12] == initial[..])
        .expect("ZDICT's repeat offsets")
}

/// Whether mzstd and the reference agree on accepting `bytes` as
/// `content`.
fn assert_same_verdict(bytes: &[u8], content: DictionaryContent, label: &str) -> bool {
    let content_type = match content {
        DictionaryContent::Raw => zstd_ffi::ZSTD_dct_rawContent,
        DictionaryContent::Zstd => zstd_ffi::ZSTD_dct_fullDict,
        _ => zstd_ffi::ZSTD_dct_auto,
    };
    let theirs = oracle::load_decoder_dictionary(&Dictionary {
        bytes: bytes.to_vec(),
        content_type,
    });
    let ours = DecodeDictionary::new(bytes, content);
    assert_eq!(
        ours.is_ok(),
        theirs.is_ok(),
        "{label}: {ours:?} vs {theirs:?}"
    );
    if let Err(error) = ours {
        let expected = if content == DictionaryContent::Zstd
            && (bytes.len() < 8 || bytes[..4] != 0xEC30_A437u32.to_le_bytes())
        {
            DictionaryError::NotZstdFormat
        } else {
            DictionaryError::Corrupted
        };
        assert_eq!(error, expected, "{label}");
    }
    theirs.is_ok()
}

#[test]
fn corrupted_dictionaries_are_rejected_exactly_when_the_reference_rejects_them() {
    let bytes = trained();
    let contents = [
        DictionaryContent::Auto,
        DictionaryContent::Zstd,
        DictionaryContent::Raw,
    ];
    // Every truncation through the entropy section, and some beyond.
    let reps = repeat_offsets_at(bytes);
    for cut in (0..reps + 40).chain([bytes.len() - 1, bytes.len()]) {
        for content in contents {
            assert_same_verdict(&bytes[..cut], content, &format!("cut {cut} {content:?}"));
        }
    }
    // Zero, largest valid and oversized repeat offsets.
    let content_len = (bytes.len() - reps - 12) as u32;
    for index in 0..3 {
        for value in [0, 1, content_len, content_len + 1, u32::MAX] {
            let mut corrupt = bytes.to_vec();
            corrupt[reps + 4 * index..reps + 4 * index + 4].copy_from_slice(&value.to_le_bytes());
            let accepted = assert_same_verdict(
                &corrupt,
                DictionaryContent::Auto,
                &format!("rep {index} = {value}"),
            );
            assert_eq!(
                accepted,
                (1..=content_len).contains(&value),
                "rep {index} = {value}"
            );
        }
    }
    // Random byte changes across the entropy section.
    let input = records(3, 111);
    let frame = compress(
        &input,
        &Dictionary::auto(bytes),
        3,
        &[(zstd_ffi::ZSTD_c_dictIDFlag, 0)],
    );
    let mut rng = Rng::new(0xd1c7);
    for round in 0..2_000 {
        let mut corrupt = bytes.to_vec();
        for _ in 0..1 + rng.below(3) {
            let at = 8 + rng.below(reps + 12 - 8);
            corrupt[at] = rng.next_u64() as u8;
        }
        if assert_same_verdict(&corrupt, DictionaryContent::Auto, &format!("round {round}")) {
            // Accepted alike, a frame decodes alike or mzstd rejects it.
            let dictionary = DecodeDictionary::try_from(&corrupt[..]).expect("accepted");
            if let Ok(output) =
                Decompressor::default().decompress_with_dictionary(&dictionary, &frame)
            {
                let reference = Dictionary::auto(&corrupt);
                assert_eq!(
                    oracle::decompress_with_dictionary(&frame, 1 << 20, &reference).as_ref(),
                    Ok(&output),
                    "round {round}"
                );
            }
        }
    }
}

#[test]
fn dictionary_references_are_borrowed_views() {
    let dictionary = DecodeDictionary::try_from(raw_text()).expect("raw");
    let view = DictionaryRef::from(&dictionary);
    let copy = view;
    assert!(matches!(copy, DictionaryRef::Decode(inner) if std::ptr::eq(inner, &dictionary)));
    assert!(format!("{view:?}").contains("DecodeDictionary"));
    assert_eq!(dictionary.retained_bytes(), raw_text().len());
    let trained = DecodeDictionary::try_from(trained()).expect("valid");
    assert!(trained.retained_bytes() > trained.content_len());
}

#[test]
fn owned_sessions_keep_their_dictionary_across_reinit() {
    let bytes = trained();
    let reference = Dictionary::auto(bytes);
    let input = records(30, 101);
    let frame = compress(&input, &reference, 5, &[]);
    let dictionary = DecodeDictionary::try_from(bytes).expect("valid");
    let mut session = Decompressor::default().into_session_with_dictionary(dictionary);
    for _ in 0..2 {
        let mut output = vec![0u8; input.len()];
        let progress = session
            .process(&frame, &mut output, DecodeOperation::Finish)
            .expect("valid");
        assert_eq!(progress.status, DecoderStatus::Finished);
        assert_eq!(output, input);
        session.reinit();
    }
    // The decoder handed back has no dictionary.
    let mut decoder = session.into_decompressor();
    assert!(matches!(
        decoder.decompress(&frame),
        Err(DecodeError::DictionaryUnsupported { .. })
    ));
    // A `&'static` dictionary works too.
    let leaked: &'static DecodeDictionary =
        Box::leak(Box::new(DecodeDictionary::try_from(bytes).expect("valid")));
    let mut session = decoder.into_session_with_dictionary(leaked);
    let mut output = vec![0u8; input.len()];
    session
        .process(&frame, &mut output, DecodeOperation::Finish)
        .expect("valid");
    assert_eq!(output, input);
}
