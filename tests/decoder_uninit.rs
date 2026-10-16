#![cfg(feature = "decompression")]
//! Decoding into uninitialized memory: `process_uninit` and
//! `decompress_to_uninit` against their initialized counterparts, with
//! sentinels proving that nothing past the reported output is written.

mod decoder_support;

use std::mem::MaybeUninit;

use decoder_support::{compress, corpora, params, skippable, text};
use mzstd::dictionary::DecodeDictionary;
use mzstd::{
    DecodeError, DecodeFailure, DecodeLimits, DecodeOperation, DecodeProgress, DecoderConfig,
    DecoderStatus, Decompressor, FrameMode,
};
use zstd_ffi::oracle::{self, Params};

/// The byte every uninitialized buffer starts with, standing in for
/// memory the decoder must not touch.
const SENTINEL: u8 = 0xA5;

fn sentinels(len: usize) -> Vec<MaybeUninit<u8>> {
    vec![MaybeUninit::new(SENTINEL); len]
}

/// Reads back a buffer that [`sentinels`] initialized in full.
fn read(buffer: &[MaybeUninit<u8>]) -> Vec<u8> {
    buffer
        .iter()
        // SAFETY: every slot holds the sentinel or a decoded byte.
        .map(|slot| unsafe { slot.assume_init() })
        .collect()
}

/// Asserts that `buffer[at..]` still holds only sentinels.
fn untouched(buffer: &[MaybeUninit<u8>], at: usize) {
    assert!(
        read(&buffer[at..]).iter().all(|&byte| byte == SENTINEL),
        "a byte past {at} was written"
    );
}

/// Which session method a call uses.
#[derive(Clone, Copy)]
enum Calls {
    Init,
    Uninit,
    Alternating,
}

/// Decodes `input` through a session `chunk` input bytes and `window`
/// output bytes at a time, returning the output and the failure, if any.
fn drive(
    decoder: &mut Decompressor,
    dictionary: Option<&DecodeDictionary>,
    input: &[u8],
    chunk: usize,
    window: usize,
    calls: Calls,
) -> (Vec<u8>, Option<DecodeFailure>) {
    let mut session = match dictionary {
        Some(dictionary) => decoder.start_with_dictionary(dictionary),
        None => decoder.start(),
    };
    let mut output = Vec::new();
    let mut buffer = vec![0u8; window];
    let mut uninit = sentinels(window);
    let mut position = 0;
    for call in 0usize.. {
        let end = (position + chunk).min(input.len());
        let operation = if end == input.len() {
            DecodeOperation::Finish
        } else {
            DecodeOperation::Process
        };
        let piece = &input[position..end];
        let uninitialized = match calls {
            Calls::Init => false,
            Calls::Uninit => true,
            Calls::Alternating => call % 2 == 1,
        };
        let result = if uninitialized {
            let result = session.process_uninit(piece, &mut uninit, operation);
            let produced = match &result {
                Ok(progress) => progress.produced,
                Err(failure) => failure.produced,
            };
            output.extend_from_slice(&read(&uninit[..produced]));
            untouched(&uninit, produced);
            uninit.fill(MaybeUninit::new(SENTINEL));
            result
        } else {
            let result = session.process(piece, &mut buffer, operation);
            let produced = match &result {
                Ok(progress) => progress.produced,
                Err(failure) => failure.produced,
            };
            output.extend_from_slice(&buffer[..produced]);
            result
        };
        let progress = match result {
            Ok(progress) => progress,
            Err(failure) => return (output, Some(failure)),
        };
        position += progress.consumed;
        if progress.status == DecoderStatus::Finished {
            break;
        }
    }
    (output, None)
}

#[test]
fn process_uninit_matches_process_across_corpora_chunks_and_windows() {
    for (name, input) in corpora(300_000, 41) {
        for level in [1, 3, 19] {
            let frame = compress(&input, &params(level, true, level != 3));
            for (chunk, window) in [(1 << 20, 1 << 20), (4096, 1000), (777, 1), (1, 131_075)] {
                let mut decoder = Decompressor::default();
                let (initialized, failure) =
                    drive(&mut decoder, None, &frame, chunk, window, Calls::Init);
                assert!(failure.is_none());
                assert_eq!(initialized, input, "{name} level {level}");
                for calls in [Calls::Uninit, Calls::Alternating] {
                    let (uninitialized, failure) =
                        drive(&mut decoder, None, &frame, chunk, window, calls);
                    assert!(failure.is_none(), "{name} level {level}");
                    assert_eq!(
                        uninitialized, input,
                        "{name} level {level} {chunk}/{window}"
                    );
                }
            }
        }
    }
}

#[test]
fn process_uninit_reports_the_same_failure_progress_as_process() {
    let input = text(200_000, 42);
    let mut frame = compress(&input, &params(3, true, true));
    let middle = frame.len() / 2;
    frame[middle] ^= 0x55;
    let checksum = frame.len() - 1;
    let mut checksum_frame = compress(&input, &params(3, true, true));
    checksum_frame[checksum] ^= 1;
    for corrupt in [&frame, &checksum_frame] {
        for window in [1000, 1 << 20] {
            let mut decoder = Decompressor::default();
            let (expected, failure) = drive(&mut decoder, None, corrupt, 5000, window, Calls::Init);
            let expected_error = failure.expect("corrupt").into_error().to_string();
            let (ours, failure) = drive(&mut decoder, None, corrupt, 5000, window, Calls::Uninit);
            assert_eq!(ours, expected);
            assert_eq!(
                failure.expect("corrupt").into_error().to_string(),
                expected_error
            );
        }
    }
}

#[test]
fn a_failed_uninit_session_stays_failed() {
    let mut decoder = Decompressor::default();
    let mut session = decoder.start();
    let mut output = sentinels(8);
    let failure = session
        .process_uninit(b"not zstd", &mut output, DecodeOperation::Process)
        .expect_err("bad magic");
    assert_eq!(failure.produced, 0);
    let failure = session
        .process_uninit(&[], &mut output, DecodeOperation::Process)
        .expect_err("failed");
    assert!(matches!(failure.into_error(), DecodeError::InvalidState));
    untouched(&output, 0);
}

#[test]
fn a_finished_uninit_session_writes_nothing() {
    let frame = compress(b"done", &params(1, false, true));
    let mut decoder = Decompressor::new(FrameMode::Single.into());
    let mut session = decoder.start();
    let mut output = sentinels(16);
    let progress = session
        .process_uninit(&frame, &mut output, DecodeOperation::Finish)
        .expect("decodes");
    assert_eq!(read(&output[..progress.produced]), b"done");
    output.fill(MaybeUninit::new(SENTINEL));
    let progress = session
        .process_uninit(&[], &mut output, DecodeOperation::Finish)
        .expect("finished");
    assert_eq!(
        progress,
        DecodeProgress {
            consumed: 0,
            produced: 0,
            status: DecoderStatus::Finished
        }
    );
    untouched(&output, 0);
    // An empty destination accepts input but writes nothing.
    let mut session = decoder.start();
    let progress = session
        .process_uninit(&frame, &mut [], DecodeOperation::Finish)
        .expect("needs room");
    assert_eq!(progress.status, DecoderStatus::NeedsOutput);
}

/// A record and its frame.
type Record = (Vec<u8>, Vec<u8>);

/// A trained dictionary and records compressed with it.
fn dictionary_records() -> (DecodeDictionary, Vec<Record>) {
    let training = zstd_ffi::corpus::records(500, 5);
    let samples: Vec<&[u8]> = training.iter().map(Vec::as_slice).collect();
    let bytes = oracle::train_dictionary(&samples, 8 << 10).expect("trains");
    let reference = oracle::Dictionary::auto(&bytes);
    let params = Params::level(3).with_dictionary(reference);
    let records = zstd_ffi::corpus::records(20, 6)
        .into_iter()
        .map(|record| {
            let frame = oracle::compress2(&record, &params).expect("reference");
            (record, frame)
        })
        .collect();
    (
        DecodeDictionary::try_from(&bytes[..]).expect("valid"),
        records,
    )
}

#[test]
fn owned_and_dictionary_sessions_decode_into_uninitialized_memory() {
    let (dictionary, records) = dictionary_records();
    let mut decoder = Decompressor::default();
    for (record, frame) in &records {
        let (ours, failure) = drive(
            &mut decoder,
            Some(&dictionary),
            frame,
            7,
            13,
            Calls::Alternating,
        );
        assert!(failure.is_none());
        assert_eq!(&ours, record);
    }
    let mut session = Decompressor::default().into_session_with_dictionary(dictionary);
    for (record, frame) in &records {
        let mut output = sentinels(record.len() + 8);
        let progress = session
            .process_uninit(frame, &mut output, DecodeOperation::Finish)
            .expect("decodes");
        assert_eq!(progress.status, DecoderStatus::Finished);
        assert_eq!(&read(&output[..progress.produced]), record);
        untouched(&output, progress.produced);
        session.reinit();
    }
    let input = text(50_000, 43);
    let frame = compress(&input, &params(5, true, false));
    let mut session = Decompressor::default().into_session();
    let mut output = sentinels(input.len());
    let progress = session
        .process_uninit(&frame, &mut output, DecodeOperation::Finish)
        .expect("decodes");
    assert_eq!(progress.status, DecoderStatus::Finished);
    assert_eq!(read(&output), input);
}

/// `decompress_to_uninit` and `decompress_to_slice` on `input` into `len`
/// bytes agree: on the result, on the written prefix, and on a full
/// destination after `OutputTooSmall`. On success nothing past the output
/// is written.
fn one_shots_agree(decoder: &mut Decompressor, input: &[u8], len: usize) {
    let mut slice = vec![0u8; len];
    let expected = decoder.decompress_to_slice(input, &mut slice);
    let mut uninit = sentinels(len);
    let ours = decoder.decompress_to_uninit(input, &mut uninit);
    match (&expected, &ours) {
        (Ok(expected), Ok(written)) => {
            assert_eq!(written, expected);
            assert_eq!(read(&uninit[..*written]), slice[..*written]);
            untouched(&uninit, *written);
        }
        (
            Err(DecodeError::OutputTooSmall { written: a }),
            Err(DecodeError::OutputTooSmall { written: b }),
        ) => {
            assert_eq!(a, b);
            assert_eq!(*b, len);
            assert_eq!(read(&uninit), slice);
        }
        (Err(expected), Err(ours)) => assert_eq!(ours.to_string(), expected.to_string()),
        _ => panic!("slice {expected:?} vs uninit {ours:?}"),
    }
}

#[test]
fn decompress_to_uninit_matches_decompress_to_slice() {
    let mut decoder = Decompressor::default();
    for (name, input) in corpora(400_000, 44) {
        for (level, content_size) in [(1, true), (3, false), (19, true)] {
            let frame = compress(&input, &params(level, true, content_size));
            for len in [input.len(), input.len() + 100, input.len() - 1, 1000, 0] {
                one_shots_agree(&mut decoder, &frame, len);
            }
            let mut written = sentinels(input.len());
            let size = decoder
                .decompress_to_uninit(&frame, &mut written)
                .expect("fits");
            assert_eq!(size, input.len(), "{name}");
            assert_eq!(read(&written), input, "{name} level {level}");
        }
    }
}

#[test]
fn decompress_to_uninit_handles_frame_sequences_and_malformed_input() {
    let a = text(70_000, 45);
    let b = decoder_support::runs(90_000, 46);
    let declared = compress(&a, &params(3, true, true));
    let undeclared = compress(&b, &params(3, false, false));
    let total = a.len() + b.len();
    let mut decoder = Decompressor::default();
    // Every frame declared: the direct path. Then a frame without a
    // declared size after a declared one: the direct path gives up after
    // writing the first frame and the incremental path decodes both.
    for input in [
        [&declared[..], &skippable(1, b"meta"), &declared[..]].concat(),
        [&declared[..], &undeclared[..]].concat(),
    ] {
        for len in [2 * total, total, a.len(), a.len() + 1, 10] {
            one_shots_agree(&mut decoder, &input, len);
        }
    }
    let mut truncated = declared.clone();
    truncated.truncate(declared.len() - 3);
    let mut trailing = declared.clone();
    trailing.push(0);
    for input in [&truncated[..], &[][..], &declared[..10], &skippable(0, b"")] {
        one_shots_agree(&mut decoder, input, a.len());
    }
    let mut single = Decompressor::new(FrameMode::Single.into());
    one_shots_agree(&mut single, &trailing, 2 * a.len());
    let limits = DecodeLimits::default().with_max_output_bytes(Some(1000));
    let mut limited = Decompressor::new(DecoderConfig::default().with_limits(limits));
    one_shots_agree(&mut limited, &declared, a.len());
}

#[test]
fn dictionary_one_shots_decode_into_uninitialized_memory() {
    let (dictionary, records) = dictionary_records();
    let mut decoder = Decompressor::default();
    for (record, frame) in &records {
        let mut output = sentinels(record.len() + 3);
        let written = decoder
            .decompress_with_dictionary_to_uninit(&dictionary, frame, &mut output)
            .expect("decodes");
        assert_eq!(&read(&output[..written]), record);
        untouched(&output, written);
        assert!(matches!(
            decoder.decompress_with_dictionary_to_uninit(
                &dictionary,
                frame,
                &mut sentinels(record.len() - 1)
            ),
            Err(DecodeError::OutputTooSmall { .. })
        ));
    }
}
