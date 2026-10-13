#![cfg(feature = "decompression")]
//! The decoder's public contract: frame modes, the incremental protocol,
//! limits, typed errors and buffer retention.

mod decoder_support;

use decoder_support::{compress, decode_fixed, params, reference_decompress, skippable, text};
use mzstd::{
    DecodeError, DecodeLimits, DecodeOperation, DecoderConfig, DecoderStatus, Decompressor,
    FrameMode, InvalidDataKind, RetentionPolicy,
};

/// A frame of one raw block, built by hand: optional content size and
/// checksum, a 1 KiB window otherwise.
fn raw_frame(content: &[u8], declared: Option<u8>, checksum: Option<u32>) -> Vec<u8> {
    let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd];
    let mut descriptor = 0u8;
    if checksum.is_some() {
        descriptor |= 0x04;
    }
    match declared {
        Some(size) => {
            frame.push(descriptor | 0x20);
            frame.push(size);
        }
        None => {
            frame.push(descriptor);
            frame.push(0);
        }
    }
    let header = ((content.len() as u32) << 3) | 1;
    frame.extend_from_slice(&header.to_le_bytes()[..3]);
    frame.extend_from_slice(content);
    if let Some(checksum) = checksum {
        frame.extend_from_slice(&checksum.to_le_bytes());
    }
    frame
}

fn invalid(kind: InvalidDataKind) -> impl Fn(&DecodeError) -> bool {
    move |error| matches!(error, DecodeError::InvalidData { kind: found } if *found == kind)
}

#[test]
fn single_mode_skips_leading_skippable_frames_and_rejects_trailing_bytes() {
    let frame = raw_frame(b"one", Some(3), None);
    let mut input = skippable(3, b"meta");
    input.extend_from_slice(&frame);
    let mut decoder = Decompressor::new(FrameMode::Single.into());
    assert_eq!(decoder.decompress(&input).expect("one frame"), b"one");
    input.push(0);
    assert!(matches!(
        decoder.decompress(&input),
        Err(DecodeError::TrailingData { offset }) if offset == input.len() as u64 - 1
    ));
    assert!(matches!(
        decoder.decompress_to_slice(&input, &mut [0; 8]),
        Err(DecodeError::TrailingData { .. })
    ));
    // Skippable frames alone are no Zstandard frame.
    assert!(matches!(
        decoder.decompress(&skippable(0, b"x")),
        Err(DecodeError::UnexpectedEndOfInput)
    ));
    // Concatenated mode accepts them as a complete, empty operation.
    let mut decoder = Decompressor::default();
    assert!(
        decoder
            .decompress(&skippable(0, b"x"))
            .expect("skippable")
            .is_empty()
    );
}

#[test]
fn concatenated_sessions_wait_for_finish_at_frame_boundaries() {
    let frame = raw_frame(b"ab", Some(2), None);
    let mut decoder = Decompressor::default();
    let mut session = decoder.start();
    let mut output = [0u8; 8];
    let progress = session
        .process(&frame, &mut output, DecodeOperation::Process)
        .expect("valid");
    assert_eq!(progress.status, DecoderStatus::NeedsInput);
    assert_eq!(&output[..progress.produced], b"ab");
    assert!(!session.is_finished());
    let progress = session
        .process(&frame, &mut output, DecodeOperation::Process)
        .expect("second frame");
    assert_eq!(progress.produced, 2);
    let progress = session.finish(&mut output).expect("ends at a boundary");
    assert_eq!(progress.status, DecoderStatus::Finished);
    assert_eq!(session.frames_decoded(), 2);
    assert_eq!(session.total_out(), 4);
    assert_eq!(session.total_in(), 2 * frame.len() as u64);
    assert!(session.is_finished());
}

#[test]
fn the_finish_contract_is_enforced() {
    let input = text(50_000, 3);
    let frame = compress(&input, &params(3, true, true));
    let mut decoder = Decompressor::default();
    // Draining through `flush` and `finish`.
    let mut session = decoder.start();
    let mut output = vec![0u8; input.len()];
    let progress = session
        .process(&frame, &mut output[..10], DecodeOperation::Process)
        .expect("valid");
    assert_eq!(progress.status, DecoderStatus::NeedsOutput);
    let mut written = progress.produced;
    let mut consumed = progress.consumed;
    loop {
        let progress = session
            .process(
                &frame[consumed..],
                &mut output[written..],
                DecodeOperation::Process,
            )
            .expect("valid");
        written += progress.produced;
        consumed += progress.consumed;
        if progress.status == DecoderStatus::NeedsInput {
            break;
        }
    }
    let progress = session
        .flush(&mut output[written..])
        .expect("nothing pending");
    assert_eq!(progress.produced, 0);
    let progress = session.finish(&mut output[written..]).expect("complete");
    assert_eq!(progress.status, DecoderStatus::Finished);
    assert_eq!(output, input);
    assert!(session.window_size().is_some());

    // After `Finish`, the remaining input may not change.
    let mut session = decoder.start();
    let progress = session
        .process(&frame, &mut [0u8; 16], DecodeOperation::Finish)
        .expect("partial");
    let rest = &frame[progress.consumed..];
    let failure = session
        .process(
            &rest[..rest.len() - 1],
            &mut [0u8; 16],
            DecodeOperation::Finish,
        )
        .expect_err("shorter suffix");
    assert!(matches!(failure.error, DecodeError::InvalidState));
    assert_eq!((failure.consumed, failure.produced), (0, 0));
    // A failed session stays failed.
    let failure = session.flush(&mut [0u8; 16]).expect_err("failed");
    assert!(matches!(failure.into_error(), DecodeError::InvalidState));
    // The decoder itself is reusable.
    assert_eq!(decoder.decompress(&frame).expect("reusable"), input);
}

#[test]
fn failures_report_the_progress_of_the_failing_call() {
    let mut frame = raw_frame(b"hello", Some(5), Some(0));
    let mut decoder = Decompressor::default();
    let mut session = decoder.start();
    let mut output = [0u8; 8];
    let failure = session
        .process(&frame, &mut output, DecodeOperation::Finish)
        .expect_err("bad checksum");
    assert!(matches!(
        failure.error,
        DecodeError::ChecksumMismatch { expected: 0, .. }
    ));
    assert_eq!(failure.consumed, frame.len());
    assert_eq!(failure.produced, 5);
    assert_eq!(&output[..5], b"hello");
    assert_eq!(failure.to_string(), failure.error.to_string());
    assert!(std::error::Error::source(&failure).is_some());
    // The reference agrees on the checksum.
    let error = reference_decompress(&frame, 16).expect_err("reference rejects");
    assert!(error.contains("checksum"), "{error}");
    // With the right checksum it decodes.
    let length = frame.len();
    frame[length - 4..].copy_from_slice(&reference_checksum(b"hello").to_le_bytes());
    assert_eq!(decoder.decompress(&frame).expect("valid"), b"hello");
}

/// The reference's content checksum of `content`.
fn reference_checksum(content: &[u8]) -> u32 {
    let frame = compress(content, &params(1, true, true));
    u32::from_le_bytes(frame[frame.len() - 4..].try_into().expect("four bytes"))
}

#[test]
fn malformed_headers_and_sizes_produce_typed_errors() {
    let mut decoder = Decompressor::default();
    // Reserved descriptor bit.
    let mut reserved = raw_frame(b"x", Some(1), None);
    reserved[4] |= 0x08;
    assert!(invalid(InvalidDataKind::FrameHeader)(
        &decoder.decompress(&reserved).unwrap_err()
    ));
    assert!(reference_decompress(&reserved, 16).is_err());
    // Reserved block type.
    let mut block_type = raw_frame(b"x", Some(1), None);
    block_type[6] |= 0x06;
    assert!(invalid(InvalidDataKind::BlockType)(
        &decoder.decompress(&block_type).unwrap_err()
    ));
    assert!(reference_decompress(&block_type, 16).is_err());
    // A block larger than a single-segment frame's content size.
    let oversized = raw_frame(b"xyz", Some(2), None);
    assert!(invalid(InvalidDataKind::BlockSize)(
        &decoder.decompress(&oversized).unwrap_err()
    ));
    // Fewer bytes than declared.
    let short = raw_frame(b"xy", Some(3), None);
    assert!(matches!(
        decoder.decompress(&short),
        Err(DecodeError::ContentSizeMismatch {
            declared: 3,
            decoded: 2
        })
    ));
    assert!(reference_decompress(&short, 16).is_err());
    // A dictionary ID: header descriptor with a one-byte ID of 7.
    let mut dictionary = vec![0x28, 0xb5, 0x2f, 0xfd, 0x21, 0x07, 0x01];
    dictionary.extend_from_slice(&[0x09, 0x00, 0x00, b'z']);
    assert!(matches!(
        decoder.decompress(&dictionary),
        Err(DecodeError::DictionaryUnsupported { id: 7 })
    ));
    assert!(reference_decompress(&dictionary, 16).is_err());
    // A dictionary ID of zero means none.
    let mut no_dictionary = dictionary.clone();
    no_dictionary[5] = 0;
    assert_eq!(
        decoder.decompress(&no_dictionary).expect("no dictionary"),
        b"z"
    );
    // Unknown magic numbers.
    assert!(invalid(InvalidDataKind::Magic)(
        &decoder.decompress(b"\x28\xb5\x2f\xfe....").unwrap_err()
    ));
    // Garbage after a complete frame.
    let mut garbage = raw_frame(b"x", Some(1), None);
    garbage.extend_from_slice(b"junk");
    assert!(invalid(InvalidDataKind::Magic)(
        &decoder.decompress(&garbage).unwrap_err()
    ));
    assert!(decode_fixed(&mut decoder, &garbage, 2, 2).is_err());
}

#[test]
fn byte_budgets_bound_one_shot_and_streaming_operations() {
    let input = text(40_000, 4);
    let frame = compress(&input, &params(3, false, true));
    let exact = DecodeLimits::default()
        .with_max_input_bytes(Some(frame.len() as u64))
        .with_max_output_bytes(Some(input.len() as u64));
    let mut decoder = Decompressor::new(exact.into());
    assert_eq!(decoder.decompress(&frame).expect("within budget"), input);
    assert_eq!(
        decode_fixed(&mut decoder, &frame, 100, 100).expect("within budget"),
        input
    );

    let small_input = exact.with_max_input_bytes(Some(frame.len() as u64 - 1));
    let mut decoder = Decompressor::new(small_input.into());
    assert!(matches!(
        decoder.decompress(&frame),
        Err(DecodeError::InputLimitExceeded { .. })
    ));
    assert!(matches!(
        decode_fixed(&mut decoder, &frame, 100, 100),
        Err(DecodeError::InputLimitExceeded { .. })
    ));
    let small_output = exact.with_max_output_bytes(Some(input.len() as u64 - 1));
    let mut decoder = Decompressor::new(small_output.into());
    assert!(matches!(
        decoder.decompress(&frame),
        Err(DecodeError::OutputLimitExceeded { .. })
    ));
    assert!(matches!(
        decode_fixed(&mut decoder, &frame, 100, 100),
        Err(DecodeError::OutputLimitExceeded { .. })
    ));
    assert_eq!(
        small_output.max_output_bytes(),
        Some(input.len() as u64 - 1)
    );
    assert!(matches!(
        DecodeLimits::default().with_max_window_log(32),
        Err(mzstd::DecodeConfigError::WindowLog { log: 32 })
    ));
    assert_eq!(DecodeLimits::DEFAULT.max_window_log(), 27);
    assert_eq!(DecodeLimits::default().max_input_bytes(), None);
}

#[test]
fn slices_and_vectors_report_their_outcome() {
    let input = text(10_000, 5);
    let frame = compress(&input, &params(5, true, true));
    let mut decoder = Decompressor::default();
    let mut exact = vec![0u8; input.len()];
    assert_eq!(
        decoder
            .decompress_to_slice(&frame, &mut exact)
            .expect("fits"),
        input.len()
    );
    assert_eq!(exact, input);
    let mut short = vec![0u8; input.len() - 1];
    assert!(matches!(
        decoder.decompress_to_slice(&frame, &mut short),
        Err(DecodeError::OutputTooSmall { written }) if written == input.len() - 1
    ));
    assert_eq!(short, input[..input.len() - 1]);
    assert!(matches!(
        decoder.decompress_to_slice(&frame[..frame.len() - 1], &mut exact),
        Err(DecodeError::UnexpectedEndOfInput)
    ));
    // A failed append leaves the vector as it was.
    let mut output = b"kept".to_vec();
    decoder
        .decompress_into(&frame[..frame.len() - 1], &mut output)
        .expect_err("truncated");
    assert_eq!(output, b"kept");
    assert!(decoder.decompress(&[]).is_err());
}

#[test]
fn retention_policies_release_buffers_between_operations() {
    let input = text(200_000, 6);
    let frame = compress(&input, &params(3, false, true));
    let mut decoder = Decompressor::default();
    assert_eq!(decoder.retained_bytes(), 0);
    assert_eq!(decoder.retention(), RetentionPolicy::Aggressive);
    decode_fixed(&mut decoder, &frame, 1000, 1000).expect("valid");
    let retained = decoder.retained_bytes();
    assert!(retained > 0);
    decoder.trim(RetentionPolicy::Bounded {
        max_bytes: retained,
    });
    assert_eq!(decoder.retained_bytes(), retained);
    decoder.trim(RetentionPolicy::Bounded {
        max_bytes: retained - 1,
    });
    assert_eq!(decoder.retained_bytes(), 0);

    let mut releasing = Decompressor::builder(DecoderConfig::default())
        .with_retention(RetentionPolicy::ReleaseAll)
        .build();
    assert_eq!(releasing.decompress(&frame).expect("valid"), input);
    assert_eq!(releasing.retained_bytes(), 0);
    decode_fixed(&mut releasing, &frame, 1000, 1000).expect("valid");
    assert_eq!(releasing.retained_bytes(), 0);
    let mut session = releasing.into_session();
    let mut output = vec![0u8; input.len()];
    session
        .process(&frame, &mut output, DecodeOperation::Finish)
        .expect("valid");
    session.reinit();
    assert!(!session.is_finished());
    let decoder = session.into_decompressor();
    assert_eq!(decoder.retained_bytes(), 0);
    assert_eq!(decoder.retention(), RetentionPolicy::ReleaseAll);
}

#[test]
fn owned_sessions_mirror_borrowed_ones() {
    let frame = raw_frame(b"owned", Some(5), None);
    let mut session = Decompressor::new(FrameMode::Single.into()).into_session();
    let mut output = [0u8; 2];
    let mut decoded = Vec::new();
    let mut remaining = &frame[..];
    loop {
        let progress = session
            .process(remaining, &mut output, DecodeOperation::Process)
            .expect("valid");
        remaining = &remaining[progress.consumed..];
        decoded.extend_from_slice(&output[..progress.produced]);
        if progress.status == DecoderStatus::Finished {
            break;
        }
        let progress = session.flush(&mut output).expect("drains");
        decoded.extend_from_slice(&output[..progress.produced]);
    }
    assert_eq!(decoded, b"owned");
    assert!(session.is_finished());
    assert_eq!(session.total_in(), frame.len() as u64);
    assert_eq!(session.total_out(), 5);
    assert_eq!(session.frames_decoded(), 1);
    assert_eq!(session.window_size(), Some(5));
    let progress = session.finish(&mut output).expect("finished");
    assert_eq!(
        (progress.consumed, progress.status),
        (0, DecoderStatus::Finished)
    );
    session.reinit();
    let failure = session.finish(&mut output).expect_err("no frame");
    assert!(matches!(failure.error, DecodeError::UnexpectedEndOfInput));
}

#[test]
fn configuration_round_trips_through_the_decoder() {
    let config = DecoderConfig::from(FrameMode::Single).with_limits(
        DecodeLimits::default()
            .with_max_window_log(20)
            .expect("valid"),
    );
    let mut decoder = Decompressor::from(config);
    assert_eq!(decoder.config(), &config);
    decoder.reconfigure(DecoderConfig::default());
    assert_eq!(decoder.config().frame_mode(), FrameMode::Concatenated);
    assert_eq!(
        DecoderConfig::from(DecodeLimits::DEFAULT),
        DecoderConfig::DEFAULT
    );
    assert_eq!(FrameMode::default(), FrameMode::Concatenated);
    let empty = compress(&[], &params(3, true, true));
    assert!(decoder.decompress(&empty).expect("empty frame").is_empty());
    assert!(format!("{decoder:?}").contains("Decompressor"));
}

#[test]
fn window_limits_accept_every_log_between_the_bounds() {
    for log in DecodeLimits::MIN_WINDOW_LOG..=DecodeLimits::MAX_WINDOW_LOG {
        let limits = DecodeLimits::default()
            .with_max_window_log(log)
            .expect("in range");
        assert_eq!(limits.max_window_log(), log);
        assert_eq!(limits.max_window_size(), 1u64 << log);
    }
    let error = DecodeLimits::default()
        .with_max_window_log(DecodeLimits::MIN_WINDOW_LOG - 1)
        .expect_err("below the minimum");
    assert_eq!(error.to_string(), "window log limit must be 10..=31, got 9");
}

#[test]
fn a_window_limit_rejects_frames_with_a_larger_window() {
    let input = text(300_000, 7);
    let frame = compress(
        &input,
        &params(3, false, false).with(zstd_ffi::ZSTD_c_windowLog, 18),
    );
    let tight = DecodeLimits::default()
        .with_max_window_log(17)
        .expect("in range");
    let mut decoder = Decompressor::new(tight.into());
    assert!(matches!(
        decoder.decompress(&frame),
        Err(DecodeError::WindowLimitExceeded {
            window_size: 0x4_0000,
            limit: 0x2_0000
        })
    ));
    decoder.reconfigure(
        DecodeLimits::default()
            .with_max_window_log(18)
            .expect("in range")
            .into(),
    );
    assert_eq!(decoder.decompress(&frame).expect("fits"), input);
}

#[test]
fn borrowed_sessions_count_frames_and_bytes() {
    let frame = raw_frame(b"one", Some(3), None);
    let input = [frame.as_slice(), &skippable(1, b"meta"), &frame].concat();
    let mut decoder = Decompressor::default();
    let mut session = decoder.start();
    assert_eq!(session.window_size(), None);
    assert_eq!((session.total_in(), session.total_out()), (0, 0));
    let mut output = [0u8; 16];
    let progress = session
        .process(&input, &mut output, DecodeOperation::Finish)
        .expect("valid");
    assert_eq!(progress.status, DecoderStatus::Finished);
    assert_eq!(&output[..progress.produced], b"oneone");
    assert!(session.is_finished());
    assert_eq!(session.frames_decoded(), 2);
    assert_eq!(session.total_in(), input.len() as u64);
    assert_eq!(session.total_out(), 6);
    assert_eq!(session.window_size(), Some(3));
}

#[test]
fn owned_sessions_count_frames_and_bytes() {
    let first = raw_frame(b"first", Some(5), None);
    let second = raw_frame(b"second!", Some(7), None);
    let input = [first.as_slice(), &second].concat();
    let mut session = Decompressor::default().into_session();
    assert_eq!(session.frames_decoded(), 0);
    assert_eq!(session.window_size(), None);
    let mut output = [0u8; 32];
    let progress = session
        .process(&input, &mut output, DecodeOperation::Finish)
        .expect("valid");
    assert_eq!(&output[..progress.produced], b"firstsecond!");
    assert!(session.is_finished());
    assert_eq!(session.frames_decoded(), 2);
    assert_eq!(session.total_in(), input.len() as u64);
    assert_eq!(session.total_out(), 12);
    assert_eq!(session.window_size(), Some(7));
}

#[test]
fn decode_failures_convert_into_their_error() {
    let mut decoder = Decompressor::default();
    let mut session = decoder.start();
    let failure = session
        .process(b"not a frame", &mut [0u8; 16], DecodeOperation::Finish)
        .expect_err("bad magic");
    let message = failure.error.to_string();
    assert_eq!(failure.to_string(), message);
    let source = std::error::Error::source(&failure).expect("the decode error");
    assert_eq!(source.to_string(), message);
    let error = DecodeError::from(failure);
    assert_eq!(error.to_string(), message);
    let failure = session.finish(&mut []).expect_err("failed session");
    assert!(matches!(failure.into_error(), DecodeError::InvalidState));
}

#[test]
fn decompressors_built_from_a_configuration_keep_buffers() {
    let config = DecoderConfig::from(FrameMode::Single);
    let decoder = Decompressor::from(config);
    assert_eq!(decoder.config(), &config);
    assert_eq!(decoder.retention(), RetentionPolicy::Aggressive);
    let built = Decompressor::builder(config).build();
    assert_eq!(built.config(), &config);
    assert_eq!(built.retention(), RetentionPolicy::Aggressive);
}

#[test]
fn the_builder_records_a_bounded_retention_policy() {
    let policy = RetentionPolicy::Bounded { max_bytes: 4096 };
    let input = text(200_000, 8);
    let frame = compress(&input, &params(3, false, true));
    let mut decoder = Decompressor::builder(DecoderConfig::default())
        .with_retention(policy)
        .build();
    assert_eq!(decoder.retention(), policy);
    assert_eq!(decoder.decompress(&frame).expect("valid"), input);
    assert!(decoder.retained_bytes() <= 4096);
}
