#![cfg(feature = "compression")]
//! Configuration surface added for OpenZL: the frame format, the reference
//! parameters by ID, their errors and the containers that refuse the
//! magicless format.

use std::num::NonZeroU32;

use mzstd::{
    CParameter, CompressionParams, Compressor, ConfigError, DictionaryAttach, EncoderConfig,
    FrameFormat, FrameFormatError, LdmParams, Level, LongDistanceMatching, Parameter, Strategy,
    Switch,
};

#[test]
fn the_new_options_default_to_the_references_defaults() {
    let config = EncoderConfig::default();
    assert_eq!(config.format(), FrameFormat::Standard);
    assert!(config.dictionary_id());
    assert_eq!(config.literal_compression(), Switch::Auto);
    assert_eq!(config.dictionary_attach(), DictionaryAttach::Default);
    assert!(!config.force_max_window());
    assert_eq!(config.row_match_finder(), Switch::Auto);
    assert_eq!(config.post_block_splitter(), Switch::Auto);
    assert_eq!(config.block_splitter_level(), 0);
    assert_eq!(config.max_block_size(), None);
    assert_eq!(config.long_distance_matching(), LongDistanceMatching::Auto);
}

#[test]
fn the_new_options_are_set_and_read_back() {
    let config = EncoderConfig::default()
        .with_format(FrameFormat::Magicless)
        .with_dictionary_id(false)
        .with_literal_compression(Switch::Disabled)
        .with_dictionary_attach(DictionaryAttach::Copy)
        .with_force_max_window(true)
        .with_row_match_finder(Switch::Enabled)
        .with_post_block_splitter(Switch::Disabled)
        .with_block_splitter_level(4)
        .with_max_block_size(Some(4096));
    assert_eq!(config.format(), FrameFormat::Magicless);
    assert!(!config.dictionary_id());
    assert_eq!(config.literal_compression(), Switch::Disabled);
    assert_eq!(config.dictionary_attach(), DictionaryAttach::Copy);
    assert!(config.force_max_window());
    assert_eq!(config.row_match_finder(), Switch::Enabled);
    assert_eq!(config.post_block_splitter(), Switch::Disabled);
    assert_eq!(config.block_splitter_level(), 4);
    assert_eq!(config.max_block_size(), Some(4096));
    assert!(Compressor::new(config).is_ok());
}

#[test]
fn an_out_of_range_splitter_level_or_block_size_is_rejected() {
    let level = Compressor::new(EncoderConfig::default().with_block_splitter_level(7));
    assert_eq!(
        level.unwrap_err().to_string(),
        "block splitter level 7 is outside 0..=6"
    );
    for size in [0, 1023, 131_073] {
        let error =
            Compressor::new(EncoderConfig::default().with_max_block_size(Some(size))).unwrap_err();
        assert!(matches!(
            error,
            ConfigError::OutOfRange {
                parameter: Parameter::MaxBlockSize,
                ..
            }
        ));
    }
    assert!(Compressor::new(EncoderConfig::default().with_max_block_size(Some(1024))).is_ok());
    assert!(Compressor::new(EncoderConfig::default().with_max_block_size(Some(131_072))).is_ok());
}

#[test]
fn the_new_errors_and_parameters_read_well() {
    assert_eq!(
        ConfigError::UnknownParameter { id: 77 }.to_string(),
        "unknown zstd parameter 77"
    );
    assert_eq!(
        ConfigError::Unsupported { id: 400, value: 2 }.to_string(),
        "zstd parameter 400 does not support the value 2"
    );
    let names = [
        (Parameter::Format, "frame format"),
        (Parameter::DictionaryAttach, "dictionary attach preference"),
        (Parameter::LiteralCompression, "literal compression mode"),
        (
            Parameter::LongDistanceMatching,
            "long distance matching switch",
        ),
        (Parameter::PostBlockSplitter, "post-sequence block splitter"),
        (Parameter::BlockSplitterLevel, "block splitter level"),
        (Parameter::RowMatchFinder, "row match finder switch"),
        (Parameter::MaxBlockSize, "maximum block size"),
        (
            Parameter::TargetCompressedBlockSize,
            "target compressed block size",
        ),
        (Parameter::Other(1005), "zstd parameter 1005"),
    ];
    for (parameter, name) in names {
        assert_eq!(parameter.to_string(), name);
    }
    let error: FrameFormatError = FrameFormat::try_from(9).unwrap_err();
    assert_eq!(error.to_string(), "9 is not a frame format (0 or 1)");
}

/// Every typed parameter applies to the option it names, and converts back to
/// the reference's pair.
/// A check on the configuration a parameter produced.
type Check = Box<dyn Fn(&EncoderConfig) -> bool>;

#[test]
fn every_typed_parameter_applies_to_its_option() {
    let base = EncoderConfig::default();
    let strategy = Strategy::BtOpt;
    let cases: Vec<(CParameter, Check)> = vec![
        (
            CParameter::Format(FrameFormat::Magicless),
            Box::new(|c| c.format() == FrameFormat::Magicless),
        ),
        (
            CParameter::CompressionLevel(Level::MAX),
            Box::new(|c| c.level() == Level::MAX),
        ),
        (
            CParameter::WindowLog(20),
            Box::new(|c| c.params().window_log() == Some(20)),
        ),
        (
            CParameter::HashLog(14),
            Box::new(|c| c.params().hash_log() == Some(14)),
        ),
        (
            CParameter::ChainLog(15),
            Box::new(|c| c.params().chain_log() == Some(15)),
        ),
        (
            CParameter::SearchLog(4),
            Box::new(|c| c.params().search_log() == Some(4)),
        ),
        (
            CParameter::MinMatch(5),
            Box::new(|c| c.params().min_match() == Some(5)),
        ),
        (
            CParameter::TargetLength(64),
            Box::new(|c| c.params().target_length() == Some(64)),
        ),
        (
            CParameter::Strategy(Some(strategy)),
            Box::new(move |c| c.params().strategy() == Some(strategy)),
        ),
        (
            CParameter::EnableLongDistanceMatching(Switch::Disabled),
            Box::new(|c| c.long_distance_matching() == LongDistanceMatching::Disabled),
        ),
        (
            CParameter::LdmHashLog(18),
            Box::new(|c| {
                matches!(
                    c.with_parameter(CParameter::EnableLongDistanceMatching(Switch::Enabled))
                        .long_distance_matching(),
                    LongDistanceMatching::Enabled(ldm) if ldm.hash_log() == Some(18)
                )
            }),
        ),
        (
            CParameter::ContentSizeFlag(false),
            Box::new(|c| !c.content_size()),
        ),
        (CParameter::ChecksumFlag(true), Box::new(|c| c.checksum())),
        (
            CParameter::DictIdFlag(false),
            Box::new(|c| !c.dictionary_id()),
        ),
        (
            CParameter::ForceMaxWindow(true),
            Box::new(|c| c.force_max_window()),
        ),
        (
            CParameter::ForceAttachDict(DictionaryAttach::Load),
            Box::new(|c| c.dictionary_attach() == DictionaryAttach::Load),
        ),
        (
            CParameter::LiteralCompressionMode(Switch::Disabled),
            Box::new(|c| c.literal_compression() == Switch::Disabled),
        ),
        (
            CParameter::SrcSizeHint(Some(9)),
            Box::new(|c| c.size_hint() == Some(9)),
        ),
        (
            CParameter::SplitAfterSequences(Switch::Enabled),
            Box::new(|c| c.post_block_splitter() == Switch::Enabled),
        ),
        (
            CParameter::UseRowMatchFinder(Switch::Disabled),
            Box::new(|c| c.row_match_finder() == Switch::Disabled),
        ),
        (
            CParameter::MaxBlockSize(2048),
            Box::new(|c| c.max_block_size() == Some(2048)),
        ),
        (
            CParameter::BlockSplitterLevel(3),
            Box::new(|c| c.block_splitter_level() == 3),
        ),
        (
            CParameter::TargetCBlockSize(NonZeroU32::new(1340)),
            Box::new(|c| c.target_compressed_block_size() == NonZeroU32::new(1340)),
        ),
        (
            CParameter::TargetCBlockSize(None),
            Box::new(|c| c.target_compressed_block_size().is_none()),
        ),
        (
            CParameter::Ignored { id: 1006, value: 1 },
            Box::new(|c| *c == EncoderConfig::default()),
        ),
    ];
    for (parameter, applied) in cases {
        let config = base.with_parameter(parameter);
        assert!(applied(&config), "{parameter:?}");
        let pair = <(i32, i32)>::from(parameter);
        assert_eq!(CParameter::try_from(pair), Ok(parameter), "{parameter:?}");
    }
}

#[test]
fn zero_clears_the_options_that_treat_it_as_the_default() {
    let config = EncoderConfig::default()
        .with_parameter(CParameter::WindowLog(20))
        .with_parameter(CParameter::Strategy(Some(Strategy::Lazy)))
        .with_parameter(CParameter::SrcSizeHint(Some(5)))
        .with_parameter(CParameter::MaxBlockSize(4096));
    let cleared = config
        .with_parameter(CParameter::try_from((101, 0)).expect("valid"))
        .with_parameter(CParameter::try_from((107, 0)).expect("valid"))
        .with_parameter(CParameter::try_from((1004, 0)).expect("valid"))
        .with_parameter(CParameter::try_from((1015, 0)).expect("valid"));
    assert_eq!(cleared, EncoderConfig::default());
}

#[test]
fn long_distance_values_survive_switching_the_matcher() {
    let config = [(161, 18), (162, 100), (163, 5), (164, 6), (160, 1)]
        .into_iter()
        .map(|pair| CParameter::try_from(pair).expect("valid"))
        .fold(EncoderConfig::default(), EncoderConfig::with_parameter);
    let expected = LdmParams::default()
        .with_hash_log(18)
        .with_min_match(100)
        .with_bucket_size_log(5)
        .with_hash_rate_log(6);
    assert_eq!(
        config.long_distance_matching(),
        LongDistanceMatching::Enabled(expected)
    );
    // Switching to automatic keeps the values for when it turns itself on.
    let automatic = config.with_parameter(CParameter::EnableLongDistanceMatching(Switch::Auto));
    assert_eq!(
        automatic.long_distance_matching(),
        LongDistanceMatching::Auto
    );
    let back = automatic.with_parameter(CParameter::EnableLongDistanceMatching(Switch::Enabled));
    assert_eq!(back, config);
    // The explicit setter starts over, as it always has.
    let reset = config.with_long_distance_matching(LongDistanceMatching::Auto);
    assert_eq!(reset, EncoderConfig::default());
}

#[test]
fn level_parameters_describe_the_dictionary_they_prepare() {
    use mzstd::dictionary::DictionaryBuilder;
    let bytes = b"some shared history for the dictionary";
    let level = Level::try_from(11).expect("a level");
    let dictionary = DictionaryBuilder::new(bytes)
        .with_level_parameters(level)
        .build()
        .expect("prepares");
    let expected =
        EncoderConfig::default().with_params(CompressionParams::for_level(level, 0, bytes.len()));
    assert_eq!(dictionary.config(), &expected);
}

#[test]
fn the_parallel_and_framed_compressors_refuse_the_magicless_format() {
    let magicless = EncoderConfig::default().with_format(FrameFormat::Magicless);
    #[cfg(not(feature = "no_std"))]
    {
        let refused = ConfigError::Unsupported { id: 10, value: 1 };
        use mzstd::compressor::parallel::{ParallelCompressor, ParallelConfig};
        let result = ParallelCompressor::new(magicless, ParallelConfig::default());
        assert_eq!(result.err(), Some(refused));
    }
    #[cfg(feature = "framed")]
    {
        use mzstd::framing::FramedEncodeConfig;
        let config = FramedEncodeConfig::default().with_encoder_config(magicless);
        let error = mzstd::FramedCompressor::new(config).unwrap_err();
        assert!(error.to_string().contains("zstd parameter 10"), "{error}");
        let mut encoder =
            mzstd::FramedCompressor::new(FramedEncodeConfig::default()).expect("valid");
        assert!(encoder.reconfigure(config).is_err());
    }
}
