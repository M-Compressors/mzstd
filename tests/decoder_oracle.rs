#![cfg(feature = "decompression")]
//! Differential tests: frames written by the reference encoder (zstd 1.5.7)
//! must decode to their input through every decoder entry point, and agree
//! with the reference decoder.

mod decoder_support;

use decoder_support::{
    Rng, compress, corpora, decode_fixed, decode_streaming, mixed, params, reference_decompress,
    skippable, text,
};
use mzstd::{Backend, DecodeError, DecodeLimits, DecoderConfig, Decompressor, FrameMode};
use zstd_ffi::oracle::{self, Directive, Params};

/// Streams `input` through the reference in two steps, so the content size
/// stays unknown and the frame keeps its configured window descriptor
/// instead of shrinking to the input.
fn stream_unknown_size(params: &Params, input: &[u8]) -> Vec<u8> {
    let (head, tail) = input.split_at(input.len() / 2);
    let frame = oracle::compress_stream(
        params,
        &[(head, Directive::Continue), (tail, Directive::End)],
    )
    .expect("the reference streams");
    assert_eq!(frame[4] & 0xe0, 0, "no content size, not single-segment");
    frame
}

/// Decodes `frame` through every one-shot shape and checks the result.
fn assert_one_shot(decoder: &mut Decompressor, frame: &[u8], expected: &[u8], label: &str) {
    assert_eq!(
        decoder.decompress(frame).expect(label),
        expected,
        "{label}: decompress"
    );
    let mut appended = b"prefix".to_vec();
    let range = decoder.decompress_into(frame, &mut appended).expect(label);
    assert_eq!(&appended[range], expected, "{label}: decompress_into");
    let mut slice = vec![0xa5u8; expected.len() + 3];
    let written = decoder.decompress_to_slice(frame, &mut slice).expect(label);
    assert_eq!(&slice[..written], expected, "{label}: decompress_to_slice");
    assert_eq!(
        &slice[written..],
        &[0xa5; 3],
        "{label}: slice suffix untouched"
    );
}

#[test]
fn reference_frames_decode_across_levels_sizes_and_flags() {
    let mut decoder = Decompressor::default();
    let levels = [-5, -1, 1, 3, 6, 9, 13, 19];
    let mut flags = 0usize;
    for size in [0usize, 1, 2, 17, 300, 4_000, 140_000] {
        for (name, input) in corpora(size, size as u64 + 1) {
            for level in levels {
                if level >= 13 && size > 4_000 && name != "text" {
                    continue; // Slow at -O0; covered by the release-only matrix.
                }
                flags += 1;
                let checksum = flags.is_multiple_of(2);
                let content_size = !flags.is_multiple_of(3);
                let frame = compress(&input, &params(level, checksum, content_size));
                let label =
                    format!("{name} {size} level {level} checksum {checksum} size {content_size}");
                assert_one_shot(&mut decoder, &frame, &input, &label);
                assert_eq!(
                    reference_decompress(&frame, input.len()).expect("reference"),
                    input,
                    "{label}: reference agrees"
                );
            }
        }
    }
}

#[test]
fn every_backend_decodes_reference_frames_alike() {
    // Short-offset matches, literal-heavy and literal-light blocks, both
    // Huffman decoders and raw blocks, on every SIMD level the host has.
    for backend in Backend::available() {
        let mut decoder = Decompressor::builder(DecoderConfig::default())
            .with_backend(backend)
            .build();
        assert_eq!(decoder.backend(), backend);
        for size in [17usize, 4_000, 140_000] {
            for (name, input) in corpora(size, size as u64 + 7) {
                for level in [-5, 1, 3, 9, 19] {
                    if level == 19 && size > 4_000 && name != "text" {
                        continue; // Slow at -O0; other levels cover these blocks.
                    }
                    let content_size = level != 3;
                    let frame = compress(&input, &params(level, level == 9, content_size));
                    let label = format!("{backend} {name} {size} level {level}");
                    assert_one_shot(&mut decoder, &frame, &input, &label);
                    assert_eq!(
                        decode_streaming(&mut decoder, &frame, || 4096, 1000).expect(&label),
                        input,
                        "{label}: streaming"
                    );
                }
            }
        }
    }
}

#[test]
fn streaming_matches_one_shot_for_any_chunking() {
    let mut decoder = Decompressor::default();
    let inputs = [
        ("text", text(300_000, 7), 3),
        ("mixed", mixed(260_000, 8), 9),
        ("text-19", text(90_000, 9), 19),
    ];
    for (name, input, level) in inputs {
        let frame = compress(&input, &params(level, true, true));
        for (in_chunk, out_chunk) in [
            (1, 1 << 16),
            (7, 3),
            (13, 1),
            (4096, 64),
            (1 << 20, 1 << 20),
        ] {
            let decoded = decode_fixed(&mut decoder, &frame, in_chunk, out_chunk)
                .unwrap_or_else(|error| panic!("{name} {in_chunk}/{out_chunk}: {error}"));
            assert_eq!(decoded, input, "{name} {in_chunk}/{out_chunk}");
        }
        let mut rng = Rng::new(level as u64);
        let decoded = decode_streaming(&mut decoder, &frame, || rng.below(5_000), 777)
            .expect("random chunks");
        assert_eq!(decoded, input, "{name} random chunks");
        // Owned sessions run the same state machine.
        let mut session = Decompressor::default().into_session();
        let mut output = vec![0u8; input.len()];
        let progress = session
            .process(&frame, &mut output, mzstd::DecodeOperation::Finish)
            .expect("owned session");
        assert_eq!(progress.status, mzstd::DecoderStatus::Finished);
        assert_eq!(&output[..progress.produced], input, "{name} owned");
    }
}

#[test]
fn reference_streaming_frames_with_flushes_decode() {
    let mut decoder = Decompressor::default();
    let input = mixed(400_000, 21);
    for level in [1, 4, 12] {
        let mut steps = Vec::new();
        let mut rng = Rng::new(level as u64 + 100);
        let mut position = 0;
        while position < input.len() {
            let end = (position + 1 + rng.below(60_000)).min(input.len());
            let directive = if rng.below(3) == 0 {
                Directive::Flush
            } else {
                Directive::Continue
            };
            steps.push((&input[position..end], directive));
            position = end;
        }
        let params = Params::level(level).with(zstd_ffi::ZSTD_c_checksumFlag, 1);
        let frame = oracle::compress_stream(&params, &steps).expect("reference streams");
        assert_one_shot(
            &mut decoder,
            &frame,
            &input,
            &format!("flushed level {level}"),
        );
        let decoded = decode_fixed(&mut decoder, &frame, 1000, 5000).expect("streams");
        assert_eq!(decoded, input);
        assert_eq!(
            oracle::decompress_stream(&frame, 1000, None).expect("reference"),
            input
        );
    }
}

#[test]
fn small_windows_compact_history_without_losing_matches() {
    let input = text(1_200_000, 31);
    for window_log in [10, 12, 17] {
        let params = Params::level(7)
            .with(zstd_ffi::ZSTD_c_windowLog, window_log)
            .with(zstd_ffi::ZSTD_c_checksumFlag, 1);
        let frame = stream_unknown_size(&params, &input);
        let limits = DecodeLimits::default()
            .with_max_window_log(window_log as u8)
            .expect("valid log");
        let mut decoder = Decompressor::new(limits.into());
        let decoded = decode_fixed(&mut decoder, &frame, 3000, 1 << 15).expect("small window");
        assert_eq!(decoded, input, "window log {window_log}");
        assert_eq!(decoder.decompress(&frame).expect("one-shot"), input);
    }
}

#[test]
fn concatenated_and_skippable_frames_match_the_reference() {
    let first = text(50_000, 1);
    let second = mixed(70_000, 2);
    let mut input = skippable(0, b"metadata");
    input.extend_from_slice(&compress(&first, &params(3, true, true)));
    input.extend_from_slice(&skippable(15, &[]));
    input.extend_from_slice(&compress(&[], &params(3, false, true)));
    input.extend_from_slice(&compress(&second, &params(5, false, false)));
    input.extend_from_slice(&skippable(7, &[0xee; 300]));
    let expected = [first.as_slice(), &second].concat();
    let mut decoder = Decompressor::default();
    assert_one_shot(&mut decoder, &input, &expected, "concatenated");
    assert_eq!(
        decode_fixed(&mut decoder, &input, 5, 11).expect("streams"),
        expected
    );
    assert_eq!(
        reference_decompress(&input, expected.len()).expect("reference"),
        expected
    );

    // Single mode stops after the first frame, here the leading skippable
    // one, and the next member is the first Zstandard frame.
    let mut single = Decompressor::new(FrameMode::Single.into());
    assert!(matches!(
        single.decompress(&input),
        Err(DecodeError::TrailingData { offset: 16 })
    ));
    let mut session = single.start();
    let mut output = vec![0u8; first.len() + 10];
    let progress = session
        .process(&input[16..], &mut output, mzstd::DecodeOperation::Process)
        .expect("single frame");
    assert_eq!(progress.status, mzstd::DecoderStatus::Finished);
    assert_eq!(&output[..progress.produced], first);
    assert_eq!(session.frames_decoded(), 1);
    let members = single_members(&input).expect("decodes");
    assert_eq!(members, reference_members(&input));
    let outputs: Vec<u8> = members.into_iter().flat_map(|(output, _)| output).collect();
    assert_eq!(outputs, expected);
}

#[test]
fn window_limits_match_the_reference_streaming_limit() {
    let input = text(100_000, 5);
    let params = Params::level(3).with(zstd_ffi::ZSTD_c_windowLog, 17);
    let frame = stream_unknown_size(&params, &input);
    // The frame declares a 2^17 window.
    for log in [10u8, 16] {
        let limits = DecodeLimits::default()
            .with_max_window_log(log)
            .expect("valid");
        let mut decoder = Decompressor::new(limits.into());
        assert!(matches!(
            decoder.decompress(&frame),
            Err(DecodeError::WindowLimitExceeded {
                window_size: 0x2_0000,
                ..
            })
        ));
        assert!(oracle::decompress_stream(&frame, 1 << 20, Some(i32::from(log))).is_err());
    }
    let limits = DecodeLimits::default()
        .with_max_window_log(17)
        .expect("valid");
    assert_eq!(
        Decompressor::new(limits.into())
            .decompress(&frame)
            .expect("fits"),
        input
    );

    // A frame claiming a 2^28 window is refused by default, like the
    // reference's streaming decoder, and accepted when allowed.
    let mut wide = frame.clone();
    let descriptor = wide[4];
    assert_eq!(descriptor & 0x20, 0, "the frame has a window descriptor");
    wide[5] = 18 << 3;
    assert!(matches!(
        Decompressor::default().decompress(&wide),
        Err(DecodeError::WindowLimitExceeded {
            window_size: 0x1000_0000,
            limit: 0x0800_0000
        })
    ));
    assert!(oracle::decompress_stream(&wide, 1 << 20, None).is_err());
    let limits = DecodeLimits::default()
        .with_max_window_log(28)
        .expect("valid");
    let config = DecoderConfig::default().with_limits(limits);
    assert_eq!(
        Decompressor::new(config)
            .decompress(&wide)
            .expect("allowed"),
        input
    );
}

/// The full matrix: every level, larger inputs, long-distance matching and
/// `--ultra` windows. Too slow for the unoptimized reference.
#[test]
#[cfg_attr(debug_assertions, ignore = "slow without optimizations")]
fn reference_frames_decode_across_every_level() {
    let mut decoder = Decompressor::default();
    for (name, input) in corpora(3_000_000, 99) {
        for level in -7..=22 {
            let checksum = level % 2 == 0;
            let frame = compress(&input, &params(level, checksum, true));
            let label = format!("{name} level {level}");
            assert_eq!(decoder.decompress(&frame).expect(&label), input, "{label}");
            let decoded = decode_fixed(&mut decoder, &frame, 65_536, 100_000).expect(&label);
            assert_eq!(decoded, input, "{label} streaming");
        }
    }
    let input = mixed(20_000_000, 7);
    for (level, long) in [(3, true), (19, true), (22, false)] {
        let params = Params::level(level)
            .with(
                zstd_ffi::ZSTD_c_enableLongDistanceMatching,
                if long { 1 } else { 0 },
            )
            .with(zstd_ffi::ZSTD_c_windowLog, 27)
            .with(zstd_ffi::ZSTD_c_checksumFlag, 1);
        let frame = stream_unknown_size(&params, &input);
        let decoded = decode_fixed(&mut decoder, &frame, 1 << 17, 1 << 17).expect("long window");
        assert_eq!(decoded, input, "level {level} long {long}");
    }
}

/// Decodes `input` as consecutive single-frame members, each through a
/// fresh `FrameMode::Single` session fed everything that is left, as a
/// multi-member reader does. Returns each member's output and length.
fn single_members(input: &[u8]) -> Result<Vec<(Vec<u8>, usize)>, DecodeError> {
    let mut decoder = Decompressor::new(FrameMode::Single.into());
    let mut members = Vec::new();
    let mut rest = input;
    while !rest.is_empty() {
        let mut session = decoder.start();
        let mut output = Vec::new();
        let mut buffer = vec![0u8; 4096];
        let mut consumed = 0;
        loop {
            let progress = session
                .process(
                    &rest[consumed..],
                    &mut buffer,
                    mzstd::DecodeOperation::Finish,
                )
                .map_err(mzstd::DecodeFailure::into_error)?;
            consumed += progress.consumed;
            output.extend_from_slice(&buffer[..progress.produced]);
            if progress.status == mzstd::DecoderStatus::Finished {
                break;
            }
        }
        rest = &rest[consumed..];
        members.push((output, consumed));
    }
    Ok(members)
}

/// The reference's members of `input`: `ZSTD_decompressStream` until it
/// reports a frame complete, repeated on what is left.
fn reference_members(input: &[u8]) -> Vec<(Vec<u8>, usize)> {
    let mut members = Vec::new();
    let mut rest = input;
    while !rest.is_empty() {
        let member = oracle::decompress_first_frame(rest, None).expect("the reference decodes");
        rest = &rest[member.1..];
        members.push(member);
    }
    members
}

#[test]
fn single_mode_ends_a_member_after_a_skippable_frame_like_the_reference() {
    let frame = compress(&text(5_000, 61), &params(3, true, true));
    let meta = skippable(2, b"metadata");
    let empty = skippable(15, b"");
    for input in [
        meta.clone(),
        [&frame[..], &meta].concat(),
        [&meta[..], &frame].concat(),
        [&meta[..], &empty, &frame, &meta, &frame].concat(),
    ] {
        let expected = reference_members(&input);
        assert_eq!(single_members(&input).expect("decodes"), expected);
    }
}

#[test]
fn single_mode_one_shots_treat_a_skippable_frame_as_the_frame() {
    let frame = compress(b"payload", &params(1, false, true));
    let meta = skippable(0, b"meta");
    let mut single = Decompressor::new(FrameMode::Single.into());
    // A stream of only a skippable frame is complete and empty.
    assert_eq!(single.decompress(&meta).expect("complete"), b"");
    assert_eq!(
        single
            .decompress_to_slice(&meta, &mut [0; 4])
            .expect("complete"),
        0
    );
    // A frame after it is trailing data.
    let input = [&meta[..], &frame].concat();
    for result in [
        single.decompress(&input).map(|output| output.len()),
        single.decompress_to_slice(&input, &mut [0; 64]),
        single.decompress_to_uninit(&input, &mut [std::mem::MaybeUninit::uninit(); 64]),
    ] {
        assert!(
            matches!(result, Err(DecodeError::TrailingData { offset }) if offset == meta.len() as u64),
            "{result:?}"
        );
    }
}
