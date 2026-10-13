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
