#![cfg(feature = "compression")]
//! `ZSTD_c_targetCBlockSize` through the public API: every frame is the one
//! `ZSTD_compress2` / `ZSTD_compressStream2` write for the same parameters
//! (the superblock path of `zstd_compress_superblock.c`).

use std::num::NonZeroU32;

use mzstd::dictionary::{DictionaryBuilder, DictionaryContent, PreparedDictionary};
use mzstd::{
    Backend, CParameter, Compressor, ConfigError, DictionaryAttach, EncoderConfig, EncoderStatus,
    InputSize, LdmParams, Level, LongDistanceMatching, Operation, Parameter, StreamConfig, Switch,
};
use zstd_ffi::corpus;
use zstd_ffi::oracle::{self, Dictionary, Directive, Params};
use zstd_ffi::*;

/// `ZSTD_TARGETCBLOCKSIZE_MIN` and `ZSTD_TARGETCBLOCKSIZE_MAX`.
const MIN: u32 = 1340;
const MAX: u32 = 131_072;

/// Targets at and around the bounds and the usual sizes.
const TARGETS: [u32; 5] = [MIN, 2048, 16_384, 65_536, MAX];

fn level(value: i32) -> Level {
    Level::try_from(value).expect("a level")
}

fn config(level_value: i32, target: u32) -> EncoderConfig {
    EncoderConfig::default()
        .with_level(level(level_value))
        .with_target_compressed_block_size(NonZeroU32::new(target))
}

fn reference(level_value: i32, target: u32) -> Params {
    Params::level(level_value).with(ZSTD_c_targetCBlockSize, target as i32)
}

/// Input shapes of the sweep.
#[derive(Copy, Clone, Debug)]
enum Shape {
    Text,
    Noise,
    Constant,
    Records,
    Mixed,
    Binary,
    /// Text, a noise block, a constant block, then text again: blocks that
    /// are compressed, stored and RLE in one frame.
    Patchwork,
}

const SHAPES: [Shape; 7] = [
    Shape::Text,
    Shape::Noise,
    Shape::Constant,
    Shape::Records,
    Shape::Mixed,
    Shape::Binary,
    Shape::Patchwork,
];

fn input(shape: Shape, len: usize, seed: u64) -> Vec<u8> {
    match shape {
        Shape::Text => corpus::text(len, seed),
        Shape::Noise => corpus::random(len, seed),
        Shape::Constant => vec![b'z'; len],
        Shape::Records => {
            let mut out = corpus::records(len / 60 + 1, seed).concat();
            out.resize(len, b'\n');
            out
        }
        Shape::Mixed => corpus::mixed(len, seed),
        Shape::Binary => corpus::binary(len, seed),
        Shape::Patchwork => {
            let mut out = Vec::with_capacity(len);
            let mut part = 0;
            while out.len() < len {
                let piece = (1usize << 17).min(len - out.len());
                out.extend(match part % 4 {
                    0 | 3 => corpus::text(piece, seed + part as u64),
                    1 => corpus::random(piece, seed + part as u64),
                    _ => vec![0x5a; piece],
                });
                part += 1;
            }
            out
        }
    }
}

fn assert_same_frame(config: EncoderConfig, params: &Params, src: &[u8], what: &str) {
    let expected = oracle::compress2(src, params).expect("the reference compresses");
    let mut encoder = Compressor::new(config).expect("a valid configuration");
    let ours = encoder.compress(src).expect("compresses");
    assert!(
        ours == expected,
        "{what}: {} bytes, ours {} bytes, reference {} bytes",
        src.len(),
        ours.len(),
        expected.len()
    );
    assert_eq!(
        oracle::decompress(&ours, src.len()).expect("decodes"),
        src,
        "{what}: round trip"
    );
}

#[test]
fn every_level_writes_the_reference_frame_for_every_shape() {
    for value in (-5..=22).chain([0]) {
        for (index, shape) in SHAPES.into_iter().enumerate() {
            let src = input(shape, 150_000 + 7 * index, 31 + index as u64);
            for target in [MIN, 4096, MAX] {
                assert_same_frame(
                    config(value, target),
                    &reference(value, target),
                    &src,
                    &format!("level {value}, target {target}, {shape:?}"),
                );
            }
        }
    }
}

#[test]
fn targets_at_and_around_the_bounds_write_the_reference_frame() {
    let src = corpus::mixed(300_000, 3);
    let mut targets = vec![1, 2, 100, 1339, 1340, 1341, 1342, 1500, 2047, 2048, 2049];
    targets.extend([16_383, 16_384, 16_385, 65_535, 65_536, 65_537]);
    targets.extend([100_000, 131_070, 131_071, 131_072]);
    for value in [-3, 1, 3, 5, 9, 13, 19] {
        for &target in &targets {
            assert_same_frame(
                config(value, target),
                &reference(value, target),
                &src,
                &format!("level {value}, target {target}"),
            );
        }
    }
}

/// A structured input made of pieces that stress the sub-block decisions:
/// constant runs, tiny and skewed alphabets, repeats of earlier bytes,
/// text and noise.
fn patchwork(rng: &mut corpus::Rng, len: usize) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(len + 4096);
    while out.len() < len {
        let piece = 1 + (rng.next_u64() % 6000) as usize;
        match rng.next_u64() % 8 {
            0 => out.extend(std::iter::repeat_n(rng.next_u64() as u8, piece)),
            1 => {
                let alphabet = 2 + rng.next_u64() % 14;
                out.extend((0..piece).map(|_| (rng.next_u64() % alphabet) as u8));
            }
            2 => out.extend((0..piece).map(|_| {
                // A geometric-like skew toward small byte values.
                let r = rng.next_u64();
                (r.trailing_zeros() as u8).wrapping_mul(17) ^ (r >> 60) as u8
            })),
            3 | 4 if out.len() > 16 => {
                let from = (rng.next_u64() as usize) % (out.len() - 8);
                let take = piece.min(out.len() - from);
                let copy = out[from..from + take].to_vec();
                out.extend(copy);
            }
            5 => out.extend(corpus::text(piece, rng.next_u64())),
            6 => out.extend(corpus::random(piece, rng.next_u64())),
            _ => out.extend(corpus::binary(piece, rng.next_u64())),
        }
    }
    out.truncate(len);
    out
}

#[test]
fn random_structured_inputs_write_the_reference_frame() {
    let mut rng = corpus::Rng::new(0x5eed_cb10);
    let targets = [1, 1340, 1500, 2048, 3000, 4096, 9000, 16_384, 40_000, MAX];
    for round in 0..400 {
        let len = match round % 4 {
            0 => 1 + (rng.next_u64() % 4000) as usize,
            1 => 4000 + (rng.next_u64() % 60_000) as usize,
            2 => 60_000 + (rng.next_u64() % 140_000) as usize,
            _ => 130_000 + (rng.next_u64() % 400_000) as usize,
        };
        let src = patchwork(&mut rng, len);
        let value = (rng.next_u64() % 25) as i32 - 5;
        let target = targets[(rng.next_u64() % targets.len() as u64) as usize];
        assert_same_frame(
            config(value, target),
            &reference(value, target),
            &src,
            &format!("round {round}: level {value}, target {target}"),
        );
    }
}

#[test]
fn a_target_below_the_minimum_is_the_minimum() {
    let src = corpus::text(200_000, 8);
    for value in [1, 3, 12] {
        let at_min = Compressor::new(config(value, MIN))
            .unwrap()
            .compress(&src)
            .unwrap();
        for target in [1, 7, 1339] {
            let below = Compressor::new(config(value, target))
                .unwrap()
                .compress(&src)
                .unwrap();
            assert_eq!(below, at_min, "level {value}, target {target}");
        }
        // And a target is not the same as none.
        let none = Compressor::new(config(value, 0))
            .unwrap()
            .compress(&src)
            .unwrap();
        assert_ne!(none, at_min);
    }
}

#[test]
fn sizes_around_block_boundaries_write_the_reference_frame() {
    let sizes = [
        0usize, 1, 2, 3, 6, 7, 8, 63, 64, 1339, 1340, 1341, 4095, 4096, 65_535, 65_536, 131_071,
        131_072, 131_073, 262_143, 262_144, 262_145, 400_001,
    ];
    for value in [-5, 1, 3, 7, 12, 19] {
        for target in [MIN, 2048, 16_384, MAX] {
            for (index, &len) in sizes.iter().enumerate() {
                for shape in [Shape::Text, Shape::Mixed] {
                    let src = input(shape, len, index as u64);
                    assert_same_frame(
                        config(value, target),
                        &reference(value, target),
                        &src,
                        &format!("level {value}, target {target}, {shape:?}"),
                    );
                }
            }
        }
    }
}

#[test]
fn multi_mebibyte_inputs_write_the_reference_frame() {
    for (shape, len) in [
        (Shape::Text, 3 << 20),
        (Shape::Mixed, 5 << 20),
        (Shape::Patchwork, 2 << 20),
    ] {
        let src = input(shape, len, 77);
        for value in [1, 3, 9, 17] {
            for target in [MIN, 16_384] {
                assert_same_frame(
                    config(value, target),
                    &reference(value, target),
                    &src,
                    &format!("level {value}, target {target}, {shape:?}"),
                );
            }
        }
    }
}

#[test]
fn incompressible_and_constant_blocks_write_the_reference_frame() {
    for value in [-5, 1, 3, 9, 19] {
        for target in TARGETS {
            for shape in [Shape::Noise, Shape::Constant, Shape::Patchwork] {
                for len in [1 << 17, (1 << 17) + 1, 3 << 17, 1_000_000] {
                    let src = input(shape, len, 5);
                    assert_same_frame(
                        config(value, target),
                        &reference(value, target),
                        &src,
                        &format!("level {value}, target {target}, {shape:?}"),
                    );
                }
            }
        }
    }
}

#[test]
fn the_checksum_and_the_content_size_flag_are_unaffected() {
    let src = corpus::text(300_000, 4);
    for value in [1, 6, 15] {
        for (checksum, content_size) in [(true, true), (true, false), (false, false)] {
            for target in [MIN, 8192] {
                let params = reference(value, target)
                    .with(ZSTD_c_checksumFlag, i32::from(checksum))
                    .with(ZSTD_c_contentSizeFlag, i32::from(content_size));
                assert_same_frame(
                    config(value, target)
                        .with_checksum(checksum)
                        .with_content_size(content_size),
                    &params,
                    &src,
                    &format!("level {value}, target {target}, checksum {checksum}"),
                );
            }
        }
    }
}

#[test]
fn long_distance_matching_writes_the_reference_frame() {
    // Repeats far apart, which only the long distance matcher finds cheaply.
    let unit = corpus::mixed(100_000, 6);
    let mut src = Vec::new();
    for round in 0..8 {
        src.extend_from_slice(&unit);
        src.extend_from_slice(&corpus::random(20_000, 100 + round));
    }
    for value in [3, 9, 16] {
        for target in [MIN, 16_384] {
            let ldm = LongDistanceMatching::Enabled(LdmParams::default().with_min_match(32));
            let params = reference(value, target)
                .with(ZSTD_c_enableLongDistanceMatching, 1)
                .with(ZSTD_c_ldmMinMatch, 32);
            assert_same_frame(
                config(value, target).with_long_distance_matching(ldm),
                &params,
                &src,
                &format!("level {value}, target {target}"),
            );
        }
    }
}

#[test]
fn the_target_replaces_the_block_splitters() {
    let src = corpus::text(600_000, 12);
    for value in [3, 9, 15] {
        for target in [MIN, 16_384] {
            // `splitAfterSequences` is the post-match splitter, and
            // `blockSplitterLevel` the one before matching.
            for (pairs, config) in [
                (
                    vec![(ZSTD_c_splitAfterSequences, 1)],
                    config(value, target).with_post_block_splitter(Switch::Enabled),
                ),
                (
                    vec![(ZSTD_c_splitAfterSequences, 2)],
                    config(value, target).with_post_block_splitter(Switch::Disabled),
                ),
                (
                    vec![(ZSTD_c_blockSplitterLevel, 6)],
                    config(value, target).with_block_splitter_level(6),
                ),
                (
                    vec![(ZSTD_c_blockSplitterLevel, 1)],
                    config(value, target).with_block_splitter_level(1),
                ),
            ] {
                let params = pairs
                    .iter()
                    .fold(reference(value, target), |params, &(id, v)| {
                        params.with(id, v)
                    });
                assert_same_frame(
                    config,
                    &params,
                    &src,
                    &format!("level {value}, target {target}, {pairs:?}"),
                );
            }
        }
    }
}

#[test]
fn a_smaller_maximum_block_size_writes_the_reference_frame() {
    let src = corpus::mixed(400_000, 14);
    for value in [1, 5, 12] {
        for max_block in [1024, 4096, 65_536, 131_072] {
            for target in [MIN, 2048, 16_384, MAX] {
                let params = reference(value, target).with(ZSTD_c_maxBlockSize, max_block);
                assert_same_frame(
                    config(value, target).with_max_block_size(Some(max_block as u32)),
                    &params,
                    &src,
                    &format!("level {value}, max block {max_block}, target {target}"),
                );
            }
        }
    }
}

#[test]
fn every_backend_writes_the_reference_frame() {
    let src = corpus::mixed(300_000, 17);
    for value in [-5, 1, 3, 9, 19] {
        for target in [MIN, 16_384] {
            let expected = oracle::compress2(&src, &reference(value, target)).unwrap();
            for backend in Backend::available() {
                let mut encoder = Compressor::builder(config(value, target))
                    .with_backend(backend)
                    .build()
                    .unwrap();
                assert!(
                    encoder.compress(&src).unwrap() == expected,
                    "{backend}, level {value}, target {target}"
                );
            }
        }
    }
}

#[test]
fn a_reused_compressor_keeps_no_state_between_frames() {
    let inputs = [
        corpus::text(300_000, 1),
        corpus::random(200_000, 2),
        vec![7u8; 300_000],
        corpus::records(3000, 3).concat(),
        Vec::new(),
        corpus::text(10, 4),
    ];
    for value in [1, 3, 9, 19] {
        let mut encoder = Compressor::new(config(value, 4096)).unwrap();
        for src in inputs.iter().chain(inputs.iter().rev()) {
            let expected = oracle::compress2(src, &reference(value, 4096)).unwrap();
            assert!(encoder.compress(src).unwrap() == expected, "level {value}");
        }
    }
}

#[test]
fn the_output_modes_write_the_one_shot_frame() {
    let src = corpus::text(300_000, 21);
    for value in [1, 9] {
        let expected = oracle::compress2(&src, &reference(value, MIN)).unwrap();
        let mut encoder = Compressor::new(config(value, MIN)).unwrap();
        let mut slice = vec![0xa5; Compressor::max_compressed_size(src.len()).unwrap()];
        let written = encoder.compress_to_slice(&src, &mut slice).unwrap();
        assert_eq!(slice[..written], expected[..]);
        let mut uninit = Vec::with_capacity(slice.len());
        let written = encoder
            .compress_to_uninit(&src, uninit.spare_capacity_mut())
            .unwrap();
        // SAFETY: the call initialized the first `written` bytes.
        unsafe { uninit.set_len(written) };
        assert_eq!(uninit, expected);
        let mut appended = b"prefix".to_vec();
        let range = encoder.compress_into(&src, &mut appended).unwrap();
        assert_eq!(appended[range], expected[..]);
    }
}

/// Free of the C reference, so Miri can run it: the superblock path writes
/// into uninitialized memory, a slice and a `Vec` alike.
#[test]
fn uninitialized_and_initialized_outputs_hold_the_same_superblock_frame() {
    let len = if cfg!(miri) { 5_000 } else { 400_000 };
    let src = corpus::mixed(len, 22);
    let levels: &[i32] = if cfg!(miri) { &[3] } else { &[1, 6] };
    for &value in levels {
        let mut encoder = Compressor::new(config(value, MIN)).unwrap();
        let frame = encoder.compress(&src).unwrap();
        let bound = Compressor::max_compressed_size(src.len()).unwrap();
        let mut uninit = Vec::with_capacity(bound);
        let written = encoder
            .compress_to_uninit(&src, uninit.spare_capacity_mut())
            .unwrap();
        // SAFETY: the call initialized the first `written` bytes.
        unsafe { uninit.set_len(written) };
        assert_eq!(uninit, frame);
        let mut slice = vec![0xa5; bound];
        let written = encoder.compress_to_slice(&src, &mut slice).unwrap();
        assert_eq!(slice[..written], frame[..]);
    }
}

#[test]
fn output_capacity_boundaries_agree_with_the_reference() {
    let inputs = [
        corpus::random(63, 71),
        corpus::random(3000, 72),
        corpus::text(70_000, 73),
        corpus::mixed(200_000, 74),
        corpus::random(140_000, 75),
        vec![1u8; 300_000],
    ];
    for value in [-5, 1, 3, 9, 16] {
        for target in [MIN, 16_384] {
            let config = config(value, target);
            let params = reference(value, target);
            let mut encoder = Compressor::new(config).unwrap();
            for src in &inputs {
                let frame = oracle::compress2(src, &params).unwrap();
                let mut capacities = vec![
                    0,
                    5,
                    src.len().saturating_sub(1),
                    src.len(),
                    src.len() + 3,
                    src.len() + 20,
                    frame.len() - 1,
                    frame.len(),
                    frame.len() + 1,
                    frame.len() + 100,
                    Compressor::max_compressed_size(src.len()).unwrap(),
                ];
                capacities.sort_unstable();
                capacities.dedup();
                for capacity in capacities {
                    let theirs = oracle::compress2_capacity(src, &params, capacity);
                    let mut out = vec![0xa5; capacity];
                    let ours = encoder.compress_to_slice(src, &mut out);
                    assert_eq!(
                        ours.is_ok(),
                        theirs.is_ok(),
                        "level {value}, target {target}, {} bytes, capacity {capacity}",
                        src.len()
                    );
                    if let Ok(written) = ours {
                        assert!(out[..written] == theirs.unwrap()[..]);
                    }
                }
                assert!(encoder.compress(src).unwrap() == frame);
            }
        }
    }
}

/// zstd 1.5.7 writes past a destination somewhat smaller than a
/// target-block frame (see `zstd-ffi/tests/oracle_capacity_overrun.rs`):
/// at every capacity where the reference overruns, mzstd stays inside the
/// buffer and fails with `OutputTooSmall`, and at every other capacity
/// it matches the reference. The 1,273-byte input reproduces the
/// `output_capacity` fuzz finding of 2026-11-09.
#[test]
fn capacities_the_reference_overruns_fail_with_output_too_small() {
    let config = config(4, MAX)
        .with_checksum(true)
        .with_content_size(false)
        .with_params(
            mzstd::CompressionParams::default()
                .with_search_log(4)
                .with_hash_log(12)
                .with_chain_log(12),
        );
    let params = reference(4, MAX)
        .with(ZSTD_c_checksumFlag, 1)
        .with(ZSTD_c_contentSizeFlag, 0)
        .with(ZSTD_c_searchLog, 4)
        .with(ZSTD_c_hashLog, 12)
        .with(ZSTD_c_chainLog, 12);
    let inputs = [
        corpus::text(1_273, 76),
        corpus::text(1_300, 7),
        corpus::mixed(5_000, 77),
    ];
    let mut overruns = 0;
    let mut encoder = Compressor::new(config).unwrap();
    for src in &inputs {
        let frame = oracle::compress2(src, &params).unwrap();
        assert!(encoder.compress(src).unwrap() == frame);
        for capacity in 0..frame.len() + 16 {
            let theirs = oracle::compress2_capacity(src, &params, capacity);
            let mut out = vec![0xa5; capacity];
            let ours = encoder.compress_to_slice(src, &mut out);
            match (&ours, &theirs) {
                (Ok(written), Ok(theirs)) => assert!(out[..*written] == theirs[..]),
                (Err(mzstd::EncodeError::OutputTooSmall { .. }), Err(error)) => {
                    overruns += usize::from(error == oracle::CAPACITY_OVERRUN);
                }
                _ => panic!("capacity {capacity}: {ours:?} vs {theirs:?}"),
            }
        }
    }
    assert!(overruns > 0, "the inputs reach the reference's overrun");
}

// ---- streaming --------------------------------------------------------

/// Drives a session with a fresh 128 KiB output buffer per call, as
/// `oracle::compress_stream` does.
fn drive(
    mut session: mzstd::EncoderSession<'_>,
    steps: &[(&[u8], Operation)],
    capacity: usize,
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chunk = vec![0u8; capacity];
    let mut all = steps.to_vec();
    if all.last().is_none_or(|&(_, op)| op != Operation::End) {
        all.push((&[], Operation::End));
    }
    for (mut data, op) in all {
        loop {
            let progress = session.process(data, &mut chunk, op).unwrap();
            data = &data[progress.consumed..];
            out.extend_from_slice(&chunk[..progress.produced]);
            let done = match op {
                Operation::Continue => data.is_empty(),
                _ => progress.status != EncoderStatus::NeedsOutput,
            };
            if done {
                break;
            }
        }
    }
    assert!(session.is_finished());
    out
}

fn directive(op: Operation) -> Directive {
    match op {
        Operation::Continue => Directive::Continue,
        Operation::Flush => Directive::Flush,
        _ => Directive::End,
    }
}

fn chunked(src: &[u8], chunk: usize, flush_every: usize) -> Vec<(&[u8], Operation)> {
    src.chunks(chunk)
        .enumerate()
        .map(|(i, data)| {
            let op = if flush_every != 0 && i % flush_every == flush_every - 1 {
                Operation::Flush
            } else {
                Operation::Continue
            };
            (data, op)
        })
        .collect()
}

#[test]
fn streaming_with_flushes_writes_the_reference_frame() {
    let src = corpus::mixed(500_000, 9);
    for value in [-2, 1, 3, 9, 19] {
        for target in [MIN, 16_384, MAX] {
            let mut encoder = Compressor::new(config(value, target)).unwrap();
            for (chunk, flush_every) in [(70_001, 3), (1000, 1), (4099, 0), (200_000, 2)] {
                for size in [InputSize::Unknown, InputSize::Exact(src.len() as u64)] {
                    let steps = chunked(&src, chunk, flush_every);
                    let theirs_steps: Vec<(&[u8], Directive)> = steps
                        .iter()
                        .map(|&(data, op)| (data, directive(op)))
                        .collect();
                    let mut params = reference(value, target);
                    if let InputSize::Exact(n) = size {
                        params = params.with_pledged(n);
                    }
                    let theirs = oracle::compress_stream(&params, &theirs_steps).unwrap();
                    let session = encoder.start(size.into());
                    let ours = drive(session, &steps, 1 << 17);
                    assert!(
                        ours == theirs,
                        "level {value}, target {target}, chunks of {chunk} flushed every \
                         {flush_every}, {size:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn streaming_into_small_output_buffers_writes_the_reference_frame() {
    let src = corpus::text(300_000, 10);
    for value in [1, 9] {
        let params = reference(value, MIN);
        let steps = chunked(&src, 33_333, 2);
        let theirs_steps: Vec<(&[u8], Directive)> = steps
            .iter()
            .map(|&(data, op)| (data, directive(op)))
            .collect();
        let theirs = oracle::compress_stream(&params, &theirs_steps).unwrap();
        let mut encoder = Compressor::new(config(value, MIN)).unwrap();
        for capacity in [1, 100, 1500, 4096, 200_000] {
            let ours = drive(encoder.start(StreamConfig::default()), &steps, capacity);
            assert!(
                ours == theirs,
                "level {value}, output buffers of {capacity}"
            );
        }
    }
}

// ---- dictionaries -----------------------------------------------------

fn trained() -> (Vec<u8>, Vec<Vec<u8>>) {
    let records = corpus::records(4000, 3);
    let samples: Vec<&[u8]> = records[..3000].iter().map(Vec::as_slice).collect();
    let dictionary = oracle::train_dictionary(&samples, 24 << 10).expect("trains");
    (dictionary, records[3000..].to_vec())
}

fn prepare(bytes: &[u8], content: DictionaryContent, config: EncoderConfig) -> PreparedDictionary {
    DictionaryBuilder::new(bytes)
        .with_content(content)
        .with_config(config)
        .build()
        .expect("prepares")
}

#[test]
fn dictionaries_write_the_reference_frame() {
    let (dictionary, records) = trained();
    let raw = corpus::text(30_000, 8);
    let inputs = [
        records[0].clone(),
        records[..20].concat(),
        records[..400].concat(),
        records.concat(),
        corpus::text(200_000, 4),
    ];
    for value in [-3, 1, 3, 6, 9, 14, 19] {
        for (bytes, content, content_type) in [
            (&dictionary, DictionaryContent::Auto, ZSTD_dct_auto),
            (&dictionary, DictionaryContent::Raw, ZSTD_dct_rawContent),
            (&raw, DictionaryContent::Auto, ZSTD_dct_auto),
        ] {
            for target in [MIN, 4096] {
                let config = config(value, target);
                let prepared = prepare(bytes, content, config);
                let mut encoder = Compressor::new(config).unwrap();
                let params = reference(value, target).with_dictionary(Dictionary {
                    bytes: bytes.clone(),
                    content_type,
                });
                for src in &inputs {
                    let expected = oracle::compress2(src, &params).unwrap();
                    let ours = encoder.compress_with_dictionary(&prepared, src).unwrap();
                    assert!(
                        ours == expected,
                        "level {value}, {content:?}, target {target}, {} bytes",
                        src.len()
                    );
                }
            }
        }
    }
}

#[test]
fn dictionary_attach_modes_write_the_reference_frame() {
    let (dictionary, records) = trained();
    let src = records[..300].concat();
    for value in [1, 5, 12] {
        for (attach, id) in [
            (DictionaryAttach::Attach, 1),
            (DictionaryAttach::Copy, 2),
            (DictionaryAttach::Load, 3),
        ] {
            let config = config(value, MIN).with_dictionary_attach(attach);
            let prepared = prepare(&dictionary, DictionaryContent::Auto, config);
            let params = reference(value, MIN)
                .with(ZSTD_c_forceAttachDict, id)
                .with_dictionary(Dictionary::auto(&dictionary));
            let expected = oracle::compress2(&src, &params).unwrap();
            let ours = Compressor::new(config)
                .unwrap()
                .compress_with_dictionary(&prepared, &src)
                .unwrap();
            assert!(ours == expected, "level {value}, {attach:?}");
        }
    }
}

#[test]
fn streaming_with_a_dictionary_writes_the_reference_frame() {
    let (dictionary, records) = trained();
    let src = records.concat();
    for value in [1, 3, 9, 19] {
        for target in [MIN, 8192] {
            let config = config(value, target);
            let prepared = prepare(&dictionary, DictionaryContent::Auto, config);
            let mut encoder = Compressor::new(config).unwrap();
            let steps = chunked(&src, 30_000, 2);
            let theirs_steps: Vec<(&[u8], Directive)> = steps
                .iter()
                .map(|&(data, op)| (data, directive(op)))
                .collect();
            let params = reference(value, target).with_dictionary(Dictionary::auto(&dictionary));
            let theirs = oracle::compress_stream(&params, &theirs_steps).unwrap();
            let session = encoder
                .start_with_dictionary(&prepared, StreamConfig::default())
                .unwrap();
            let ours = drive(session, &steps, 1 << 17);
            assert!(ours == theirs, "level {value}, target {target}");
        }
    }
}

// ---- configuration ----------------------------------------------------

#[test]
fn the_target_is_off_by_default_and_read_back() {
    assert_eq!(
        EncoderConfig::default().target_compressed_block_size(),
        None
    );
    let config = EncoderConfig::default().with_target_compressed_block_size(NonZeroU32::new(5));
    // The value is kept as set; the minimum applies when compressing.
    assert_eq!(config.target_compressed_block_size(), NonZeroU32::new(5));
    assert!(Compressor::new(config).is_ok());
    let cleared = config.with_target_compressed_block_size(None);
    assert_eq!(cleared, EncoderConfig::default());
}

#[test]
fn a_target_above_the_maximum_is_refused_when_building() {
    for target in [MAX + 1, 1 << 20, u32::MAX] {
        let error = Compressor::new(config(3, target)).unwrap_err();
        assert_eq!(
            error,
            ConfigError::OutOfRange {
                parameter: Parameter::TargetCompressedBlockSize,
                value: i64::from(target),
                min: i64::from(MIN),
                max: i64::from(MAX),
            }
        );
        assert_eq!(
            error.to_string(),
            format!("target compressed block size {target} is outside 1340..=131072")
        );
    }
    assert!(Compressor::new(config(3, MAX)).is_ok());
    assert!(Compressor::new(config(3, MIN)).is_ok());
}

#[test]
fn the_parameter_by_id_accepts_and_refuses_what_the_reference_does() {
    for value in [
        i32::MIN,
        -131_073,
        -1,
        0,
        1,
        1339,
        1340,
        1341,
        65_536,
        131_071,
        131_072,
        131_073,
        1 << 20,
        i32::MAX,
    ] {
        let reference = oracle::set_parameter(ZSTD_c_targetCBlockSize, value);
        let ours = CParameter::try_from((ZSTD_c_targetCBlockSize, value));
        assert_eq!(reference.is_ok(), ours.is_ok(), "value {value}");
        if let Ok(parameter) = ours {
            // The reference stores the raised value; so does the typed form.
            let expected = if value == 0 { 0 } else { value.max(1340) };
            assert_eq!(
                <(i32, i32)>::from(parameter),
                (ZSTD_c_targetCBlockSize, expected),
                "value {value}"
            );
            let applied = EncoderConfig::default().with_parameter(parameter);
            assert_eq!(
                applied.target_compressed_block_size(),
                NonZeroU32::new(expected as u32)
            );
        } else {
            assert_eq!(
                ours,
                Err(ConfigError::OutOfRange {
                    parameter: Parameter::TargetCompressedBlockSize,
                    value: i64::from(value),
                    min: i64::from(MIN),
                    max: i64::from(MAX),
                })
            );
        }
    }
}

#[test]
fn a_target_set_by_id_writes_the_reference_frame_and_the_last_value_wins() {
    let src = corpus::text(200_000, 33);
    let cases: [&[(i32, i32)]; 4] = [
        &[(ZSTD_c_targetCBlockSize, 4000)],
        &[
            (ZSTD_c_targetCBlockSize, 4000),
            (ZSTD_c_targetCBlockSize, 0),
        ],
        &[(ZSTD_c_targetCBlockSize, 0), (ZSTD_c_targetCBlockSize, 1)],
        &[
            (ZSTD_c_compressionLevel, 7),
            (ZSTD_c_targetCBlockSize, -9),
            (ZSTD_c_checksumFlag, 1),
        ],
    ];
    for pairs in cases {
        let mut config = EncoderConfig::default();
        let mut params = Params::default();
        for &(id, value) in pairs {
            config = config.with_parameter(CParameter::try_from((id, value)).unwrap());
            params = params.with(id, value);
        }
        assert_same_frame(config, &params, &src, &format!("{pairs:?}"));
    }
}
