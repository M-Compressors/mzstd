//! Framed containers: a bounded parser of arbitrary bytes, native encoder
//! schedules and structured round trips, checked against zstd's own
//! decoders.

use std::io::{Cursor, Read, Write};

use mzstd::dictionary::DictionaryBuilder;
use mzstd::framing::*;
use mzstd::{Backend, DecodeOperation, EncoderConfig, InputSize, Level, Operation, StreamConfig};
use zstd_ffi::oracle::{self, Dictionary};

/// The prefix every dictionary-compressed resource of these targets uses.
const PREFIX: &[u8] = b"dictionary payload words";
/// Its external identity.
const PREFIX_ID: DictionaryId = DictionaryId([7; 32]);
/// The longest payload [`framed_roundtrip`] writes.
const ROUNDTRIP_PAYLOAD: usize = 4096;

/// Resolves [`PREFIX_ID`] to [`PREFIX`].
struct Prefix;

impl DictionaryResolver for Prefix {
    fn resolve(&self, request: ExternalDictionaryRequest) -> Option<&[u8]> {
        (request.id == PREFIX_ID && request.kind == ExternalDictionaryKind::Prefix)
            .then_some(PREFIX)
    }
}

/// Auto-mode decoding with ceilings small enough for fuzzing.
fn config() -> FramedDecodeConfig {
    FramedDecodeConfig::default()
        .with_input_mode(InputMode::Auto)
        .with_limits(
            FramedDecodeLimits::default()
                .with_max_output_bytes(Some(1 << 20))
                .with_max_decoded_bytes(Some(2 << 20))
                // One-byte chunks give `framed_roundtrip` a chunk per payload
                // byte on top of its metadata and seek table.
                .with_max_chunks(Some(2 * ROUNDTRIP_PAYLOAD as u64))
                .with_max_resources(Some(1024)),
        )
}

/// The concatenated payload of every visible resource.
fn visible(output: &FramedOutput) -> Vec<u8> {
    output
        .resources
        .iter()
        .filter(|resource| !resource.hidden)
        .flat_map(|resource| resource.data.iter().copied())
        .collect()
}

/// Arbitrary bytes: no panic, bounded progress, streaming equals one-shot
/// on every backend, Vec rollback, raw input agrees with the plain decoder,
/// and the seek reader agrees with sequential decoding.
pub fn framed_decode(data: &[u8]) {
    let data = &data[..data.len().min(65536)];
    let mut decoder = FramedDecompressor::new(config()).expect("configuration");
    let expected = decoder.decompress_with_dictionaries(&Prefix, data);
    if let Ok(output) = &expected {
        if matches!(output.structure, OutputStructure::Raw) {
            let raw = mzstd::Decompressor::default()
                .decompress(data)
                .expect("raw input the framed decoder accepts");
            assert_eq!(raw, output.resources[0].data);
        } else if let OutputStructure::Framed { layout, .. } = &output.structure
            && layout
                .chunks
                .iter()
                .all(|chunk| chunk.dictionaries.is_empty())
        {
            // Without dictionaries, every Zstandard decoder sees exactly the
            // visible resources, and the contrib seekable decoder maps them.
            let visible = visible(output);
            assert_eq!(
                oracle::decompress(data, visible.len()).expect("ZSTD_decompress"),
                visible
            );
            let seekable =
                oracle::seekable_decompress(data, 0, visible.len()).expect("contrib seekable");
            assert_eq!(seekable.data, visible);
        }
        let mut strict = FramedDecompressor::new(config().with_input_mode(InputMode::FramedOnly))
            .expect("configuration");
        let framed_only = strict.decompress_with_dictionaries(&Prefix, data);
        assert_eq!(
            framed_only.is_ok(),
            matches!(output.structure, OutputStructure::Framed { .. })
        );
    }
    // Opening is structural only: malformed untouched payload can remain accepted.
    // For sequentially valid objects, every indexed resource must match exactly.
    if let Ok(mut indexed) =
        decoder.framed_seek_reader_with_dictionaries(&Prefix, Cursor::new(data))
    {
        for i in (0..indexed.resources().len().min(16)).rev() {
            let index = ResourceIndex(i as u64);
            if let Ok(mut resource) = indexed.resource(index) {
                let _ = resource.read(&mut [0; 1]);
            }
            if let Ok(mut resource) = indexed.resource(index) {
                let mut payload = Vec::new();
                let decoded = resource.read_to_end(&mut payload);
                if let Ok(expected) = &expected {
                    decoded.expect("an indexed resource of a valid container");
                    assert_eq!(payload, expected.resources[i].data);
                }
            } else {
                assert!(expected.is_err());
            }
            let metadata = indexed.resource_metadata(index);
            if let Ok(expected) = &expected {
                assert_eq!(
                    metadata.expect("metadata"),
                    expected.resources[i].metadata.as_ref()
                );
            }
            let metadata = indexed.resource_footer_metadata(index);
            if let Ok(expected) = &expected {
                assert_eq!(
                    metadata.expect("footer metadata"),
                    expected.resources[i].footer_metadata.as_ref()
                );
            }
        }
    }
    let mut prefix = b"preserved prefix".to_vec();
    let appended = decoder.decompress_with_dictionaries_into(&Prefix, data, &mut prefix);
    assert_eq!(expected.is_ok(), appended.is_ok());
    if appended.is_err() {
        assert_eq!(prefix, b"preserved prefix");
    }
    for backend in Backend::available() {
        let mut decoder = FramedDecompressor::builder(config())
            .with_backend(backend)
            .build()
            .expect("configuration");
        let mut session = decoder
            .start_with_dictionaries(&Prefix, FramedDecodeStreamConfig::default())
            .expect("start");
        let mut cursor = 0;
        let mut payload = Vec::new();
        let mut done = false;
        let stride = usize::from(data.first().copied().unwrap_or(0) % 31) + 1;
        let mut end = stride.min(data.len());
        for step in 0..2_200_000 {
            let mut output = [0; 32];
            let capacity = if step % 17 == 0 { 0 } else { 1 + step % 32 };
            let operation = if end == data.len() {
                DecodeOperation::Finish
            } else {
                DecodeOperation::Process
            };
            match session.process(&data[cursor..end], &mut output[..capacity], operation) {
                Err(failure) => {
                    assert!(failure.consumed <= end - cursor);
                    assert!(failure.produced <= capacity);
                    assert!(
                        expected.is_err(),
                        "valid one-shot input failed incrementally: {failure:?}"
                    );
                    assert_eq!(failure.last_output.is_some(), failure.produced != 0);
                    done = true;
                    break;
                }
                Ok(progress) => {
                    assert!(progress.consumed <= end - cursor);
                    assert!(progress.produced <= capacity);
                    cursor += progress.consumed;
                    match progress.status {
                        FramedDecoderStatus::Event(FramedEvent::ResourceData(fragment)) => {
                            assert_eq!(fragment.bytes.len(), progress.produced);
                            payload.extend_from_slice(fragment.bytes);
                        }
                        FramedDecoderStatus::Finished => {
                            assert_eq!((progress.consumed, progress.produced), (0, 0));
                            if cursor == data.len()
                                && let Ok(expected) = &expected
                            {
                                assert!(
                                    expected
                                        .resources
                                        .iter()
                                        .flat_map(|r| r.data.iter())
                                        .copied()
                                        .eq(payload)
                                );
                            } else {
                                assert!(
                                    expected.is_err(),
                                    "a valid object finished before its end"
                                );
                            }
                            done = true;
                            break;
                        }
                        FramedDecoderStatus::NeedsInput => {
                            assert_eq!(cursor, end);
                            end = (end + stride).min(data.len());
                        }
                        _ => assert_eq!(progress.produced, 0),
                    }
                }
            }
        }
        assert!(done, "framing zero-progress loop");
    }
}

/// Fuzzed payload and settings through the std writer: the container
/// decodes back, reads back by index, and zstd's decoders see the payload.
pub fn framed_roundtrip(input: &[u8]) {
    let (settings, data) = input.split_at(input.len().min(2));
    let data = &data[..data.len().min(ROUNDTRIP_PAYLOAD)];
    let chunk_bytes = 1 + usize::from(settings.first().copied().unwrap_or(0) % 63);
    let level = [1, 3, 9, 19, -5][usize::from(settings.get(1).copied().unwrap_or(0) % 5)];
    let mut encoder = FramedCompressor::new(
        FramedEncodeConfig::default()
            .with_encoder_config(
                EncoderConfig::default()
                    .with_level(Level::try_from(level).expect("level"))
                    .with_checksum(settings.get(1).is_some_and(|b| b & 0x80 != 0)),
            )
            .with_framing_config(FramingConfig {
                chunk_bytes,
                repeat_metadata: true,
                ..Default::default()
            }),
    )
    .expect("framed configuration");
    let mut writer = encoder
        .framed_writer(Vec::new(), FramedEncodeStreamConfig::default())
        .expect("writer");
    writer
        .metadata(
            MetadataKind::Resource,
            &[MetadataField {
                code: *b"id",
                value: b"fuzz",
            }],
        )
        .expect("metadata");
    {
        let mut resource = writer
            .resource(ResourceOptions::default(), StreamConfig::default())
            .expect("resource");
        resource.write_all(data).expect("payload");
        resource.try_finish().expect("finish resource");
    }
    let bytes = writer.finish().expect("finish container");
    let mut decoder = FramedDecompressor::new(config()).expect("configuration");
    let result = decoder.decompress(&bytes).expect("decode");
    assert_eq!(result.resources.len(), 1);
    assert_eq!(result.resources[0].data, data);
    assert_eq!(
        oracle::decompress(&bytes, data.len()).expect("ZSTD_decompress"),
        data
    );
    let start = data.len() / 3;
    let seekable =
        oracle::seekable_decompress(&bytes, start as u64, data.len()).expect("contrib seekable");
    assert_eq!(seekable.data, &data[start..]);
    let mut indexed = decoder
        .framed_seek_reader(Cursor::new(&bytes))
        .expect("indexed");
    let mut payload = Vec::new();
    indexed
        .resource(ResourceIndex(0))
        .expect("resource")
        .read_to_end(&mut payload)
        .expect("read");
    assert_eq!(payload, data);
    framed_decode(&bytes);
}

/// Native resource schedules agree with Write and, without explicit Flush,
/// one-shot and Read; the container decodes back and zstd's decoders see
/// exactly the visible payload.
pub fn framed_encode(input: &[u8]) {
    let selector = input.first().copied().unwrap_or(0);
    let data = &input[input.len().min(1)..input.len().min(1025)];
    let config = FramedEncodeConfig::default()
        .with_encoder_config(EncoderConfig::default().with_level(Level::try_from(5).expect("5")))
        .with_framing_config(FramingConfig {
            chunk_bytes: 1 + usize::from(selector % 31),
            ..Default::default()
        });
    let backends = Backend::available();
    let backend = backends[usize::from(selector) % backends.len()];
    let mut owner = FramedCompressor::builder(config)
        .with_backend(backend)
        .build()
        .expect("configuration");
    let dictionary = DictionaryBuilder::new(PREFIX)
        .with_config(*config.encoder_config())
        .build()
        .expect("dictionary");
    let references = [DictionaryReference::PrefixId(PREFIX_ID)];
    let encoding = if selector & 8 != 0 {
        ResourceEncoding::Shared {
            dictionary: &dictionary,
            references: &references,
        }
    } else if selector & 4 != 0 {
        ResourceEncoding::Uncompressed
    } else {
        ResourceEncoding::Zstd
    };
    let options = ResourceOptions {
        hidden: selector & 16 != 0,
        id: (selector & 32 != 0).then_some(DictionaryId([9; 32])),
    };
    let stream: StreamConfig = InputSize::Exact(data.len() as u64).into();
    let split = data.len() / 2;
    let flush = selector & 1 != 0;
    let width = [1, 2, 7, 31][usize::from(selector >> 6)];
    let mut buffer = vec![0; width];
    let mut native = Vec::new();
    {
        let mut session = owner
            .start(InputSize::Exact(data.len() as u64).into())
            .expect("start");
        assert!(matches!(
            session.padding(0),
            Err(FramedEncodeError::OutputPending)
        ));
        assert_eq!(
            session
                .process(&mut [], FramedEncodeOperation::Process)
                .expect("empty output")
                .status,
            FramedEncoderStatus::NeedsOutput
        );
        loop {
            let p = session
                .process(&mut buffer, FramedEncodeOperation::Process)
                .expect("header");
            native.extend_from_slice(&buffer[..p.produced]);
            if p.status != FramedEncoderStatus::NeedsOutput {
                break;
            }
        }
        assert!(
            session
                .metadata(
                    MetadataKind::Global,
                    &[MetadataField {
                        code: *b"xx",
                        value: data
                    }]
                )
                .is_err()
        );
        assert!(
            session
                .resource_with_dictionary(
                    ResourceOptions::default(),
                    stream,
                    &dictionary,
                    &[DictionaryReference::PrefixResource(u64::from(selector))]
                )
                .is_err()
        );
        {
            let mut resource = match encoding {
                ResourceEncoding::Uncompressed => {
                    session.uncompressed_resource(options).expect("resource")
                }
                ResourceEncoding::Zstd => session.resource(options, stream).expect("resource"),
                ResourceEncoding::Shared {
                    dictionary,
                    references,
                } => session
                    .resource_with_dictionary(options, stream, dictionary, references)
                    .expect("resource"),
            };
            if flush {
                let mut p = 0;
                loop {
                    let progress = resource
                        .process(&data[p..split], &mut buffer, Operation::Flush)
                        .expect("flush");
                    p += progress.consumed;
                    native.extend_from_slice(&buffer[..progress.produced]);
                    if progress.status == FramedEncoderStatus::NeedsInput {
                        break;
                    }
                }
            }
            let mut p = if flush { split } else { 0 };
            loop {
                let progress = resource
                    .process(&data[p..], &mut buffer, Operation::End)
                    .expect("end");
                p += progress.consumed;
                native.extend_from_slice(&buffer[..progress.produced]);
                if progress.status == FramedEncoderStatus::Finished {
                    break;
                }
            }
        }
        loop {
            let p = session
                .process(&mut buffer, FramedEncodeOperation::Finish)
                .expect("finish");
            native.extend_from_slice(&buffer[..p.produced]);
            if p.status == FramedEncoderStatus::Finished {
                break;
            }
        }
    }
    {
        let mut writer = owner
            .framed_writer(Vec::new(), FramedEncodeStreamConfig::default())
            .expect("writer");
        {
            let mut resource = match encoding {
                ResourceEncoding::Uncompressed => {
                    writer.uncompressed_resource(options).expect("resource")
                }
                ResourceEncoding::Zstd => writer.resource(options, stream).expect("resource"),
                ResourceEncoding::Shared {
                    dictionary,
                    references,
                } => writer
                    .resource_with_dictionary(options, stream, dictionary, references)
                    .expect("resource"),
            };
            resource.write_all(&data[..split]).expect("write");
            if flush {
                resource.flush().expect("flush");
            }
            resource.write_all(&data[split..]).expect("write");
            resource.try_finish().expect("finish resource");
        }
        assert_eq!(writer.finish().expect("finish container"), native);
    }
    if !flush {
        let items = [FramedItem::Resource(FramedResource {
            data,
            options,
            stream,
            encoding,
        })];
        let input = items.as_slice().into();
        assert_eq!(owner.compress(input).expect("one-shot"), native);
        let mut actual = Vec::new();
        owner
            .framed_reader(input, FramedEncodeStreamConfig::default())
            .expect("reader")
            .read_to_end(&mut actual)
            .expect("read");
        assert_eq!(actual, native);
    }
    let decoded = FramedDecompressor::new(FramedDecodeConfig::default())
        .expect("configuration")
        .decompress_with_dictionaries(&Prefix, &native)
        .expect("decode");
    assert_eq!(decoded.resources[0].data, data);
    let shown: &[u8] = if options.hidden { &[] } else { data };
    let plain = if matches!(encoding, ResourceEncoding::Shared { .. }) && !options.hidden {
        let prefix = Dictionary {
            bytes: PREFIX.to_vec(),
            content_type: zstd_ffi::ZSTD_dct_rawContent,
        };
        oracle::decompress_with_dictionary(&native, shown.len(), &prefix)
    } else {
        oracle::decompress(&native, shown.len())
    };
    assert_eq!(plain.expect("zstd decodes the container"), shown);
    if !matches!(encoding, ResourceEncoding::Shared { .. }) {
        let seekable =
            oracle::seekable_decompress(&native, 0, shown.len()).expect("contrib seekable");
        assert_eq!(seekable.data, shown);
        assert_eq!(seekable.decompressed_size, shown.len() as u64);
    }
}
