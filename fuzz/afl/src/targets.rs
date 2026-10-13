//! Target bodies, shared by the AFL binaries and the regression replay.

use mzstd::compressor::parallel::{ParallelCompressor, ParallelConfig, SegmentSize, TaskCount};
use mzstd::{Compressor, EncodeError, EncoderStatus, InputSize, Operation};
use zstd_ffi::oracle::{self, Directive};

use crate::decode_config;

/// One-shot compression is byte-identical to `ZSTD_compress2` and decodes.
pub fn differential_c(data: &[u8]) {
    let Some(decoded) = decode_config(data) else {
        return;
    };
    let mut encoder = Compressor::new(decoded.config).expect("valid configuration");
    let ours = encoder.compress(decoded.rest).expect("compression");
    let theirs = oracle::compress2(decoded.rest, &decoded.params).expect("reference compression");
    assert_eq!(ours, theirs, "{:?}", decoded.config);
    let restored = oracle::decompress(&ours, decoded.rest.len()).expect("reference decoding");
    assert_eq!(restored, decoded.rest);
}

/// A destination of arbitrary capacity yields the reference's frame or its
/// failure.
pub fn output_capacity(data: &[u8]) {
    let Some(mut decoded) = decode_config(data) else {
        return;
    };
    let Some((capacity, rest)) = decoded.rest.split_first_chunk::<2>() else {
        return;
    };
    decoded.rest = rest;
    let capacity = usize::from(u16::from_le_bytes(*capacity)) % (decoded.rest.len() + 64);
    let mut encoder = Compressor::new(decoded.config).expect("valid configuration");
    let mut dst = vec![0u8; capacity];
    let ours = encoder.compress_to_slice(decoded.rest, &mut dst);
    let theirs = oracle::compress2_capacity(decoded.rest, &decoded.params, capacity);
    match (ours, theirs) {
        (Ok(size), Ok(frame)) => assert_eq!(&dst[..size], &frame[..]),
        (Err(EncodeError::OutputTooSmall { .. }), Err(_)) => {}
        (ours, theirs) => panic!("{ours:?} vs {theirs:?}"),
    }
}

/// A session driven with arbitrary chunks, operations and output capacity
/// matches `ZSTD_compressStream2` driven the same way.
pub fn encoder_session(data: &[u8]) {
    let Some(decoded) = decode_config(data) else {
        return;
    };
    let Some((schedule, input)) = decoded.rest.split_first_chunk::<4>() else {
        return;
    };
    let output_capacity = 1 + usize::from(schedule[0]) * 97;
    let declared = schedule[1] & 1 != 0;
    let mut seed = u32::from_le_bytes([schedule[1], schedule[2], schedule[3], 0x5A]) | 1;
    let mut steps: Vec<(&[u8], Operation)> = Vec::new();
    let mut pos = 0;
    while pos < input.len() {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let len = 1 + (seed as usize % 40_000).min(input.len() - pos - 1);
        let op = if seed.is_multiple_of(7) {
            Operation::Flush
        } else {
            Operation::Continue
        };
        steps.push((&input[pos..pos + len], op));
        pos += len;
    }
    steps.push((&[], Operation::End));

    let mut params = decoded.params.clone();
    if declared {
        params = params.with_pledged(input.len() as u64);
    }
    let reference_steps: Vec<(&[u8], Directive)> = steps
        .iter()
        .map(|&(chunk, op)| {
            let directive = match op {
                Operation::Continue => Directive::Continue,
                Operation::Flush => Directive::Flush,
                Operation::End => Directive::End,
            };
            (chunk, directive)
        })
        .collect();
    let theirs = oracle::compress_stream_with_output(&params, &reference_steps, output_capacity)
        .expect("reference stream");

    let mut encoder = Compressor::new(decoded.config).expect("valid configuration");
    let size = if declared {
        InputSize::Exact(input.len() as u64)
    } else {
        InputSize::Unknown
    };
    let mut session = encoder.start(size.into());
    let mut out = Vec::new();
    let mut buffer = vec![0u8; output_capacity];
    for (mut chunk, op) in steps {
        loop {
            let progress = session.process(chunk, &mut buffer, op).expect("session");
            chunk = &chunk[progress.consumed..];
            out.extend_from_slice(&buffer[..progress.produced]);
            let done = match op {
                Operation::Continue => chunk.is_empty(),
                _ => progress.status != EncoderStatus::NeedsOutput,
            };
            if done {
                break;
            }
        }
    }
    assert!(session.is_finished());
    assert_eq!(out, theirs);
}

/// Reusing a compressor across configurations and operations never changes
/// a frame.
pub fn compressor_lifecycle(data: &[u8]) {
    let Some(first) = decode_config(data) else {
        return;
    };
    let split = first.rest.len() / 2;
    let (a, b) = first.rest.split_at(split);
    let Some(second) = decode_config(b) else {
        return;
    };
    let mut reused = Compressor::new(first.config).expect("valid configuration");
    let frame_a = reused.compress(a).expect("compression");
    reused
        .reconfigure(second.config)
        .expect("valid configuration");
    let frame_b = reused.compress(second.rest).expect("compression");
    let mut fresh_a = Compressor::new(first.config).expect("valid configuration");
    let mut fresh_b = Compressor::new(second.config).expect("valid configuration");
    assert_eq!(frame_a, fresh_a.compress(a).expect("compression"));
    assert_eq!(frame_b, fresh_b.compress(second.rest).expect("compression"));
    reused.trim(mzstd::RetentionPolicy::ReleaseAll);
    assert_eq!(reused.retained_bytes(), 0);
    assert_eq!(frame_b, reused.compress(second.rest).expect("compression"));
}

/// Parallel frames decode, and do not depend on the number of tasks.
pub fn parallel(data: &[u8]) {
    let Some(mut decoded) = decode_config(data) else {
        return;
    };
    let Some((tasks, rest)) = decoded.rest.split_first() else {
        return;
    };
    decoded.rest = rest;
    let config = ParallelConfig::default().with_segment_size(SegmentSize::MIN);
    let mut compressor =
        ParallelCompressor::new(decoded.config, config).expect("valid configuration");
    let one = compressor
        .compress(decoded.rest)
        .expect("parallel compression");
    let tasks = TaskCount::try_from(1 + usize::from(*tasks % 8)).expect("task count");
    let mut batch = compressor.prepare_slice(decoded.rest, tasks);
    for task in batch.take_tasks().into_iter().rev() {
        task.run();
    }
    assert_eq!(batch.finish().expect("parallel compression"), one);
    let restored = oracle::decompress_stream(&one, 1 << 12, Some(31)).expect("reference decoding");
    assert_eq!(restored, decoded.rest);
}

/// Configuration decoding never panics, and every accepted configuration
/// compresses.
pub fn parameter_parsing(data: &[u8]) {
    if let Some(decoded) = decode_config(data) {
        let mut encoder = Compressor::new(decoded.config).expect("valid configuration");
        let _ = encoder.compress(&decoded.rest[..decoded.rest.len().min(64)]);
    }
    for chunk in data.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        let value = i32::from_le_bytes(word);
        let _ = mzstd::Level::try_from(value);
        let _ = mzstd::Strategy::try_from(word[0]);
        let window = value.unsigned_abs() % 64;
        let config = mzstd::EncoderConfig::default()
            .with_params(mzstd::CompressionParams::default().with_window_log(window));
        assert_eq!(Compressor::new(config).is_ok(), (10..=31).contains(&window));
    }
}

/// Output a decoder target may produce before it stops comparing: run-length
/// blocks expand a few input bytes into 128 KiB, so an unbounded decode of a
/// fuzz input could run out of memory.
const DECODE_OUTPUT_LIMIT: u64 = 16 << 20;

/// A decoder accepting every window the format allows, with the output cap.
fn bounded_decoder() -> mzstd::Decompressor {
    let limits = mzstd::DecodeLimits::DEFAULT
        .with_max_window_log(mzstd::DecodeLimits::MAX_WINDOW_LOG)
        .expect("the widest window is valid")
        .with_max_output_bytes(Some(DECODE_OUTPUT_LIMIT));
    mzstd::Decompressor::from(mzstd::DecoderConfig::DEFAULT.with_limits(limits))
}

/// Decodes `input` through a session, `chunk` input bytes and `out_chunk`
/// output bytes at a time.
fn decode_in_pieces(
    decoder: &mut mzstd::Decompressor,
    input: &[u8],
    chunk: usize,
    out_chunk: usize,
) -> Result<Vec<u8>, mzstd::DecodeError> {
    use mzstd::{DecodeOperation, DecoderStatus};
    let mut session = decoder.start();
    let mut output = Vec::new();
    let mut buffer = vec![0u8; out_chunk.max(1)];
    let mut position = 0;
    loop {
        let end = (position + chunk.max(1)).min(input.len());
        let operation = if end == input.len() {
            DecodeOperation::Finish
        } else {
            DecodeOperation::Process
        };
        let progress = session
            .process(&input[position..end], &mut buffer, operation)
            .map_err(mzstd::DecodeFailure::into_error)?;
        assert!(progress.consumed <= end - position);
        assert!(progress.produced <= buffer.len());
        position += progress.consumed;
        output.extend_from_slice(&buffer[..progress.produced]);
        match progress.status {
            DecoderStatus::Finished => return Ok(output),
            DecoderStatus::NeedsInput => assert_eq!(position, end, "NeedsInput leaves input"),
            DecoderStatus::NeedsOutput => {}
        }
    }
}

/// Arbitrary bytes never make the decoder panic, and whatever it accepts the
/// reference accepts with the same output.
///
/// The decoder is stricter than the reference in documented places (Huffman
/// streams must end exactly), so a rejection is not compared.
pub fn decompress(data: &[u8]) {
    let mut decoder = bounded_decoder();
    let ours = decoder.decompress(data);
    let mut slice = vec![0u8; data.len().saturating_mul(4).min(1 << 16)];
    let sliced = decoder.decompress_to_slice(data, &mut slice);
    match (&ours, &sliced) {
        (Ok(output), Ok(written)) => assert_eq!(&slice[..*written], &output[..]),
        (Ok(output), Err(mzstd::DecodeError::OutputTooSmall { .. })) => {
            assert!(output.len() > slice.len());
            assert_eq!(slice, output[..slice.len()]);
        }
        (Ok(_), Err(error)) => panic!("one-shot accepted, slice failed with {error:?}"),
        (Err(_), _) => {}
    }
    if let Ok(output) = ours
        && !data.is_empty()
    {
        let reference = oracle::decompress_stream(data, data.len(), Some(31))
            .unwrap_or_else(|error| panic!("accepted what the reference rejects: {error}"));
        assert_eq!(output, reference);
    }
}

/// A session fed arbitrary input and output pieces agrees with one-shot
/// decoding.
pub fn decode_streaming(data: &[u8]) {
    let Some((&[chunk, out_chunk], frame)) = data.split_first_chunk::<2>() else {
        return;
    };
    let mut decoder = bounded_decoder();
    let one_shot = decoder.decompress(frame);
    let streamed = decode_in_pieces(
        &mut decoder,
        frame,
        usize::from(chunk),
        usize::from(out_chunk) * 7,
    );
    match (one_shot, streamed) {
        (Ok(one_shot), Ok(streamed)) => assert_eq!(one_shot, streamed),
        (Err(_), Err(_)) => {}
        (one_shot, streamed) => panic!(
            "one-shot {:?} vs streamed {:?}",
            one_shot.map(|output| output.len()),
            streamed.map(|output| output.len())
        ),
    }
}

/// Frames the reference writes with a fuzzed configuration decode to their
/// payload, one-shot and in pieces.
pub fn decode_roundtrip(data: &[u8]) {
    let Some(decoded) = decode_config(data) else {
        return;
    };
    let Ok(frame) = oracle::compress2(decoded.rest, &decoded.params) else {
        return;
    };
    let mut decoder = bounded_decoder();
    assert_eq!(decoder.decompress(&frame).expect("decodes"), decoded.rest);
    let chunk = 1 + usize::from(data[0]) * 13;
    let streamed = decode_in_pieces(&mut decoder, &frame, chunk, 1 + usize::from(data[1]) * 29);
    assert_eq!(streamed.expect("decodes in pieces"), decoded.rest);
}
