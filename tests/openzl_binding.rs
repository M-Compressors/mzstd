#![cfg(all(feature = "compression", feature = "decompression"))]
//! What OpenZL v0.3.0's zstd node needs from mzstd, checked byte for byte
//! against the reference:
//!
//! - magicless frames, both ways;
//! - every `ZSTD_cParameter` by numeric ID (`CParameter`);
//! - the node's whole encoder call sequence, including the per-element
//!   `ZSTD_e_flush` streaming with a pledged size;
//! - dictionaries made with `ZSTD_createCDict_advanced` and referenced with
//!   `ZSTD_CCtx_refCDict`, and `ZSTD_decompress_usingDDict`;
//! - `ZSTD_getCParams` and the frame header queries the decoder node uses.

use mzstd::decompressor::{FrameHeaderInfo, FrameKind};
use mzstd::dictionary::{
    DecodeDictionary, DictionaryBuilder, DictionaryContent, PreparedDictionary,
};
use mzstd::{
    CParameter, CompressionParams, Compressor, ConfigError, DecodeError, DecodeOperation,
    DecoderConfig, DecoderStatus, Decompressor, DictionaryAttach, EncoderConfig, EncoderStatus,
    FrameFormat, FrameMode, InputSize, Level, Operation, StreamConfig, Switch,
};
use zstd_ffi::corpus;
use zstd_ffi::oracle::{
    self, OpenZlCDict, OpenZlDDict, OpenZlDecode, OpenZlEncoder, OpenZlError, OpenZlJob, Params,
};
use zstd_ffi::*;

/// Levels the differential tests sweep: negative, the zero alias and all
/// positive levels.
const LEVELS: [i32; 29] = [
    -5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
    21, 22, 3,
];

/// Input sizes around the block boundaries; the large ones are kept to a few
/// levels by the callers.
const SMALL_SIZES: [usize; 8] = [0, 1, 100, 4_096, 131_071, 131_072, 131_073, 300_000];

fn input(len: usize, seed: u64) -> Vec<u8> {
    match seed % 3 {
        0 => corpus::text(len, seed),
        1 => corpus::mixed(len, seed),
        _ => corpus::binary(len, seed),
    }
}

fn level(value: i32) -> Level {
    Level::try_from(value).expect("a level")
}

/// The configuration `ZSTD_CCtx_setParameter` calls for `pairs` build.
fn config_for(pairs: &[(i32, i32)]) -> Result<EncoderConfig, ConfigError> {
    pairs
        .iter()
        .try_fold(EncoderConfig::default(), |config, &pair| {
            Ok(config.with_parameter(CParameter::try_from(pair)?))
        })
}

fn reference_params(pairs: &[(i32, i32)]) -> Params {
    pairs
        .iter()
        .fold(Params::default(), |params, &(id, value)| {
            params.with(id, value)
        })
}

// ---- magicless frames -------------------------------------------------

#[test]
fn magicless_frames_are_standard_frames_without_the_magic_at_every_level() {
    for &level_value in &LEVELS {
        for (index, &len) in SMALL_SIZES.iter().enumerate() {
            let src = input(len, index as u64);
            let pairs = [(ZSTD_c_compressionLevel, level_value), (ZSTD_c_format, 1)];
            let expected = oracle::compress2(&src, &reference_params(&pairs)).expect("reference");
            let config = config_for(&pairs).expect("accepted");
            assert_eq!(config.format(), FrameFormat::Magicless);
            let ours = Compressor::new(config)
                .expect("valid")
                .compress(&src)
                .expect("compresses");
            assert_eq!(ours, expected, "level {level_value}, {len} bytes");
            let standard = oracle::compress2(&src, &Params::level(level_value)).expect("reference");
            assert_eq!(&ours[..], &standard[4..], "magic is the only difference");
        }
    }
}

#[test]
fn magicless_frames_of_several_mebibytes_match_the_reference() {
    let src = corpus::mixed(3 << 20, 77);
    for level_value in [-5, 1, 3, 9, 19] {
        let pairs = [(ZSTD_c_compressionLevel, level_value), (ZSTD_c_format, 1)];
        let expected = oracle::compress2(&src, &reference_params(&pairs)).expect("reference");
        let ours = Compressor::new(config_for(&pairs).expect("accepted"))
            .expect("valid")
            .compress(&src)
            .expect("compresses");
        assert_eq!(ours, expected, "level {level_value}");
    }
}

#[test]
fn magicless_streaming_sessions_match_the_reference_with_every_output_path() {
    let src = corpus::text(400_000, 5);
    let config = EncoderConfig::default().with_format(FrameFormat::Magicless);
    let pairs = [(ZSTD_c_format, 1)];
    let expected = oracle::compress2(&src, &reference_params(&pairs)).expect("reference");
    let mut encoder = Compressor::new(config).expect("valid");
    let mut slice = vec![0u8; expected.len() + 64];
    let written = encoder.compress_to_slice(&src, &mut slice).expect("slice");
    assert_eq!(&slice[..written], &expected[..]);
    let mut vec = b"prefix".to_vec();
    let range = encoder.compress_into(&src, &mut vec).expect("vec");
    assert_eq!(&vec[range], &expected[..]);
    let mut spare: Vec<u8> = Vec::with_capacity(expected.len() + 64);
    let written = encoder
        .compress_to_uninit(&src, spare.spare_capacity_mut())
        .expect("uninit");
    // SAFETY: the first `written` bytes were initialized.
    unsafe { spare.set_len(written) };
    assert_eq!(spare, expected);
    for exact in [true, false] {
        let mut session = encoder.start(StreamConfig::from(if exact {
            InputSize::Exact(src.len() as u64)
        } else {
            InputSize::Unknown
        }));
        let mut output = Vec::new();
        let mut chunk = [0u8; 4096];
        for part in src.chunks(70_000) {
            let mut rest = part;
            while !rest.is_empty() {
                let progress = session
                    .process(rest, &mut chunk, Operation::Continue)
                    .expect("continues");
                output.extend_from_slice(&chunk[..progress.produced]);
                rest = &rest[progress.consumed..];
            }
        }
        loop {
            let progress = session
                .process(&[], &mut chunk, Operation::End)
                .expect("ends");
            output.extend_from_slice(&chunk[..progress.produced]);
            if progress.status == EncoderStatus::Finished {
                break;
            }
        }
        let pledged = if exact {
            Params::default().with_pledged(src.len() as u64)
        } else {
            Params::default()
        };
        let reference = oracle::compress_stream(
            &pairs.iter().fold(pledged, |p, &(id, v)| p.with(id, v)),
            &src.chunks(70_000)
                .map(|c| (c, oracle::Directive::Continue))
                .collect::<Vec<_>>(),
        )
        .expect("reference stream");
        assert_eq!(output, reference, "exact size {exact}");
    }
}

#[test]
fn magicless_frames_round_trip_and_standard_decoders_refuse_them() {
    let src = corpus::text(50_000, 6);
    let config = EncoderConfig::default().with_format(FrameFormat::Magicless);
    let frame = Compressor::new(config)
        .expect("valid")
        .compress(&src)
        .expect("compresses");
    let magicless = DecoderConfig::default().with_format(FrameFormat::Magicless);
    assert_eq!(
        Decompressor::new(magicless)
            .decompress(&frame)
            .expect("decodes"),
        src
    );
    assert!(matches!(
        Decompressor::default().decompress(&frame),
        Err(DecodeError::InvalidData { .. })
    ));
}

#[test]
fn magicless_decoding_matches_the_reference_for_every_frame_shape() {
    let magicless = DecoderConfig::default().with_format(FrameFormat::Magicless);
    let mut decoder = Decompressor::new(magicless);
    for &level_value in &[-3, 1, 3, 12, 19] {
        for (index, &len) in SMALL_SIZES.iter().enumerate() {
            for (checksum, content_size) in [(0, 1), (1, 1), (1, 0), (0, 0)] {
                let src = input(len, index as u64 + 10);
                let pairs = [
                    (ZSTD_c_compressionLevel, level_value),
                    (ZSTD_c_format, 1),
                    (ZSTD_c_checksumFlag, checksum),
                    (ZSTD_c_contentSizeFlag, content_size),
                ];
                let frame = oracle::compress2(&src, &reference_params(&pairs)).expect("frame");
                assert_eq!(decoder.decompress(&frame).expect("decodes"), src);
                // The reference decodes only frames that declare a size;
                // mzstd decodes both.
                let reference = oracle::openzl_decode(true, None, &frame, src.len().max(1));
                if content_size == 1 {
                    assert_eq!(reference.expect("reference decodes"), src);
                } else {
                    assert_eq!(reference, Err(OpenZlDecode::BadHeader));
                }
            }
        }
    }
}

#[test]
fn magicless_sessions_decode_in_every_chunking_and_frame_mode() {
    let src = corpus::mixed(200_000, 9);
    let frame = Compressor::new(EncoderConfig::default().with_format(FrameFormat::Magicless))
        .expect("valid")
        .compress(&src)
        .expect("compresses");
    for mode in [FrameMode::Single, FrameMode::Concatenated] {
        let config = DecoderConfig::default()
            .with_format(FrameFormat::Magicless)
            .with_frame_mode(mode);
        for chunk in [1usize, 7, 1000, frame.len()] {
            let mut decoder = Decompressor::new(config);
            let mut session = decoder.start();
            let mut output = Vec::new();
            let mut buffer = vec![0u8; 50_000];
            for piece in frame.chunks(chunk) {
                let mut rest = piece;
                loop {
                    let progress = session
                        .process(rest, &mut buffer, DecodeOperation::Process)
                        .expect("decodes");
                    output.extend_from_slice(&buffer[..progress.produced]);
                    rest = &rest[progress.consumed..];
                    if rest.is_empty() && progress.status != DecoderStatus::NeedsOutput {
                        break;
                    }
                }
            }
            loop {
                let progress = session.finish(&mut buffer).expect("finishes");
                output.extend_from_slice(&buffer[..progress.produced]);
                if progress.status == DecoderStatus::Finished {
                    break;
                }
            }
            assert_eq!(output, src, "{mode:?}, chunk {chunk}");
        }
    }
}

#[test]
fn concatenated_magicless_frames_decode_back_to_back() {
    let first = corpus::text(30_000, 1);
    let second = corpus::binary(20_000, 2);
    let mut encoder = Compressor::new(EncoderConfig::default().with_format(FrameFormat::Magicless))
        .expect("valid");
    let mut frames = encoder.compress(&first).expect("first");
    frames.extend(encoder.compress(&second).expect("second"));
    let config = DecoderConfig::default().with_format(FrameFormat::Magicless);
    let decoded = Decompressor::new(config)
        .decompress(&frames)
        .expect("decodes");
    assert_eq!(decoded, [first.clone(), second].concat());
    let single = config.with_frame_mode(FrameMode::Single);
    assert!(matches!(
        Decompressor::new(single).decompress(&frames),
        Err(DecodeError::TrailingData { .. })
    ));
}

#[test]
fn magicless_decoding_reads_no_skippable_frames() {
    // A skippable frame is a standard-format feature; its bytes are a header.
    let skippable = [0x50, 0x2A, 0x4D, 0x18, 0, 0, 0, 0];
    let magicless = DecoderConfig::default().with_format(FrameFormat::Magicless);
    assert!(Decompressor::new(magicless).decompress(&skippable).is_err());
    assert_eq!(
        Decompressor::default()
            .decompress(&skippable)
            .expect("skipped"),
        b""
    );
}

// ---- parameters by ID -------------------------------------------------

/// IDs the reference defines, and neighbours it does not.
fn parameter_ids() -> Vec<i32> {
    let mut ids = vec![-1, 0, 1, 9, 10, 11, 99];
    ids.extend(100..=110);
    ids.extend([129, 130, 131, 159]);
    ids.extend(160..=166);
    ids.extend([199, 200, 201, 202, 203, 399]);
    ids.extend(400..=403);
    ids.extend([499, 500, 501, 999]);
    ids.extend(1000..=1019);
    ids.extend([1020, 2000, i32::MAX, i32::MIN]);
    ids
}

fn parameter_values() -> Vec<i32> {
    let mut values = vec![
        i32::MIN,
        -131_073,
        -131_072,
        -5,
        -1,
        0,
        1,
        2,
        3,
        4,
        5,
        6,
        7,
        8,
        9,
        10,
    ];
    values.extend([
        11, 12, 16, 17, 22, 23, 24, 25, 26, 30, 31, 32, 33, 64, 1023, 1024, 1339,
    ]);
    values.extend([
        1340,
        4096,
        4097,
        131_071,
        131_072,
        131_073,
        1 << 20,
        i32::MAX,
    ]);
    values
}

#[test]
fn every_parameter_id_and_value_is_accepted_or_refused_as_the_reference_does() {
    for id in parameter_ids() {
        for value in parameter_values() {
            let reference = oracle::set_parameter(id, value);
            let ours = CParameter::try_from((id, value));
            match (&reference, &ours) {
                (Ok(()), Ok(parameter)) => {
                    // The pair survives the round trip through the typed form
                    // except for values the reference normalizes.
                    let (back_id, back_value) = <(i32, i32)>::from(*parameter);
                    assert_eq!(back_id, id, "({id}, {value})");
                    if id == ZSTD_c_targetCBlockSize {
                        // The reference raises a target to its minimum and
                        // `ZSTD_CCtx_getParameter` returns what it stored.
                        let stored = oracle::stored_parameter(id, value).expect("a stored value");
                        assert_eq!(back_value, stored, "({id}, {value})");
                    }
                }
                (Err(_), Err(_)) => {}
                _ => panic!("({id}, {value}): reference {reference:?}, ours {ours:?}"),
            }
        }
    }
}

#[test]
fn level_parameters_clamp_and_zero_means_default() {
    let max = CParameter::try_from((ZSTD_c_compressionLevel, i32::MAX)).expect("clamps");
    assert_eq!(max, CParameter::CompressionLevel(Level::MAX));
    let min = CParameter::try_from((ZSTD_c_compressionLevel, i32::MIN)).expect("clamps");
    assert_eq!(min, CParameter::CompressionLevel(Level::MIN));
    let zero = CParameter::try_from((ZSTD_c_compressionLevel, 0)).expect("accepted");
    assert_eq!(zero, CParameter::CompressionLevel(Level::DEFAULT));
    let src = corpus::text(60_000, 3);
    for value in [i32::MIN, -200_000, -131_072, 0, 23, 99, i32::MAX] {
        let pairs = [(ZSTD_c_compressionLevel, value)];
        let expected = oracle::compress2(&src, &reference_params(&pairs)).expect("reference");
        let ours = Compressor::new(config_for(&pairs).expect("accepted"))
            .expect("valid")
            .compress(&src)
            .expect("compresses");
        assert_eq!(ours, expected, "level {value}");
    }
}

#[test]
fn zero_restores_a_parameters_default_and_the_last_value_wins() {
    let src = corpus::text(80_000, 4);
    let cases: [&[(i32, i32)]; 8] = [
        &[(ZSTD_c_windowLog, 15), (ZSTD_c_windowLog, 0)],
        &[(ZSTD_c_hashLog, 12), (ZSTD_c_hashLog, 14)],
        &[
            (ZSTD_c_chainLog, 12),
            (ZSTD_c_chainLog, 0),
            (ZSTD_c_strategy, 5),
        ],
        &[(ZSTD_c_strategy, 7), (ZSTD_c_strategy, 0)],
        &[
            (ZSTD_c_targetLength, 64),
            (ZSTD_c_targetLength, 0),
            (ZSTD_c_compressionLevel, 8),
        ],
        &[(ZSTD_c_minMatch, 5), (ZSTD_c_minMatch, 0)],
        &[
            (ZSTD_c_searchLog, 3),
            (ZSTD_c_searchLog, 0),
            (ZSTD_c_compressionLevel, 7),
        ],
        &[(ZSTD_c_srcSizeHint, 1000), (ZSTD_c_srcSizeHint, 0)],
    ];
    for pairs in cases {
        let expected = oracle::compress2(&src, &reference_params(pairs)).expect("reference");
        let ours = Compressor::new(config_for(pairs).expect("accepted"))
            .expect("valid")
            .compress(&src)
            .expect("compresses");
        assert_eq!(ours, expected, "{pairs:?}");
    }
}

#[test]
fn random_parameter_lists_write_the_reference_bytes() {
    let mut rng = corpus::Rng::new(0x0123_4567);
    let ids = [
        ZSTD_c_compressionLevel,
        ZSTD_c_windowLog,
        ZSTD_c_hashLog,
        ZSTD_c_chainLog,
        ZSTD_c_searchLog,
        ZSTD_c_minMatch,
        ZSTD_c_targetLength,
        ZSTD_c_strategy,
        ZSTD_c_enableLongDistanceMatching,
        ZSTD_c_ldmHashLog,
        ZSTD_c_ldmMinMatch,
        ZSTD_c_ldmBucketSizeLog,
        ZSTD_c_ldmHashRateLog,
        ZSTD_c_contentSizeFlag,
        ZSTD_c_checksumFlag,
        ZSTD_c_dictIDFlag,
        ZSTD_c_literalCompressionMode,
        ZSTD_c_srcSizeHint,
        ZSTD_c_splitAfterSequences,
        ZSTD_c_useRowMatchFinder,
        ZSTD_c_maxBlockSize,
        ZSTD_c_blockSplitterLevel,
        ZSTD_c_format,
        ZSTD_c_forceMaxWindow,
        ZSTD_c_forceAttachDict,
        ZSTD_c_nbWorkers,
        ZSTD_c_stableInBuffer,
        1005,
        1016,
    ];
    let inputs = [
        corpus::text(200_000, 11),
        corpus::mixed(90_000, 12),
        corpus::random(20_000, 13),
        corpus::binary(1_500, 14),
    ];
    let mut compressed = 0;
    for round in 0..400 {
        let count = 1 + rng.next_u64() % 4;
        let mut pairs = Vec::new();
        for _ in 0..count {
            let id = ids[(rng.next_u64() % ids.len() as u64) as usize];
            let mut pick = |n: u64| (rng.next_u64() % n) as i32;
            let value = if id == ZSTD_c_compressionLevel {
                pick(40) - 12
            } else if [
                ZSTD_c_windowLog,
                ZSTD_c_hashLog,
                ZSTD_c_chainLog,
                ZSTD_c_ldmHashLog,
            ]
            .contains(&id)
            {
                if pick(8) == 0 { 0 } else { 6 + pick(19) }
            } else if id == ZSTD_c_searchLog {
                pick(10)
            } else if id == ZSTD_c_minMatch {
                pick(9)
            } else if id == ZSTD_c_targetLength {
                pick(600)
            } else if id == ZSTD_c_strategy {
                pick(10)
            } else if id == ZSTD_c_ldmMinMatch {
                [0, 4, 16, 64, 300][pick(5) as usize]
            } else if id == ZSTD_c_ldmBucketSizeLog {
                pick(9)
            } else if id == ZSTD_c_ldmHashRateLog {
                pick(12)
            } else if id == ZSTD_c_srcSizeHint {
                pick(300_000)
            } else if id == ZSTD_c_maxBlockSize {
                [0, 1024, 4096, 40_000, 131_072][pick(5) as usize]
            } else if id == ZSTD_c_blockSplitterLevel {
                pick(7)
            } else if id == ZSTD_c_nbWorkers {
                pick(2)
            } else {
                pick(3)
            };
            pairs.push((id, value));
        }
        let src = &inputs[(rng.next_u64() % inputs.len() as u64) as usize];
        let reference = oracle::compress2(src, &reference_params(&pairs));
        let ours = config_for(&pairs).map(|config| {
            Compressor::new(config)
                .expect("valid")
                .compress(src)
                .expect("compresses")
        });
        match (reference, ours) {
            (Ok(expected), Ok(ours)) => {
                assert_eq!(ours, expected, "round {round}: {pairs:?}");
                compressed += 1;
            }
            (Err(_), Err(_)) => {}
            (reference, ours) => panic!("round {round}: {pairs:?}: {reference:?} vs {ours:?}"),
        }
    }
    assert!(compressed > 100, "the sweep exercised {compressed} frames");
}

#[test]
fn literal_compression_modes_write_the_reference_bytes_at_every_level() {
    let src = corpus::text(120_000, 21);
    for &level_value in &LEVELS {
        for mode in 0..=2 {
            let pairs = [
                (ZSTD_c_compressionLevel, level_value),
                (ZSTD_c_literalCompressionMode, mode),
            ];
            let expected = oracle::compress2(&src, &reference_params(&pairs)).expect("reference");
            let ours = Compressor::new(config_for(&pairs).expect("accepted"))
                .expect("valid")
                .compress(&src)
                .expect("compresses");
            assert_eq!(ours, expected, "level {level_value}, mode {mode}");
        }
    }
}

#[test]
fn content_size_and_checksum_flags_follow_the_last_value() {
    let src = corpus::text(1_000, 22);
    for flags in [(0, 0), (1, 0), (0, 1), (1, 1), (7, -3)] {
        let pairs = [
            (ZSTD_c_contentSizeFlag, flags.0),
            (ZSTD_c_checksumFlag, flags.1),
        ];
        let expected = oracle::compress2(&src, &reference_params(&pairs)).expect("reference");
        let ours = Compressor::new(config_for(&pairs).expect("accepted"))
            .expect("valid")
            .compress(&src)
            .expect("compresses");
        assert_eq!(ours, expected, "{flags:?}");
    }
}

#[test]
fn the_memory_cap_lowers_the_largest_logs() {
    for (id, max) in [
        (ZSTD_c_windowLog, 28),
        (ZSTD_c_hashLog, 27),
        (ZSTD_c_chainLog, 27),
        (ZSTD_c_ldmHashLog, 27),
    ] {
        let at = CParameter::try_from((id, max)).expect("valid");
        assert_eq!(at.check_memory_cap(3), Ok(at));
        let above = CParameter::try_from((id, max + 1)).expect("valid");
        assert!(matches!(
            above.check_memory_cap(3),
            Err(ConfigError::OutOfRange { max: limit, .. }) if limit == i64::from(max)
        ));
        let default = CParameter::try_from((id, 0)).expect("valid");
        assert_eq!(default.check_memory_cap(3), Ok(default));
    }
    let level = CParameter::CompressionLevel(Level::MAX);
    assert_eq!(level.check_memory_cap(3), Ok(level));
}

// ---- OpenZL's encoder node --------------------------------------------

/// OpenZL's zstd encoder node, written against mzstd.
fn rust_node(
    src: &[u8],
    job: &OpenZlJob<'_>,
    dictionary: Option<&PreparedDictionary>,
) -> Result<Vec<u8>, OpenZlError> {
    let mut config = EncoderConfig::default();
    if job.magicless {
        config = config.with_format(FrameFormat::Magicless);
    }
    config = config.with_parameter(
        CParameter::try_from((ZSTD_c_compressionLevel, job.compression_level))
            .map_err(|e| OpenZlError::Zstd(e.to_string()))?,
    );
    if job.decompression_level == 1 {
        config = config.with_literal_compression(Switch::Disabled);
    }
    for &pair in job.params {
        let parameter = CParameter::try_from(pair).map_err(|e| OpenZlError::Zstd(e.to_string()))?;
        if matches!(
            parameter,
            CParameter::Format(_) | CParameter::ContentSizeFlag(_)
        ) {
            return Err(OpenZlError::PolicyRejected);
        }
        let parameter = parameter
            .check_memory_cap(3)
            .map_err(|_| OpenZlError::PolicyRejected)?;
        config = config.with_parameter(parameter);
    }
    let mut compressor = Compressor::new(config).map_err(|e| OpenZlError::Zstd(e.to_string()))?;
    let nb_elts = src.len() / job.elt_width.max(1);
    let split = nb_elts > 0 && job.elt_width >= 1024 && nb_elts <= 8;
    if !split {
        let frame = match dictionary {
            Some(dictionary) => compressor.compress_with_dictionary(dictionary, src),
            None => compressor.compress(src),
        };
        return frame.map_err(|e| OpenZlError::Zstd(e.to_string()));
    }
    let mut session = compressor.start(InputSize::Exact(src.len() as u64).into());
    let mut output = Vec::new();
    let mut buffer = vec![0u8; src.len() + src.len() / 255 + 1024];
    for (index, element) in src.chunks(job.elt_width).enumerate() {
        let operation = if (index + 1) * job.elt_width == src.len() {
            Operation::End
        } else {
            Operation::Flush
        };
        let progress = session
            .process(element, &mut buffer, operation)
            .map_err(|e| OpenZlError::Zstd(e.to_string()))?;
        assert_eq!(progress.consumed, element.len());
        output.extend_from_slice(&buffer[..progress.produced]);
    }
    Ok(output)
}

fn node_inputs() -> Vec<(usize, Vec<u8>)> {
    vec![
        (1, Vec::new()),
        (1, corpus::text(1, 1)),
        (1, corpus::text(5_000, 2)),
        (1, corpus::mixed(200_000, 3)),
        (4, corpus::binary(64_000, 4)),
        (1024, corpus::text(1024, 5)),
        (1024, corpus::text(2 * 1024, 6)),
        (1500, corpus::mixed(3 * 1500, 7)),
        (4096, corpus::text(8 * 4096, 8)),
        (4096, corpus::text(9 * 4096, 9)),
        (70_000, corpus::mixed(4 * 70_000, 10)),
        (300_000, corpus::text(3 * 300_000, 11)),
    ]
}

#[test]
fn the_encoder_node_writes_the_reference_bytes_at_every_level() {
    let mut reference = OpenZlEncoder::default();
    for &magicless in &[false, true] {
        for &level_value in &LEVELS {
            for (elt_width, src) in node_inputs() {
                for decompression_level in [0, 1] {
                    let job = OpenZlJob {
                        magicless,
                        compression_level: level_value,
                        decompression_level,
                        params: &[],
                        cdict: None,
                        elt_width,
                    };
                    let expected = reference.encode(&src, &job);
                    let ours = rust_node(&src, &job, None);
                    assert_eq!(
                        ours,
                        expected,
                        "magicless {magicless}, level {level_value}, width {elt_width}, {} bytes, decompression level {decompression_level}",
                        src.len()
                    );
                }
            }
        }
    }
}

#[test]
fn the_encoder_node_applies_local_parameters_in_order_and_enforces_its_policy() {
    let mut reference = OpenZlEncoder::default();
    let lists: [&[(i32, i32)]; 20] = [
        &[],
        &[(ZSTD_c_compressionLevel, 19)],
        &[(ZSTD_c_windowLog, 17), (ZSTD_c_hashLog, 12)],
        &[(ZSTD_c_checksumFlag, 1)],
        &[
            (ZSTD_c_literalCompressionMode, 2),
            (ZSTD_c_literalCompressionMode, 0),
        ],
        &[(ZSTD_c_literalCompressionMode, 1)],
        &[
            (ZSTD_c_strategy, 8),
            (ZSTD_c_targetLength, 100),
            (ZSTD_c_searchLog, 5),
        ],
        &[
            (ZSTD_c_enableLongDistanceMatching, 1),
            (ZSTD_c_ldmHashLog, 14),
            (ZSTD_c_windowLog, 20),
        ],
        &[(ZSTD_c_windowLog, 29)],
        &[(ZSTD_c_hashLog, 28)],
        &[(ZSTD_c_format, 0)],
        &[(ZSTD_c_contentSizeFlag, 1)],
        &[(ZSTD_c_nbWorkers, 2)],
        &[(12_345, 1)],
        // Targets: used as given, raised to the minimum, off, and refused.
        &[(ZSTD_c_targetCBlockSize, 2048)],
        &[(ZSTD_c_targetCBlockSize, 1)],
        &[
            (ZSTD_c_targetCBlockSize, 4096),
            (ZSTD_c_targetCBlockSize, 0),
        ],
        &[(ZSTD_c_targetCBlockSize, -7), (ZSTD_c_compressionLevel, 12)],
        &[(ZSTD_c_targetCBlockSize, 131_072), (ZSTD_c_checksumFlag, 1)],
        &[(ZSTD_c_targetCBlockSize, 131_073)],
    ];
    let src = corpus::text(40_000, 14);
    for params in lists {
        for elt_width in [1usize, 5_000] {
            let src = if elt_width == 1 {
                src.clone()
            } else {
                corpus::text(elt_width * 3, 15)
            };
            let job = OpenZlJob {
                magicless: true,
                compression_level: 3,
                decompression_level: 1,
                params,
                cdict: None,
                elt_width,
            };
            let expected = reference.encode(&src, &job);
            let ours = rust_node(&src, &job, None);
            match (&expected, &ours) {
                (Err(OpenZlError::Zstd(_)), Err(OpenZlError::Zstd(_))) => {}
                _ => assert_eq!(ours, expected, "{params:?}, width {elt_width}"),
            }
        }
    }
}

// ---- dictionaries -----------------------------------------------------

fn zstd_dictionary() -> Vec<u8> {
    let records = corpus::records(3000, 31);
    let samples: Vec<&[u8]> = records.iter().map(Vec::as_slice).collect();
    oracle::train_dictionary(&samples, 12 << 10).expect("trains")
}

fn dictionaries() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("raw text", corpus::text(20_000, 32)),
        ("raw tiny", b"abcdefgh".to_vec()),
        ("raw short", b"abc".to_vec()),
        ("raw large", corpus::mixed(200_000, 33)),
        ("zstd", zstd_dictionary()),
        ("empty", Vec::new()),
    ]
}

fn prepared(bytes: &[u8], level_value: i32) -> PreparedDictionary {
    DictionaryBuilder::new(bytes)
        .with_level_parameters(level(level_value))
        .build()
        .expect("prepares")
}

#[test]
fn referenced_dictionaries_write_the_reference_bytes() {
    let records = corpus::records(500, 34).concat();
    let sources: Vec<(usize, Vec<u8>)> = vec![
        (1, Vec::new()),
        (1, records[..300].to_vec()),
        (1, records[..9_000].to_vec()),
        (1, records[..40_000].to_vec()),
        (1, corpus::text(140_000, 35)),
        (1, corpus::mixed(500_000, 36)),
    ];
    let mut reference = OpenZlEncoder::default();
    for (name, bytes) in dictionaries() {
        for dictionary_level in [-3, 3, 9, 19] {
            let Some(cdict) = OpenZlCDict::new(&bytes, dictionary_level) else {
                continue;
            };
            let dictionary = prepared(&bytes, dictionary_level);
            for compressor_level in [-5, 1, 3, 4, 7, 12, 19, 22] {
                for (elt_width, src) in &sources {
                    let job = OpenZlJob {
                        magicless: compressor_level % 2 == 0,
                        compression_level: compressor_level,
                        decompression_level: 0,
                        params: &[],
                        cdict: Some(&cdict),
                        elt_width: *elt_width,
                    };
                    let expected = reference.encode(src, &job);
                    let ours = rust_node(src, &job, Some(&dictionary));
                    assert_eq!(
                        ours,
                        expected,
                        "{name} dictionary at level {dictionary_level}, compressor level {compressor_level}, {} bytes",
                        src.len()
                    );
                }
            }
        }
    }
}

#[test]
fn referenced_dictionaries_follow_the_attach_preference_and_forced_window() {
    let bytes = corpus::text(30_000, 41);
    let cdict = OpenZlCDict::new(&bytes, 6).expect("prepares");
    let dictionary = prepared(&bytes, 6);
    let mut reference = OpenZlEncoder::default();
    let src = corpus::text(20_000, 42);
    let big = corpus::text(300_000, 43);
    for (src_name, src) in [("small", &src), ("large", &big)] {
        for attach in 0..=3 {
            for force_window in [0, 1] {
                for compressor_level in [1, 6, 13, 19] {
                    let params = [
                        (ZSTD_c_forceAttachDict, attach),
                        (ZSTD_c_forceMaxWindow, force_window),
                    ];
                    let job = OpenZlJob {
                        magicless: false,
                        compression_level: compressor_level,
                        decompression_level: 0,
                        params: &params,
                        cdict: Some(&cdict),
                        elt_width: 1,
                    };
                    let expected = reference.encode(src, &job);
                    let ours = rust_node(src, &job, Some(&dictionary));
                    assert_eq!(
                        ours, expected,
                        "{src_name}, attach {attach}, force window {force_window}, level {compressor_level}"
                    );
                }
            }
        }
    }
}

#[test]
fn referenced_dictionaries_keep_the_compressors_explicit_parameters() {
    let bytes = zstd_dictionary();
    let cdict = OpenZlCDict::new(&bytes, 3).expect("prepares");
    let dictionary = prepared(&bytes, 3);
    let mut reference = OpenZlEncoder::default();
    let records = corpus::records(200, 44).concat();
    let lists: [&[(i32, i32)]; 6] = [
        &[(ZSTD_c_windowLog, 14)],
        &[(ZSTD_c_checksumFlag, 1), (ZSTD_c_dictIDFlag, 0)],
        &[(ZSTD_c_dictIDFlag, 0)],
        &[(ZSTD_c_enableLongDistanceMatching, 1)],
        &[(ZSTD_c_strategy, 9), (ZSTD_c_hashLog, 13)],
        &[
            (ZSTD_c_useRowMatchFinder, 2),
            (ZSTD_c_splitAfterSequences, 1),
        ],
    ];
    for params in lists {
        for compressor_level in [2, 12, 20] {
            let job = OpenZlJob {
                magicless: true,
                compression_level: compressor_level,
                decompression_level: 1,
                params,
                cdict: Some(&cdict),
                elt_width: 1,
            };
            let expected = reference.encode(&records, &job);
            let ours = rust_node(&records, &job, Some(&dictionary));
            assert_eq!(ours, expected, "{params:?}, level {compressor_level}");
        }
    }
}

#[test]
fn a_standalone_dictionary_serves_every_configuration_and_a_keyed_one_does_not() {
    let bytes = corpus::text(10_000, 45);
    let standalone = prepared(&bytes, 5);
    let keyed = DictionaryBuilder::new(&bytes).build().expect("prepares");
    let other = EncoderConfig::default().with_level(level(9));
    let mut compressor = Compressor::new(other).expect("valid");
    assert!(
        compressor
            .compress_with_dictionary(&standalone, b"payload")
            .is_ok()
    );
    assert!(matches!(
        compressor.compress_with_dictionary(&keyed, b"payload"),
        Err(mzstd::EncodeError::DictionaryMismatch)
    ));
}

// ---- decoding with a DDict --------------------------------------------

#[test]
fn ddict_decoding_matches_the_reference_for_every_dictionary_kind() {
    let mut reference = OpenZlEncoder::default();
    let src = corpus::records(300, 51).concat();
    for (name, bytes) in dictionaries() {
        let cdict = OpenZlCDict::new(&bytes, 3);
        let ddict = OpenZlDDict::new(&bytes);
        let (Some(cdict), Some(ddict)) = (cdict, ddict) else {
            continue;
        };
        let decode_dictionary = DecodeDictionary::new(&bytes, DictionaryContent::Auto);
        for magicless in [false, true] {
            let job = OpenZlJob {
                magicless,
                compression_level: 5,
                decompression_level: 0,
                params: &[],
                cdict: Some(&cdict),
                elt_width: 1,
            };
            let frame = reference.encode(&src, &job).expect("frame");
            let expected = oracle::openzl_decode(magicless, Some(&ddict), &frame, src.len() + 1);
            let config = DecoderConfig::default().with_format(if magicless {
                FrameFormat::Magicless
            } else {
                FrameFormat::Standard
            });
            let ours = decode_dictionary
                .as_ref()
                .map_err(|_| ())
                .and_then(|dictionary| {
                    Decompressor::new(config)
                        .decompress_with_dictionary(dictionary, &frame)
                        .map_err(|_| ())
                });
            match (expected, ours) {
                (Ok(expected), Ok(ours)) => {
                    assert_eq!(ours, expected, "{name}, magicless {magicless}");
                    assert_eq!(ours, src);
                }
                (Err(_), Err(())) => {}
                (expected, ours) => panic!("{name}: {expected:?} vs {ours:?}"),
            }
        }
    }
}

#[test]
fn a_frame_without_the_dictionary_is_refused_as_the_reference_refuses_it() {
    let bytes = zstd_dictionary();
    let cdict = OpenZlCDict::new(&bytes, 3).expect("prepares");
    let ddict = OpenZlDDict::new(&bytes).expect("prepares");
    let src = corpus::records(100, 52).concat();
    let job = OpenZlJob {
        magicless: false,
        compression_level: 3,
        decompression_level: 0,
        params: &[],
        cdict: Some(&cdict),
        elt_width: 1,
    };
    let frame = OpenZlEncoder::default().encode(&src, &job).expect("frame");
    assert!(oracle::openzl_decode(false, None, &frame, src.len() + 1).is_err());
    assert!(Decompressor::default().decompress(&frame).is_err());
    let wrong = OpenZlDDict::new(&corpus::text(5_000, 53)).expect("prepares");
    assert!(oracle::openzl_decode(false, Some(&wrong), &frame, src.len() + 1).is_err());
    let wrong = DecodeDictionary::try_from(&corpus::text(5_000, 53)[..]).expect("raw");
    assert!(
        Decompressor::default()
            .decompress_with_dictionary(&wrong, &frame)
            .is_err()
    );
    assert_eq!(
        oracle::openzl_decode(false, Some(&ddict), &frame, src.len() + 1).expect("decodes"),
        src
    );
}

// ---- frame header queries and getCParams ------------------------------

fn compare_header(src: &[u8], magicless: bool) {
    let format = if magicless {
        FrameFormat::Magicless
    } else {
        FrameFormat::Standard
    };
    let reference = oracle::frame_header(src, magicless);
    let ours = FrameHeaderInfo::parse(src, format);
    match (reference, ours) {
        (Ok(Ok(report)), Ok(info)) => {
            let skippable = report.frame_type == 1;
            assert_eq!(info.kind == FrameKind::Skippable, skippable, "{src:02x?}");
            assert_eq!(info.header_size, report.header_size as usize, "{src:02x?}");
            if skippable {
                assert_eq!(info.content_size, Some(report.content_size));
            } else {
                let expected = (report.content_size != ZSTD_CONTENTSIZE_UNKNOWN)
                    .then_some(report.content_size);
                assert_eq!(info.content_size, expected, "{src:02x?}");
                assert_eq!(info.window_size, report.window_size, "{src:02x?}");
                assert_eq!(
                    info.dictionary_id.map_or(0, u32::from),
                    report.dict_id,
                    "{src:02x?}"
                );
                assert_eq!(info.checksum, report.checksum, "{src:02x?}");
            }
        }
        (Ok(Err(_)), Err(DecodeError::UnexpectedEndOfInput)) => {}
        (Err(_), Err(error)) => {
            assert!(
                !matches!(error, DecodeError::UnexpectedEndOfInput),
                "{src:02x?}: {error}"
            );
        }
        (reference, ours) => {
            panic!("{src:02x?} (magicless {magicless}): {reference:?} vs {ours:?}")
        }
    }
}

#[test]
fn frame_headers_read_like_the_reference_for_every_prefix_and_shape() {
    let bytes = zstd_dictionary();
    let records = corpus::records(50, 61).concat();
    let mut frames = Vec::new();
    for magicless in [0, 1] {
        for (content_size, checksum, dict_id, len) in [
            (1, 0, 1, 0),
            (1, 1, 1, 100),
            (1, 0, 1, 70_000),
            (0, 1, 1, 3_000),
            (1, 1, 0, 300),
        ] {
            let src = corpus::text(len, 62);
            let mut params = Params::level(3)
                .with(ZSTD_c_format, magicless)
                .with(ZSTD_c_contentSizeFlag, content_size)
                .with(ZSTD_c_checksumFlag, checksum)
                .with(ZSTD_c_dictIDFlag, dict_id);
            if len == 300 {
                params = params.with_dictionary(oracle::Dictionary::auto(&bytes));
            }
            let _ = &records;
            frames.push((
                magicless == 1,
                oracle::compress2(&src, &params).expect("frame"),
            ));
        }
    }
    let skippable = [0x55, 0x2A, 0x4D, 0x18, 3, 0, 0, 0, 1, 2, 3];
    frames.push((false, skippable.to_vec()));
    for (magicless, frame) in &frames {
        for end in 0..=frame.len().min(40) {
            compare_header(&frame[..end], *magicless);
        }
        // The other format reads the same bytes as something else.
        compare_header(frame, !magicless);
    }
}

#[test]
fn frame_headers_read_like_the_reference_for_random_bytes() {
    let mut rng = corpus::Rng::new(0xFEED_F00D);
    for _ in 0..20_000 {
        let len = (rng.next_u64() % 20) as usize;
        let mut bytes: Vec<u8> = (0..len).map(|_| rng.next_u64() as u8).collect();
        let magic = match rng.next_u64() % 3 {
            0 => Some(0xFD2F_B528u32),
            1 => Some(0x184D_2A50u32 + (rng.next_u64() % 16) as u32),
            _ => None,
        };
        if let Some(magic) = magic
            && len >= 4
        {
            bytes[..4].copy_from_slice(&magic.to_le_bytes());
        }
        compare_header(&bytes, false);
        compare_header(&bytes, true);
    }
}

#[test]
fn level_parameters_equal_zstd_get_cparams() {
    let strategies = [
        Strategy::Fast,
        Strategy::DoubleFast,
        Strategy::Greedy,
        Strategy::Lazy,
        Strategy::Lazy2,
        Strategy::BtLazy2,
        Strategy::BtOpt,
        Strategy::BtUltra,
        Strategy::BtUltra2,
    ];
    for &level_value in &LEVELS {
        for source in [
            0u64,
            1,
            500,
            16_384,
            16_385,
            100_000,
            131_072,
            262_145,
            1 << 30,
        ] {
            for dictionary in [0usize, 1, 700, 20_000, 300_000] {
                let reference = oracle::get_cparams(level_value, source, dictionary);
                let ours = CompressionParams::for_level(level(level_value), source, dictionary);
                let strategy = strategies[reference.strategy as usize - 1];
                assert_eq!(ours.window_log(), Some(reference.windowLog));
                assert_eq!(ours.hash_log(), Some(reference.hashLog));
                assert_eq!(ours.chain_log(), Some(reference.chainLog));
                assert_eq!(ours.search_log(), Some(reference.searchLog));
                assert_eq!(ours.min_match(), Some(reference.minMatch));
                assert_eq!(
                    ours.target_length(),
                    (reference.targetLength != 0).then_some(reference.targetLength)
                );
                assert_eq!(ours.strategy(), Some(strategy));
            }
        }
    }
}

#[test]
fn compress_bound_equals_zstd_compress_bound() {
    for len in [
        0usize,
        1,
        255,
        256,
        65_535,
        131_071,
        131_072,
        131_073,
        1 << 20,
        1 << 30,
    ] {
        assert_eq!(
            Compressor::max_compressed_size(len).expect("bound"),
            oracle::compress_bound(len)
        );
    }
}

use mzstd::Strategy;

#[test]
fn the_dictionary_attach_preference_converts_to_the_reference_numbers() {
    for (attach, value) in [
        (DictionaryAttach::Default, 0),
        (DictionaryAttach::Attach, 1),
        (DictionaryAttach::Copy, 2),
        (DictionaryAttach::Load, 3),
    ] {
        assert_eq!(i32::from(attach), value);
        assert_eq!(
            CParameter::try_from((ZSTD_c_forceAttachDict, value)).expect("valid"),
            CParameter::ForceAttachDict(attach)
        );
    }
}
