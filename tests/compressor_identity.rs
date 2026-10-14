#![cfg(feature = "compression")]
//! The public compression API produces the reference's bytes.

use mzstd::{
    CompressionParams, Compressor, EncoderConfig, EncoderStatus, InputSize, LdmParams, Level,
    LongDistanceMatching, Operation, Strategy,
};
use zstd_ffi::corpus;
use zstd_ffi::oracle::{self, Directive, Params};
use zstd_ffi::*;

fn oracle_params(config: &EncoderConfig) -> Params {
    let mut params = Params::level(config.level().get())
        .with(ZSTD_c_checksumFlag, i32::from(config.checksum()))
        .with(ZSTD_c_contentSizeFlag, i32::from(config.content_size()));
    let p = config.params();
    for (param, value) in [
        (ZSTD_c_windowLog, p.window_log()),
        (ZSTD_c_hashLog, p.hash_log()),
        (ZSTD_c_chainLog, p.chain_log()),
        (ZSTD_c_searchLog, p.search_log()),
        (ZSTD_c_minMatch, p.min_match()),
        (ZSTD_c_targetLength, p.target_length()),
    ] {
        if let Some(value) = value {
            params = params.with(param, value as i32);
        }
    }
    if let Some(strategy) = p.strategy() {
        params = params.with(ZSTD_c_strategy, i32::from(u8::from(strategy)));
    }
    match config.long_distance_matching() {
        LongDistanceMatching::Auto => {}
        LongDistanceMatching::Disabled => {
            params = params.with(ZSTD_c_enableLongDistanceMatching, 2)
        }
        LongDistanceMatching::Enabled(ldm) => {
            params = params.with(ZSTD_c_enableLongDistanceMatching, 1);
            for (param, value) in [
                (ZSTD_c_ldmHashLog, ldm.hash_log()),
                (ZSTD_c_ldmMinMatch, ldm.min_match()),
                (ZSTD_c_ldmBucketSizeLog, ldm.bucket_size_log()),
                (ZSTD_c_ldmHashRateLog, ldm.hash_rate_log()),
            ] {
                if let Some(value) = value {
                    params = params.with(param, value as i32);
                }
            }
        }
    }
    if let Some(hint) = config.size_hint() {
        params = params.with(ZSTD_c_srcSizeHint, hint as i32);
    }
    params
}

fn configs() -> Vec<EncoderConfig> {
    let mut configs = Vec::new();
    for level in [-50, -1, 1, 3, 4, 6, 10, 13, 16, 19, 22] {
        configs.push(EncoderConfig::default().with_level(Level::try_from(level).unwrap()));
    }
    configs.push(
        EncoderConfig::default()
            .with_checksum(true)
            .with_content_size(false)
            .with_params(
                CompressionParams::default()
                    .with_window_log(18)
                    .with_strategy(Strategy::Lazy2),
            ),
    );
    configs.push(
        EncoderConfig::default()
            .with_level(Level::try_from(12).unwrap())
            .with_long_distance_matching(LongDistanceMatching::Enabled(
                LdmParams::default().with_min_match(32),
            )),
    );
    configs.push(EncoderConfig::default().with_size_hint(Some(4000)));
    configs
}

#[test]
fn one_shot_matches_compress2() {
    let inputs = [
        Vec::new(),
        corpus::text(1, 1),
        corpus::text(5_000, 2),
        corpus::text(400_000, 3),
        corpus::random(70_000, 4),
    ];
    for config in configs() {
        let mut encoder = Compressor::new(config).unwrap();
        for input in &inputs {
            let ours = encoder.compress(input).unwrap();
            let theirs = oracle::compress2(input, &oracle_params(&config)).unwrap();
            assert_eq!(ours, theirs, "{config:?}, {} bytes", input.len());
            assert_eq!(oracle::decompress(&ours, input.len()).unwrap(), *input);
        }
    }
}

#[test]
fn vec_output_matches_slice_output_across_blocks_and_reused_capacity() {
    let mut alternating = vec![b'a'; 3 * (1 << 17) + 7];
    alternating[1 << 17..2 << 17].copy_from_slice(&corpus::random(1 << 17, 123));
    let inputs = [
        vec![b'a'; (1 << 17) - 1],
        vec![b'a'; 1 << 17],
        vec![b'a'; (1 << 17) + 1],
        alternating,
        corpus::text(1 << 20, 124),
        corpus::random((1 << 18) + 3, 125),
        Vec::new(),
    ];
    let prefix = b"preserve this prefix\0";
    for level in [-5, 1, 3, 9, 16, 19] {
        for checksum in [false, true] {
            let config = EncoderConfig::default()
                .with_level(Level::try_from(level).unwrap())
                .with_checksum(checksum)
                .with_content_size(!checksum);
            let mut encoder = Compressor::new(config).unwrap();
            let mut output = prefix.to_vec();
            for input in &inputs {
                let bound = Compressor::max_compressed_size(input.len()).unwrap();
                // Leave nonzero bytes in retained capacity, including an
                // earlier frame's unwritten scratch and a non-frame prefix.
                output.resize(prefix.len() + bound, 0xa5);
                output.truncate(prefix.len());
                let range = encoder.compress_into(input, &mut output).unwrap();
                assert_eq!(&output[..range.start], prefix);
                assert_eq!(range.end, output.len());
                let reference = oracle::compress2(input, &oracle_params(&config)).unwrap();
                assert_eq!(
                    &output[range],
                    reference,
                    "level {level}, {} bytes",
                    input.len()
                );

                let mut slice = vec![0x5a; bound];
                let written = encoder.compress_to_slice(input, &mut slice).unwrap();
                assert_eq!(&slice[..written], reference);
            }
        }
    }
}

#[test]
fn raw_literal_fallback_matches_c_at_output_capacity_boundaries() {
    // A de Bruijn sequence over eight symbols has no repeated four-byte
    // window, so the match finders emit only literals. Huffman can still
    // compress those literals: no sequences alone must not imply a raw block.
    fn de_bruijn(t: usize, p: usize, word: &mut [u8; 5], out: &mut Vec<u8>) {
        if t > 4 {
            if 4usize.is_multiple_of(p) {
                out.extend_from_slice(&word[1..=p]);
            }
        } else {
            word[t] = word[t - p];
            de_bruijn(t + 1, p, word, out);
            for symbol in word[t - p] + 1..8 {
                word[t] = symbol;
                de_bruijn(t + 1, t, word, out);
            }
        }
    }
    let mut huffman_only = Vec::new();
    de_bruijn(1, 1, &mut [0; 5], &mut huffman_only);
    let windows: std::collections::BTreeSet<_> = huffman_only.windows(4).collect();
    assert_eq!(windows.len(), huffman_only.len() - 3);
    let frame = oracle::compress2(&huffman_only, &Params::level(1)).unwrap();
    assert!(frame.len() < huffman_only.len());

    // A few matches still save less than minGain. Raw literals alone can
    // rule out a compressed block even when the sequence count is nonzero.
    let mut few_matches = corpus::random(65_536, 74);
    few_matches.copy_within(64..192, 4096);
    let inputs = [
        corpus::random(63, 71),
        corpus::random(1501, 72),
        corpus::random(65_536, 73),
        few_matches,
        huffman_only,
    ];
    for level in [-5, 1, 3, 9, 16] {
        let config = EncoderConfig::default().with_level(Level::try_from(level).unwrap());
        let params = oracle_params(&config);
        let mut encoder = Compressor::new(config).unwrap();
        for input in &inputs {
            let frame = oracle::compress2(input, &params).unwrap();
            let mut capacities = vec![
                0,
                5,
                input.len() - 1,
                input.len(),
                input.len() + 3,
                frame.len() - 1,
                frame.len(),
                frame.len() + 1,
                Compressor::max_compressed_size(input.len()).unwrap(),
            ];
            capacities.sort_unstable();
            capacities.dedup();
            for capacity in capacities {
                let theirs = oracle::compress2_capacity(input, &params, capacity);
                let mut out = vec![0xa5; capacity];
                let ours = encoder.compress_to_slice(input, &mut out);
                assert_eq!(
                    ours.is_ok(),
                    theirs.is_ok(),
                    "level {level}, {} bytes, capacity {capacity}",
                    input.len()
                );
                if let Ok(written) = ours {
                    assert_eq!(&out[..written], theirs.unwrap());
                }
            }
            // Reuse after the short-destination failures must also preserve
            // the preceding frame's repeat-offset and entropy-table rules.
            assert_eq!(encoder.compress(input).unwrap(), frame);
        }
    }
}

/// Drives a session with a fresh 128 KiB output buffer per call, as
/// `oracle::compress_stream` does.
fn stream(encoder: &mut Compressor, size: InputSize, steps: &[(&[u8], Operation)]) -> Vec<u8> {
    let mut session = encoder.start(size.into());
    let mut out = Vec::new();
    let mut chunk = vec![0u8; 1 << 17];
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

#[test]
fn sessions_match_compress_stream2() {
    let input = corpus::text(500_000, 9);
    for config in configs() {
        let mut encoder = Compressor::new(config).unwrap();
        for size in [InputSize::Unknown, InputSize::Exact(input.len() as u64)] {
            let mut steps: Vec<(&[u8], Operation)> = Vec::new();
            for (i, chunk) in input.chunks(70_001).enumerate() {
                steps.push((
                    chunk,
                    if i % 3 == 2 {
                        Operation::Flush
                    } else {
                        Operation::Continue
                    },
                ));
            }
            let theirs_steps: Vec<(&[u8], Directive)> = steps
                .iter()
                .map(|&(d, op)| {
                    (
                        d,
                        match op {
                            Operation::Continue => Directive::Continue,
                            Operation::Flush => Directive::Flush,
                            _ => Directive::End,
                        },
                    )
                })
                .collect();
            let mut params = oracle_params(&config);
            if let InputSize::Exact(n) = size {
                params = params.with_pledged(n);
            }
            let theirs = oracle::compress_stream(&params, &theirs_steps).unwrap();
            let ours = stream(&mut encoder, size, &steps);
            assert_eq!(ours, theirs, "{config:?} {size:?}");
        }
    }
}

#[test]
fn a_session_with_an_exact_size_matches_one_shot() {
    let input = corpus::text(20_000, 5);
    let mut encoder = Compressor::new(EncoderConfig::default()).unwrap();
    let one_shot = encoder.compress(&input).unwrap();
    let streamed = stream(
        &mut encoder,
        InputSize::Exact(input.len() as u64),
        &[(&input, Operation::End)],
    );
    assert_eq!(streamed, one_shot);
}

#[test]
fn default_compressor_matches_the_validated_default_configuration() {
    let input = corpus::text(50_000, 3);
    let mut validated = Compressor::new(EncoderConfig::default()).unwrap();
    let mut default = Compressor::default();
    assert_eq!(default.config(), validated.config());
    assert_eq!(
        default.compress(&input).unwrap(),
        validated.compress(&input).unwrap()
    );
}
