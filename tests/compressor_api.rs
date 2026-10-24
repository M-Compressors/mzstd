#![cfg(feature = "compression")]
//! The compressor's public contract: configuration values and their
//! validation, retention, reconfiguration, caller-owned destinations and
//! borrowed and owned sessions.

use std::mem::MaybeUninit;

use mzstd::{
    Backend, CompressionParams, Compressor, ConfigError, EncodeError, EncoderConfig, EncoderStatus,
    InputSize, LdmParams, Level, LongDistanceMatching, Operation, Parameter, Progress,
    RetentionPolicy, Strategy, StreamConfig,
};
use zstd_ffi::corpus;
use zstd_ffi::oracle::{self, Directive, Params};
use zstd_ffi::*;

/// Explicit match finder parameters, each different from level 6's.
fn explicit_params() -> CompressionParams {
    CompressionParams::default()
        .with_window_log(20)
        .with_hash_log(17)
        .with_chain_log(16)
        .with_search_log(3)
        .with_min_match(5)
        .with_target_length(32)
        .with_strategy(Strategy::Lazy2)
}

/// The reference's parameters for [`explicit_params`] at level 6.
fn explicit_oracle_params() -> Params {
    Params::level(6)
        .with(ZSTD_c_windowLog, 20)
        .with(ZSTD_c_hashLog, 17)
        .with(ZSTD_c_chainLog, 16)
        .with(ZSTD_c_searchLog, 3)
        .with(ZSTD_c_minMatch, 5)
        .with(ZSTD_c_targetLength, 32)
        .with(ZSTD_c_strategy, 5)
}

fn level(value: i32) -> Level {
    Level::try_from(value).expect("valid level")
}

/// The `OutOfRange` parameter a configuration is rejected for.
fn rejected_parameter(config: EncoderConfig) -> (Parameter, String) {
    match Compressor::new(config) {
        Err(error @ ConfigError::OutOfRange { parameter, .. }) => (parameter, error.to_string()),
        Err(other) => panic!("{config:?} was rejected for {other:?}"),
        Ok(_) => panic!("{config:?} was accepted"),
    }
}

/// Drives `step` the way `oracle::compress_stream_with_output` drives the
/// reference: every chunk is handed over whole and the call repeated until
/// the chunk is consumed or, for `Flush` and `End`, the operation completed.
/// A final `End` is appended when the last step is not one.
fn drive(
    capacity: usize,
    steps: &[(&[u8], Operation)],
    mut step: impl FnMut(&[u8], &mut [u8], Operation) -> Progress,
) -> Vec<u8> {
    let mut compressed = Vec::new();
    let mut chunk = vec![0u8; capacity];
    let mut all = steps.to_vec();
    if all.last().is_none_or(|&(_, op)| op != Operation::End) {
        all.push((&[], Operation::End));
    }
    for (mut data, operation) in all {
        loop {
            let progress = step(data, &mut chunk, operation);
            data = &data[progress.consumed..];
            compressed.extend_from_slice(&chunk[..progress.produced]);
            let done = match operation {
                Operation::Continue => data.is_empty(),
                Operation::Flush | Operation::End => progress.status != EncoderStatus::NeedsOutput,
            };
            if done {
                break;
            }
        }
    }
    compressed
}

fn directives<'a>(steps: &[(&'a [u8], Operation)]) -> Vec<(&'a [u8], Directive)> {
    steps
        .iter()
        .map(|&(data, operation)| {
            let directive = match operation {
                Operation::Continue => Directive::Continue,
                Operation::Flush => Directive::Flush,
                Operation::End => Directive::End,
            };
            (data, directive)
        })
        .collect()
}

/// The initialized prefix of an output buffer.
fn initialized(buffer: &[MaybeUninit<u8>], len: usize) -> Vec<u8> {
    buffer[..len]
        .iter()
        // SAFETY: the encoder initialized exactly the first `len` bytes,
        // which is all this reads.
        .map(|byte| unsafe { byte.assume_init() })
        .collect()
}

#[test]
fn compression_params_report_every_explicit_value() {
    let params = explicit_params();
    assert_eq!(params.window_log(), Some(20));
    assert_eq!(params.hash_log(), Some(17));
    assert_eq!(params.chain_log(), Some(16));
    assert_eq!(params.search_log(), Some(3));
    assert_eq!(params.min_match(), Some(5));
    assert_eq!(params.target_length(), Some(32));
    assert_eq!(params.strategy(), Some(Strategy::Lazy2));
    let unset = CompressionParams::default();
    assert_eq!(
        (
            unset.hash_log(),
            unset.chain_log(),
            unset.search_log(),
            unset.min_match(),
            unset.target_length()
        ),
        (None, None, None, None, None)
    );
}

#[test]
fn explicit_compression_params_match_the_reference() {
    let input = corpus::text(300_000, 11);
    let config = EncoderConfig::default()
        .with_level(level(6))
        .with_params(explicit_params());
    let ours = Compressor::new(config)
        .expect("valid")
        .compress(&input)
        .expect("compresses");
    let theirs = oracle::compress2(&input, &explicit_oracle_params()).expect("reference");
    assert_eq!(ours, theirs);
}

#[test]
fn ldm_params_report_every_explicit_value() {
    let ldm = LdmParams::default()
        .with_hash_log(20)
        .with_min_match(48)
        .with_bucket_size_log(4)
        .with_hash_rate_log(6);
    assert_eq!(ldm.hash_log(), Some(20));
    assert_eq!(ldm.min_match(), Some(48));
    assert_eq!(ldm.bucket_size_log(), Some(4));
    assert_eq!(ldm.hash_rate_log(), Some(6));
    let unset = LdmParams::default();
    assert_eq!(
        (
            unset.hash_log(),
            unset.bucket_size_log(),
            unset.hash_rate_log()
        ),
        (None, None, None)
    );
}

#[test]
fn explicit_ldm_params_match_the_reference() {
    let input = corpus::mixed(600_000, 12);
    let ldm = LdmParams::default()
        .with_hash_log(20)
        .with_min_match(48)
        .with_bucket_size_log(4)
        .with_hash_rate_log(6);
    let config = EncoderConfig::default()
        .with_level(level(9))
        .with_long_distance_matching(LongDistanceMatching::Enabled(ldm));
    let ours = Compressor::new(config)
        .expect("valid")
        .compress(&input)
        .expect("compresses");
    let params = Params::level(9)
        .with(ZSTD_c_enableLongDistanceMatching, 1)
        .with(ZSTD_c_ldmHashLog, 20)
        .with(ZSTD_c_ldmMinMatch, 48)
        .with(ZSTD_c_ldmBucketSizeLog, 4)
        .with(ZSTD_c_ldmHashRateLog, 6);
    let theirs = oracle::compress2(&input, &params).expect("reference");
    assert_eq!(ours, theirs);
}

#[test]
fn zero_restores_a_parameters_default_and_the_last_value_wins() {
    let params = CompressionParams::default()
        .with_window_log(20)
        .with_hash_log(18)
        .with_chain_log(17)
        .with_search_log(4)
        .with_min_match(5)
        .with_target_length(64);
    let cleared = params
        .with_window_log(0)
        .with_hash_log(0)
        .with_chain_log(0)
        .with_search_log(0)
        .with_min_match(0)
        .with_target_length(0);
    assert_eq!(cleared, CompressionParams::default());
    assert_eq!(
        params.with_window_log(0).with_window_log(21).window_log(),
        Some(21)
    );
    let ldm = LdmParams::default()
        .with_hash_log(20)
        .with_min_match(64)
        .with_bucket_size_log(3)
        .with_hash_rate_log(4);
    let cleared = ldm
        .with_hash_log(0)
        .with_min_match(0)
        .with_bucket_size_log(0)
        .with_hash_rate_log(0);
    assert_eq!(cleared, LdmParams::default());
    // Zeroes are not out of range, and compress as the defaults do.
    let input = corpus::text(100_000, 51);
    let zeroed = EncoderConfig::default()
        .with_params(
            CompressionParams::default()
                .with_window_log(0)
                .with_search_log(0),
        )
        .with_long_distance_matching(LongDistanceMatching::Enabled(
            LdmParams::default().with_hash_rate_log(0),
        ));
    let defaults = EncoderConfig::default()
        .with_long_distance_matching(LongDistanceMatching::Enabled(LdmParams::default()));
    let ours = Compressor::new(zeroed)
        .expect("valid")
        .compress(&input)
        .expect("compresses");
    let expected = Compressor::new(defaults)
        .expect("valid")
        .compress(&input)
        .expect("compresses");
    assert_eq!(ours, expected);
}

#[test]
fn out_of_range_match_finder_params_name_the_parameter() {
    let cases = [
        (
            CompressionParams::default().with_window_log(9),
            Parameter::WindowLog,
            "window log 9 is outside 10..=31",
        ),
        (
            CompressionParams::default().with_hash_log(31),
            Parameter::HashLog,
            "hash log 31 is outside 6..=30",
        ),
        (
            CompressionParams::default().with_chain_log(5),
            Parameter::ChainLog,
            "chain log 5 is outside 6..=30",
        ),
        (
            CompressionParams::default().with_search_log(31),
            Parameter::SearchLog,
            "search log 31 is outside 1..=30",
        ),
        (
            CompressionParams::default().with_min_match(8),
            Parameter::MinMatch,
            "minimum match 8 is outside 3..=7",
        ),
        (
            CompressionParams::default().with_target_length(131_073),
            Parameter::TargetLength,
            "target length 131073 is outside 0..=131072",
        ),
    ];
    for (params, parameter, message) in cases {
        let rejected = rejected_parameter(EncoderConfig::default().with_params(params));
        assert_eq!(rejected, (parameter, message.to_owned()));
    }
}

#[test]
fn out_of_range_ldm_params_name_the_parameter() {
    let cases = [
        (
            LdmParams::default().with_hash_log(5),
            Parameter::LdmHashLog,
            "LDM hash log 5 is outside 6..=30",
        ),
        (
            LdmParams::default().with_min_match(3),
            Parameter::LdmMinMatch,
            "LDM minimum match 3 is outside 4..=4096",
        ),
        (
            LdmParams::default().with_bucket_size_log(9),
            Parameter::LdmBucketSizeLog,
            "LDM bucket size log 9 is outside 1..=8",
        ),
        (
            LdmParams::default().with_hash_rate_log(26),
            Parameter::LdmHashRateLog,
            "LDM hash rate log 26 is outside 0..=25",
        ),
    ];
    for (ldm, parameter, message) in cases {
        let config = EncoderConfig::default()
            .with_long_distance_matching(LongDistanceMatching::Enabled(ldm));
        assert_eq!(rejected_parameter(config), (parameter, message.to_owned()));
    }
}

#[test]
fn ldm_params_are_not_validated_while_ldm_is_disabled() {
    let config =
        EncoderConfig::default().with_long_distance_matching(LongDistanceMatching::Disabled);
    assert!(Compressor::new(config).is_ok());
}

#[test]
fn an_oversized_size_hint_is_rejected() {
    let config = EncoderConfig::default().with_size_hint(Some(i32::MAX as u32 + 1));
    assert_eq!(
        rejected_parameter(config),
        (
            Parameter::SizeHint,
            "size hint 2147483648 is outside 0..=2147483647".to_owned()
        )
    );
}

#[test]
fn every_parameter_has_a_readable_name() {
    let names = [
        (Parameter::Level, "compression level"),
        (Parameter::Strategy, "strategy"),
        (Parameter::WindowLog, "window log"),
        (Parameter::HashLog, "hash log"),
        (Parameter::ChainLog, "chain log"),
        (Parameter::SearchLog, "search log"),
        (Parameter::MinMatch, "minimum match"),
        (Parameter::TargetLength, "target length"),
        (Parameter::LdmHashLog, "LDM hash log"),
        (Parameter::LdmMinMatch, "LDM minimum match"),
        (Parameter::LdmBucketSizeLog, "LDM bucket size log"),
        (Parameter::LdmHashRateLog, "LDM hash rate log"),
        (Parameter::SizeHint, "size hint"),
        (Parameter::SegmentSize, "segment size"),
        (Parameter::TaskCount, "task count"),
    ];
    for (parameter, name) in names {
        assert_eq!(parameter.to_string(), name);
    }
}

#[test]
fn levels_default_to_three() {
    assert_eq!(Level::default(), Level::DEFAULT);
    assert_eq!(i32::from(Level::default()), 3);
}

#[test]
fn levels_convert_to_and_display_as_the_reference_integer() {
    for value in [-131_072, -5, 1, 19, 22] {
        let level = level(value);
        assert_eq!(i32::from(level), value);
        assert_eq!(level.to_string(), value.to_string());
    }
    assert_eq!(i32::from(Level::MIN), -131_072);
    assert_eq!(i32::from(Level::MAX), 22);
}

#[test]
fn out_of_range_levels_are_rejected() {
    for value in [-131_073, 23] {
        let error = Level::try_from(value).expect_err("out of range");
        assert_eq!(
            error,
            ConfigError::OutOfRange {
                parameter: Parameter::Level,
                value: i64::from(value),
                min: -131_072,
                max: 22,
            }
        );
    }
    assert_eq!(
        Level::try_from(23).expect_err("too strong").to_string(),
        "compression level 23 is outside -131072..=22"
    );
}

#[test]
fn strategies_round_trip_through_their_reference_numbers() {
    let expected = [
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
    for (value, strategy) in (1u8..=9).zip(expected) {
        assert_eq!(Strategy::try_from(value), Ok(strategy));
        assert_eq!(u8::from(strategy), value);
    }
}

#[test]
fn unknown_strategy_numbers_are_rejected() {
    for value in [0u8, 10, 255] {
        assert_eq!(
            Strategy::try_from(value),
            Err(ConfigError::OutOfRange {
                parameter: Parameter::Strategy,
                value: i64::from(value),
                min: 1,
                max: 9,
            })
        );
    }
}

#[test]
fn each_strategy_matches_the_reference() {
    let input = corpus::text(120_000, 13);
    for value in 1u8..=9 {
        let strategy = Strategy::try_from(value).expect("valid");
        let config = EncoderConfig::default()
            .with_params(CompressionParams::default().with_strategy(strategy));
        let ours = Compressor::new(config)
            .expect("valid")
            .compress(&input)
            .expect("compresses");
        let params = Params::level(3).with(ZSTD_c_strategy, i32::from(value));
        let theirs = oracle::compress2(&input, &params).expect("reference");
        assert_eq!(ours, theirs, "{strategy:?}");
    }
}

#[test]
fn the_builder_records_the_retention_policy() {
    let policy = RetentionPolicy::Bounded { max_bytes: 1 << 20 };
    let encoder = Compressor::builder(EncoderConfig::default())
        .with_retention(policy)
        .build()
        .expect("valid");
    assert_eq!(encoder.retention(), policy);
    assert_eq!(
        Compressor::new(EncoderConfig::default())
            .expect("valid")
            .retention(),
        RetentionPolicy::Aggressive
    );
    assert_eq!(
        Compressor::default().retention(),
        RetentionPolicy::Aggressive
    );
}

#[test]
fn the_builder_rejects_an_invalid_configuration() {
    let config =
        EncoderConfig::default().with_params(CompressionParams::default().with_hash_log(99));
    let error = Compressor::builder(config)
        .with_retention(RetentionPolicy::ReleaseAll)
        .build()
        .expect_err("out of range");
    assert!(matches!(
        error,
        ConfigError::OutOfRange {
            parameter: Parameter::HashLog,
            value: 99,
            ..
        }
    ));
}

#[test]
fn release_all_frees_every_buffer_after_each_frame_without_changing_bytes() {
    let input = corpus::text(100_000, 14);
    let mut keeping = Compressor::default();
    let mut releasing = Compressor::builder(EncoderConfig::default())
        .with_retention(RetentionPolicy::ReleaseAll)
        .build()
        .expect("valid");
    for _ in 0..2 {
        assert_eq!(
            releasing.compress(&input).expect("compresses"),
            keeping.compress(&input).expect("compresses")
        );
        assert_eq!(releasing.retained_bytes(), 0);
        assert!(keeping.retained_bytes() > 0);
    }
}

#[test]
fn bounded_retention_keeps_buffers_only_within_the_ceiling() {
    let input = corpus::text(100_000, 15);
    let mut probe = Compressor::default();
    probe.compress(&input).expect("compresses");
    let retained = probe.retained_bytes();

    let mut roomy = Compressor::builder(EncoderConfig::default())
        .with_retention(RetentionPolicy::Bounded {
            max_bytes: retained,
        })
        .build()
        .expect("valid");
    roomy.compress(&input).expect("compresses");
    assert_eq!(roomy.retained_bytes(), retained);

    let mut cramped = Compressor::builder(EncoderConfig::default())
        .with_retention(RetentionPolicy::Bounded {
            max_bytes: retained - 1,
        })
        .build()
        .expect("valid");
    cramped.compress(&input).expect("compresses");
    assert_eq!(cramped.retained_bytes(), 0);
}

#[test]
fn reconfigure_matches_a_fresh_compressor_for_the_new_configuration() {
    let input = corpus::text(80_000, 16);
    let strong = EncoderConfig::default()
        .with_level(level(15))
        .with_checksum(true);
    let mut encoder = Compressor::default();
    encoder.compress(&input).expect("warm up");
    encoder.reconfigure(strong).expect("valid");
    assert_eq!(encoder.config(), &strong);
    let fresh = Compressor::new(strong)
        .expect("valid")
        .compress(&input)
        .expect("compresses");
    assert_eq!(encoder.compress(&input).expect("compresses"), fresh);
}

#[test]
fn reconfigure_rejects_an_invalid_configuration_and_keeps_the_old_one() {
    let input = corpus::text(10_000, 17);
    let mut encoder = Compressor::default();
    let before = encoder.compress(&input).expect("compresses");
    let invalid =
        EncoderConfig::default().with_params(CompressionParams::default().with_min_match(2));
    assert!(matches!(
        encoder.reconfigure(invalid),
        Err(ConfigError::OutOfRange {
            parameter: Parameter::MinMatch,
            ..
        })
    ));
    assert_eq!(encoder.config(), &EncoderConfig::default());
    assert_eq!(encoder.compress(&input).expect("compresses"), before);
}

#[test]
fn reconfigure_abandons_an_open_session() {
    let input = corpus::text(10_000, 18);
    let mut encoder = Compressor::default();
    {
        let mut session = encoder.start(StreamConfig::default());
        let mut out = [0u8; 16];
        session
            .process(&input, &mut out, Operation::Continue)
            .expect("accepts");
    }
    encoder
        .reconfigure(EncoderConfig::default())
        .expect("valid");
    let fresh = Compressor::default().compress(&input).expect("compresses");
    assert_eq!(encoder.compress(&input).expect("compresses"), fresh);
}

#[test]
fn compress_to_slice_reports_a_destination_that_is_too_small() {
    let input = corpus::random(4_000, 19);
    let mut encoder = Compressor::default();
    let mut cramped = [0u8; 100];
    assert_eq!(
        encoder.compress_to_slice(&input, &mut cramped),
        Err(EncodeError::OutputTooSmall { provided: 100 })
    );
    assert!(oracle::compress2_capacity(&input, &Params::level(3), 100).is_err());
}

#[test]
fn a_compressor_recovers_after_a_destination_was_too_small() {
    let input = corpus::random(4_000, 20);
    let mut encoder = Compressor::default();
    assert!(encoder.compress_to_slice(&input, &mut [0u8; 10]).is_err());
    let fresh = Compressor::default().compress(&input).expect("compresses");
    assert_eq!(encoder.compress(&input).expect("compresses"), fresh);
}

#[test]
fn compress_to_slice_succeeds_exactly_when_the_reference_does() {
    let input = corpus::text(30_000, 21);
    let frame = oracle::compress2(&input, &Params::level(3)).expect("reference");
    let mut encoder = Compressor::default();
    // The reference needs a few bytes of slack beyond the frame itself.
    for capacity in frame.len()..frame.len() + 16 {
        let theirs = oracle::compress2_capacity(&input, &Params::level(3), capacity);
        let mut destination = vec![0u8; capacity];
        match encoder.compress_to_slice(&input, &mut destination) {
            Ok(written) => assert_eq!(Ok(destination[..written].to_vec()), theirs),
            Err(error) => {
                assert_eq!(error, EncodeError::OutputTooSmall { provided: capacity });
                assert!(theirs.is_err(), "the reference fits {capacity} bytes");
            }
        }
    }
}

#[test]
fn compress_to_uninit_writes_the_one_shot_frame() {
    // Still several sequences and a compressed block under Miri, which
    // interprets the whole encoder.
    let input = corpus::text(if cfg!(miri) { 2_000 } else { 30_000 }, 22);
    let mut encoder = Compressor::default();
    let expected = encoder.compress(&input).expect("compresses");
    let bound = Compressor::max_compressed_size(input.len()).expect("fits");
    let mut destination = vec![MaybeUninit::<u8>::uninit(); bound];
    let written = encoder
        .compress_to_uninit(&input, &mut destination)
        .expect("compresses");
    assert_eq!(initialized(&destination, written), expected);
}

#[test]
fn compress_to_uninit_reports_a_destination_that_is_too_small() {
    let input = corpus::random(4_000, 23);
    let mut destination = [MaybeUninit::<u8>::uninit(); 50];
    assert_eq!(
        Compressor::default().compress_to_uninit(&input, &mut destination),
        Err(EncodeError::OutputTooSmall { provided: 50 })
    );
}

#[test]
fn compress_to_uninit_succeeds_exactly_when_the_reference_does() {
    // Several blocks each, so the destination is zeroed ahead of every one;
    // random input exercises the raw-block fallback near the capacity.
    let inputs = [corpus::text(300_000, 31), corpus::random(300_000, 32)];
    let mut encoder = Compressor::default();
    for input in &inputs {
        let frame = oracle::compress2(input, &Params::level(3)).expect("reference");
        let bound = Compressor::max_compressed_size(input.len()).expect("fits");
        for capacity in (frame.len() - 4..frame.len() + 16).chain([bound]) {
            let theirs = oracle::compress2_capacity(input, &Params::level(3), capacity);
            let mut destination = vec![MaybeUninit::<u8>::uninit(); capacity];
            match encoder.compress_to_uninit(input, &mut destination) {
                Ok(written) => assert_eq!(Ok(initialized(&destination, written)), theirs),
                Err(error) => {
                    assert_eq!(error, EncodeError::OutputTooSmall { provided: capacity });
                    assert!(theirs.is_err(), "the reference fits {capacity} bytes");
                }
            }
        }
    }
}

#[test]
fn input_sizes_convert_from_byte_counts() {
    assert_eq!(InputSize::from(0u64), InputSize::Exact(0));
    assert_eq!(InputSize::from(u64::MAX), InputSize::Exact(u64::MAX));
}

#[test]
fn stream_configs_record_the_declared_input_size() {
    let declared = StreamConfig::default().with_input_size(InputSize::Exact(42));
    assert_eq!(declared.input_size(), InputSize::Exact(42));
    assert_eq!(declared, StreamConfig::from(InputSize::Exact(42)));
    assert_eq!(StreamConfig::default().input_size(), InputSize::Unknown);
    assert_eq!(
        declared.with_input_size(InputSize::Unknown),
        StreamConfig::default()
    );
}

#[test]
fn a_session_with_a_declared_size_writes_the_one_shot_frame() {
    let input = corpus::text(40_000, 24);
    let mut encoder = Compressor::default();
    let one_shot = encoder.compress(&input).expect("compresses");
    let stream = StreamConfig::default().with_input_size(InputSize::from(input.len() as u64));
    let mut session = encoder.start(stream);
    let streamed = drive(1 << 17, &[(&input, Operation::End)], |data, out, op| {
        session.process(data, out, op).expect("compresses")
    });
    assert_eq!(streamed, one_shot);
}

#[test]
fn borrowed_session_flush_and_finish_match_the_reference() {
    let first = corpus::text(70_000, 25);
    let second = corpus::text(50_000, 26);
    let mut encoder = Compressor::default();
    let mut session = encoder.start(StreamConfig::default());
    let mut out = vec![0u8; 1 << 17];
    let mut ours = Vec::new();
    for chunk in [&first[..], &second[..]] {
        let progress = session
            .process(chunk, &mut out, Operation::Continue)
            .expect("accepts");
        assert_eq!(progress.consumed, chunk.len());
        ours.extend_from_slice(&out[..progress.produced]);
        let progress = session.flush(&mut out).expect("flushes");
        assert_eq!(progress.status, EncoderStatus::NeedsInput);
        assert_eq!(progress.consumed, 0);
        ours.extend_from_slice(&out[..progress.produced]);
    }
    let progress = session.finish(&mut out).expect("ends");
    assert_eq!(progress.status, EncoderStatus::Finished);
    ours.extend_from_slice(&out[..progress.produced]);
    assert!(session.is_finished());

    let steps = directives(&[
        (&first, Operation::Continue),
        (&[], Operation::Flush),
        (&second, Operation::Continue),
        (&[], Operation::Flush),
        (&[], Operation::End),
    ]);
    let theirs = oracle::compress_stream(&Params::level(3), &steps).expect("reference");
    assert_eq!(ours, theirs);
}

#[test]
fn borrowed_session_process_uninit_matches_the_reference_with_small_outputs() {
    let input = corpus::text(150_000, 27);
    let steps = [
        (&input[..60_000], Operation::Continue),
        (&input[60_000..], Operation::End),
    ];
    let mut encoder = Compressor::default();
    let mut session = encoder.start(StreamConfig::default());
    let mut scratch = vec![MaybeUninit::<u8>::uninit(); 1000];
    let ours = drive(1000, &steps, |data, out, op| {
        let progress = session
            .process_uninit(data, &mut scratch, op)
            .expect("compresses");
        out[..progress.produced].copy_from_slice(&initialized(&scratch, progress.produced));
        progress
    });
    assert!(session.is_finished());
    let theirs = oracle::compress_stream_with_output(&Params::level(3), &directives(&steps), 1000)
        .expect("reference");
    assert_eq!(ours, theirs);
}

#[test]
fn finished_sessions_accept_nothing_and_produce_nothing() {
    let mut encoder = Compressor::default();
    let mut session = encoder.start(StreamConfig::default());
    let mut out = [0u8; 256];
    assert!(!session.is_finished());
    session.finish(&mut out).expect("ends");
    let progress = session
        .process(b"late", &mut out, Operation::Continue)
        .expect("idempotent");
    assert_eq!(
        progress,
        Progress {
            consumed: 0,
            produced: 0,
            status: EncoderStatus::Finished
        }
    );
}

#[test]
fn a_session_fails_on_input_beyond_its_declared_size() {
    let mut encoder = Compressor::default();
    let mut session = encoder.start(InputSize::Exact(4).into());
    let mut out = [0u8; 256];
    // The reference rejects the call itself, before buffering anything.
    assert_eq!(
        session.process(b"more than four", &mut out, Operation::Continue),
        Err(EncodeError::InputSizeMismatch { declared: 4 })
    );
    assert_eq!(
        session.process(b"", &mut out, Operation::End),
        Err(EncodeError::InvalidState)
    );
    assert!(!session.is_finished());
    let params = Params::level(3).with_pledged(4);
    let steps: [(&[u8], Directive); 1] = [(b"more than four", Directive::Continue)];
    assert!(oracle::compress_stream(&params, &steps).is_err());
}

#[test]
fn a_first_end_call_longer_than_the_declared_size_fails() {
    let input = b"more than four";
    let mut encoder = Compressor::default();
    {
        let mut session = encoder.start(InputSize::Exact(4).into());
        let mut out = [0u8; 256];
        assert_eq!(
            session.process(input, &mut out, Operation::End),
            Err(EncodeError::InputSizeMismatch { declared: 4 })
        );
        assert_eq!(session.finish(&mut out), Err(EncodeError::InvalidState));
        assert!(!session.is_finished());
    }
    // The reference instead replaces the pledge with the input's length.
    let params = Params::level(3).with_pledged(4);
    assert!(oracle::compress_stream(&params, &[(input, Directive::End)]).is_ok());
    // The compressor is left as good as new.
    let fresh = Compressor::default().compress(input).expect("compresses");
    assert_eq!(encoder.compress(input).expect("compresses"), fresh);
}

#[test]
fn a_first_end_call_shorter_than_the_declared_size_fails() {
    let mut session = Compressor::default().into_session(InputSize::Exact(100).into());
    let mut out = [MaybeUninit::<u8>::uninit(); 256];
    assert_eq!(
        session.process_uninit(b"short", &mut out, Operation::End),
        Err(EncodeError::InputSizeMismatch { declared: 100 })
    );
    assert_eq!(
        session.process_uninit(b"", &mut out, Operation::End),
        Err(EncodeError::InvalidState)
    );
    session.reinit(InputSize::Exact(5).into());
    let progress = session
        .process_uninit(b"short", &mut out, Operation::End)
        .expect("matches the new declaration");
    assert_eq!(progress.status, EncoderStatus::Finished);
    let mut expected = Compressor::default();
    let mut reference = expected.start(InputSize::Exact(5).into());
    let mut frame = [0u8; 256];
    let written = reference
        .process(b"short", &mut frame, Operation::End)
        .expect("compresses")
        .produced;
    assert_eq!(initialized(&out, progress.produced), frame[..written]);
}

#[test]
fn borrowed_session_process_uninit_matches_the_reference_with_large_outputs() {
    // Outputs that hold a block's bound take the direct paths: blocks are
    // written straight into the caller's memory, and the final `End` is
    // compressed from the caller's input.
    let input = corpus::text(400_000, 33);
    let steps = [
        (&input[..250_000], Operation::Continue),
        (&[][..], Operation::Flush),
        (&input[250_000..], Operation::End),
    ];
    let capacity = 1 << 19;
    let mut encoder = Compressor::default();
    let mut session = encoder.start(StreamConfig::default());
    let mut scratch = vec![MaybeUninit::<u8>::uninit(); capacity];
    let ours = drive(capacity, &steps, |data, out, op| {
        let progress = session
            .process_uninit(data, &mut scratch, op)
            .expect("compresses");
        out[..progress.produced].copy_from_slice(&initialized(&scratch, progress.produced));
        progress
    });
    assert!(session.is_finished());
    let theirs =
        oracle::compress_stream_with_output(&Params::level(3), &directives(&steps), capacity)
            .expect("reference");
    assert_eq!(ours, theirs);
}

#[test]
fn a_single_end_call_through_process_uninit_writes_the_one_shot_frame() {
    let input = corpus::random(300_000, 34);
    let mut encoder = Compressor::default();
    let one_shot = encoder.compress(&input).expect("compresses");
    let bound = Compressor::max_compressed_size(input.len()).expect("fits");
    let mut out = vec![MaybeUninit::<u8>::uninit(); bound];
    let mut session = encoder.start(InputSize::from(input.len() as u64).into());
    let progress = session
        .process_uninit(&input, &mut out, Operation::End)
        .expect("compresses");
    assert_eq!(progress.consumed, input.len());
    assert_eq!(progress.status, EncoderStatus::Finished);
    assert_eq!(initialized(&out, progress.produced), one_shot);
}

#[test]
fn a_session_fails_when_input_falls_short_of_its_declared_size() {
    let mut encoder = Compressor::default();
    let mut session = encoder.start(InputSize::Exact(100).into());
    let mut out = [0u8; 256];
    session
        .process(b"short", &mut out, Operation::Continue)
        .expect("accepts");
    assert_eq!(
        session.finish(&mut out),
        Err(EncodeError::InputSizeMismatch { declared: 100 })
    );
}

#[test]
fn owned_sessions_match_the_reference_with_a_declared_size() {
    let input = corpus::text(200_000, 28);
    let (head, tail) = input.split_at(90_000);
    let mut session =
        Compressor::default().into_session(InputSize::from(input.len() as u64).into());
    assert!(!session.is_finished());
    let mut out = vec![0u8; 1 << 17];
    let mut ours = Vec::new();
    let progress = session
        .process(head, &mut out, Operation::Continue)
        .expect("accepts");
    assert_eq!(progress.consumed, head.len());
    ours.extend_from_slice(&out[..progress.produced]);
    let progress = session.flush(&mut out).expect("flushes");
    assert_eq!(progress.status, EncoderStatus::NeedsInput);
    ours.extend_from_slice(&out[..progress.produced]);
    let mut uninit = vec![MaybeUninit::<u8>::uninit(); 1 << 17];
    let progress = session
        .process_uninit(tail, &mut uninit, Operation::Continue)
        .expect("accepts");
    assert_eq!(progress.consumed, tail.len());
    ours.extend_from_slice(&initialized(&uninit, progress.produced));
    let progress = session.finish(&mut out).expect("ends");
    assert_eq!(progress.status, EncoderStatus::Finished);
    ours.extend_from_slice(&out[..progress.produced]);
    assert!(session.is_finished());

    let steps = directives(&[
        (head, Operation::Continue),
        (&[], Operation::Flush),
        (tail, Operation::Continue),
        (&[], Operation::End),
    ]);
    let params = Params::level(3).with_pledged(input.len() as u64);
    let theirs = oracle::compress_stream(&params, &steps).expect("reference");
    assert_eq!(ours, theirs);
}

#[test]
fn owned_sessions_reinit_to_an_identical_frame() {
    let input = corpus::text(30_000, 29);
    let mut session = Compressor::default().into_session(StreamConfig::default());
    let mut out = vec![0u8; 1 << 17];
    let first = session
        .process(&input, &mut out, Operation::End)
        .expect("compresses");
    assert_eq!(first.status, EncoderStatus::Finished);
    let first = out[..first.produced].to_vec();
    session.reinit(StreamConfig::default());
    assert!(!session.is_finished());
    let second = session
        .process(&input, &mut out, Operation::End)
        .expect("compresses");
    assert_eq!(out[..second.produced], first[..]);
}

#[test]
fn owned_sessions_report_a_declared_size_mismatch() {
    let mut session = Compressor::default().into_session(InputSize::Exact(2).into());
    let mut out = [0u8; 64];
    assert_eq!(
        session.process(b"three", &mut out, Operation::Continue),
        Err(EncodeError::InputSizeMismatch { declared: 2 })
    );
    assert_eq!(session.finish(&mut out), Err(EncodeError::InvalidState));
    session.reinit(InputSize::Exact(5).into());
    let progress = session
        .process(b"three", &mut out, Operation::End)
        .expect("matches the new declaration");
    assert_eq!(progress.status, EncoderStatus::Finished);
}

#[test]
fn into_compressor_abandons_the_frame_and_keeps_the_compressor_usable() {
    let input = corpus::text(30_000, 30);
    let config = EncoderConfig::default().with_level(level(7));
    let mut session = Compressor::new(config)
        .expect("valid")
        .into_session(StreamConfig::default());
    let mut out = [0u8; 64];
    session
        .process(&input, &mut out, Operation::Continue)
        .expect("accepts");
    let mut encoder = session.into_compressor();
    assert_eq!(encoder.config(), &config);
    let fresh = Compressor::new(config)
        .expect("valid")
        .compress(&input)
        .expect("compresses");
    assert_eq!(encoder.compress(&input).expect("compresses"), fresh);
}

#[test]
fn owned_sessions_apply_the_retention_policy_when_the_frame_ends() {
    let input = corpus::text(30_000, 31);
    let encoder = Compressor::builder(EncoderConfig::default())
        .with_retention(RetentionPolicy::ReleaseAll)
        .build()
        .expect("valid");
    let mut session = encoder.into_session(StreamConfig::default());
    let mut out = vec![0u8; 1 << 17];
    let progress = session
        .process(&input, &mut out, Operation::End)
        .expect("compresses");
    assert_eq!(progress.status, EncoderStatus::Finished);
    assert_eq!(session.into_compressor().retained_bytes(), 0);
}

#[test]
fn every_backend_writes_the_reference_frame() {
    let input = corpus::mixed(300_000, 17);
    for value in [-5, 1, 3, 5, 9, 13, 16, 19] {
        let reference =
            oracle::compress2(&input, &Params::level(value)).expect("the reference compresses");
        for backend in Backend::available() {
            let mut encoder =
                Compressor::builder(EncoderConfig::default().with_level(level(value)))
                    .with_backend(backend)
                    .build()
                    .expect("a valid configuration");
            assert_eq!(encoder.backend(), backend);
            assert_eq!(
                encoder.compress(&input).expect("compresses"),
                reference,
                "{backend} level {value}"
            );
            // Trimming keeps the selected backend.
            encoder.trim(RetentionPolicy::ReleaseAll);
            assert_eq!(encoder.backend(), backend);
            assert_eq!(encoder.compress(&input).expect("compresses"), reference);
        }
    }
    assert_eq!(Compressor::default().backend(), Backend::default());
}
