#![cfg(all(feature = "framed", feature = "compression"))]
//! The framed compressor: every destination writes the same canonical
//! container, the container obeys the wire format, its frames are the
//! reference's frames, and zstd's own decoders read it.

mod framed_support;

use framed_support::*;
use mzstd::dictionary::{DictionaryBuilder, DictionaryContent, PreparedDictionary};
use mzstd::framing::*;
use mzstd::{
    Backend, CompressionParams, EncodeError, EncoderConfig, InputSize, Level, Operation,
    RetentionPolicy, StreamConfig,
};
use zstd_ffi::{corpus, oracle};

fn encoder_config() -> EncoderConfig {
    EncoderConfig::default().with_level(Level::try_from(5).expect("level"))
}

fn config() -> FramedEncodeConfig {
    FramedEncodeConfig::default()
        .with_encoder_config(encoder_config())
        .with_framing_config(FramingConfig {
            chunk_bytes: 7,
            ..Default::default()
        })
}

fn chunked(chunk_bytes: usize) -> FramedEncodeConfig {
    config().with_framing_config(FramingConfig {
        chunk_bytes,
        ..Default::default()
    })
}

fn prepared(bytes: &[u8], config: &FramedEncodeConfig) -> PreparedDictionary {
    DictionaryBuilder::new(bytes)
        .with_config(*config.encoder_config())
        .build()
        .expect("dictionary")
}

/// Drains the container header so commands are accepted.
fn header(session: &mut FramedEncoderSession<'_>) {
    let p = session
        .process(&mut [0; 13], FramedEncodeOperation::Process)
        .expect("header");
    assert_eq!(p.status, FramedEncoderStatus::NeedsInput);
}

fn native(
    owner: &mut FramedCompressor,
    data: &[u8],
    width: usize,
    split: usize,
    stored: bool,
) -> Vec<u8> {
    let mut session = owner
        .start(InputSize::Exact(data.len() as u64).into())
        .expect("start");
    let mut output = vec![0; width];
    let mut wire = Vec::new();
    loop {
        let p = session
            .process(&mut output, FramedEncodeOperation::Process)
            .expect("header");
        wire.extend_from_slice(&output[..p.produced]);
        if p.status == FramedEncoderStatus::NeedsInput {
            break;
        }
    }
    {
        let mut resource = if stored {
            session
                .uncompressed_resource(ResourceOptions::default())
                .expect("resource")
        } else {
            session
                .resource(
                    ResourceOptions::default(),
                    InputSize::Exact(data.len() as u64).into(),
                )
                .expect("resource")
        };
        let mut start = 0;
        loop {
            let end = if start < split { split } else { data.len() };
            let op = if end == data.len() {
                Operation::End
            } else {
                Operation::Continue
            };
            let p = resource
                .process(&data[start..end], &mut output, op)
                .expect("payload");
            start += p.consumed;
            wire.extend_from_slice(&output[..p.produced]);
            if p.status == FramedEncoderStatus::Finished {
                break;
            }
        }
        assert!(resource.is_finished());
        assert_eq!(resource.total_in(), data.len() as u64);
        assert!(resource.total_out() > 0);
    }
    assert_eq!(session.resources_encoded(), 1);
    loop {
        let p = session
            .process(&mut output, FramedEncodeOperation::Finish)
            .expect("finish");
        wire.extend_from_slice(&output[..p.produced]);
        if p.status == FramedEncoderStatus::Finished {
            break;
        }
    }
    assert!(session.is_finished());
    assert_eq!(session.total_in(), data.len() as u64);
    assert_eq!(session.total_out(), wire.len() as u64);
    assert_eq!(
        session
            .process(&mut [], FramedEncodeOperation::Finish)
            .expect("finished")
            .produced,
        0
    );
    wire
}

#[test]
fn native_sessions_match_one_shot_bytes_across_chunk_and_output_boundaries() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    for stored in [false, true] {
        for length in [0, 1, 6, 7, 8, 14, 21, 32] {
            let data: Vec<u8> = (0..length).map(|i| b'a' + (i % 4) as u8).collect();
            let mut r = FramedResource::from(data.as_slice());
            if stored {
                r.encoding = ResourceEncoding::Uncompressed;
            }
            let items = [FramedItem::Resource(r)];
            let expected = owner.compress(items.as_slice().into()).expect("compress");
            parse(&expected);
            for width in [1, 2, 7, 31, 4096] {
                for split in 0..=length {
                    assert_eq!(native(&mut owner, &data, width, split, stored), expected);
                }
            }
        }
    }
}

#[test]
fn every_backend_writes_the_same_container() {
    let data = corpus::text(10_000, 4);
    let items = [FramedItem::Resource(FramedResource::from(data.as_slice()))];
    let config = chunked(4096);
    let expected = FramedCompressor::new(config)
        .expect("config")
        .compress(items.as_slice().into())
        .expect("compress");
    for backend in Backend::available() {
        let mut owner = FramedCompressor::builder(config)
            .with_backend(backend)
            .build()
            .expect("config");
        assert_eq!(
            owner.compress(items.as_slice().into()).expect("compress"),
            expected
        );
    }
}

#[test]
fn slice_destinations_hold_the_container_and_report_short_ones() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    let data = b"slice destination payload";
    let items = [FramedItem::Resource(FramedResource::from(&data[..]))];
    let input = FramedInput::from(items.as_slice());
    let expected = owner.compress(input).expect("compress");
    let mut dst = vec![0xa5; expected.len() + 3];
    assert_eq!(
        owner.compress_to_slice(input, &mut dst).expect("fits"),
        expected.len()
    );
    assert_eq!(&dst[..expected.len()], expected);
    assert_eq!(&dst[expected.len()..], [0xa5; 3]);
    for capacity in 0..expected.len() {
        let mut dst = vec![0xa5; capacity];
        let failure = owner.compress_to_slice(input, &mut dst).unwrap_err();
        assert!(matches!(failure.error, FramedEncodeError::OutputTooSmall));
        assert_eq!(failure.produced, capacity);
        assert_eq!(dst, expected[..capacity]);
    }
}

#[test]
fn appending_keeps_the_prefix_and_returns_the_container_range() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    let items = [FramedItem::Resource(FramedResource::from(&b"appended"[..]))];
    let input = FramedInput::from(items.as_slice());
    let expected = owner.compress(input).expect("compress");
    let mut appended = b"prefix".to_vec();
    let range = owner.compress_into(input, &mut appended).expect("append");
    assert_eq!(range, 6..6 + expected.len());
    assert_eq!(&appended[..6], b"prefix");
    assert_eq!(&appended[range], expected);
}

#[cfg(not(feature = "no_std"))]
#[test]
fn reader_and_writer_produce_the_one_shot_container() {
    use std::io::{Read, Write};
    let mut owner = FramedCompressor::new(config()).expect("config");
    for stored in [false, true] {
        let data = b"reader and writer payload".to_vec();
        let mut r = FramedResource::from(data.as_slice());
        if stored {
            r.encoding = ResourceEncoding::Uncompressed;
        }
        let items = [FramedItem::Resource(r)];
        let input = FramedInput::from(items.as_slice());
        let expected = owner.compress(input).expect("compress");
        let mut reader = owner
            .framed_reader(input, InputSize::Exact(data.len() as u64).into())
            .expect("reader");
        assert_eq!(reader.read(&mut []).expect("empty read"), 0);
        let mut actual = Vec::new();
        let mut byte = [0; 1];
        while reader.read(&mut byte).expect("read") != 0 {
            actual.push(byte[0]);
        }
        assert_eq!(actual, expected);
        drop(reader);
        let mut writer = owner
            .framed_writer(Vec::new(), FramedEncodeStreamConfig::default())
            .expect("writer");
        {
            let mut resource = if stored {
                writer
                    .uncompressed_resource(ResourceOptions::default())
                    .expect("resource")
            } else {
                writer
                    .resource(ResourceOptions::default(), r.stream)
                    .expect("resource")
            };
            resource.write_all(&data).expect("write");
            resource.try_finish().expect("finish resource");
        }
        assert_eq!(writer.finish().expect("finish"), expected);
    }
}

#[test]
fn every_body_frame_is_the_references_frame_of_its_chunk() {
    let data = corpus::text(70_000, 9);
    for chunk_bytes in [1000, 4096, 65536] {
        let config = chunked(chunk_bytes);
        for level in [1, 3, 19] {
            let config = config.with_encoder_config(
                EncoderConfig::default()
                    .with_level(Level::try_from(level).expect("level"))
                    .with_checksum(level == 3),
            );
            let items = [FramedItem::Resource(FramedResource::from(data.as_slice()))];
            let bytes = FramedCompressor::new(config)
                .expect("config")
                .compress(items.as_slice().into())
                .expect("compress");
            let container = parse(&bytes);
            let chunks: Vec<&Chunk> = container
                .chunks
                .iter()
                .filter(|c| c.body.is_some())
                .collect();
            let pieces: Vec<&[u8]> = data.chunks(chunk_bytes).collect();
            assert_eq!(chunks.len(), pieces.len());
            let params = oracle_params(config.encoder_config());
            for (chunk, piece) in chunks.iter().zip(pieces) {
                let reference = oracle::compress2(piece, &params).expect("reference");
                assert_eq!(&bytes[chunk.content()], reference.as_slice());
            }
        }
    }
}

#[test]
fn dictionary_chunks_are_the_references_dictionary_frames() {
    let dictionary_bytes = b"some dictionary words and payload ".repeat(8);
    let config = chunked(64);
    let dictionary = prepared(&dictionary_bytes, &config);
    let references = [DictionaryReference::PrefixId(DictionaryId([7; 32]))];
    let data = b"payload with some dictionary words, payload, words ".repeat(5);
    let items = [FramedItem::Resource(FramedResource {
        data: &data,
        options: ResourceOptions::default(),
        stream: StreamConfig::default(),
        encoding: ResourceEncoding::Shared {
            dictionary: &dictionary,
            references: &references,
        },
    })];
    let bytes = FramedCompressor::new(config)
        .expect("config")
        .compress(items.as_slice().into())
        .expect("compress");
    let container = parse(&bytes);
    let params = oracle_params(config.encoder_config())
        .with_dictionary(oracle::Dictionary::auto(&dictionary_bytes));
    for (chunk, piece) in container
        .chunks
        .iter()
        .filter(|c| c.body.is_some())
        .zip(data.chunks(64))
    {
        assert_eq!(chunk.codec, Some(3));
        let reference = oracle::compress2(piece, &params).expect("reference");
        assert_eq!(&bytes[chunk.content()], reference.as_slice());
    }
    assert_eq!(resources(&bytes, Some(&dictionary_bytes)), [data]);
}

#[test]
fn zstd_decompress_outputs_exactly_the_visible_resources() {
    let text = corpus::text(20_000, 1);
    let binary = corpus::binary(9_000, 2);
    let items = [
        FramedItem::Resource(FramedResource::from(text.as_slice())),
        FramedItem::Padding { bytes: 5 },
        FramedItem::Resource(FramedResource {
            data: b"hidden payload",
            options: ResourceOptions {
                hidden: true,
                ..Default::default()
            },
            stream: StreamConfig::default(),
            encoding: ResourceEncoding::Zstd,
        }),
        FramedItem::Resource(FramedResource {
            data: &binary,
            options: ResourceOptions::default(),
            stream: StreamConfig::default(),
            encoding: ResourceEncoding::Uncompressed,
        }),
        FramedItem::Resource(FramedResource {
            data: b"hidden verbatim",
            options: ResourceOptions {
                hidden: true,
                ..Default::default()
            },
            stream: StreamConfig::default(),
            encoding: ResourceEncoding::Uncompressed,
        }),
    ];
    for chunk_bytes in [7, 4096] {
        let bytes = FramedCompressor::new(chunked(chunk_bytes))
            .expect("config")
            .compress(items.as_slice().into())
            .expect("compress");
        assert_eq!(zstd_decompress(&bytes), [text.as_slice(), &binary].concat());
        assert_eq!(
            resources(&bytes, None),
            [
                text.clone(),
                b"hidden payload".to_vec(),
                binary.clone(),
                b"hidden verbatim".to_vec()
            ]
        );
    }
}

#[test]
fn the_contrib_seekable_decoder_reads_every_range_of_the_visible_payload() {
    let first = corpus::text(30_000, 5);
    let second = corpus::mixed(25_000, 6);
    let items = [
        FramedItem::Resource(FramedResource::from(first.as_slice())),
        FramedItem::Resource(FramedResource {
            data: b"hidden",
            options: ResourceOptions {
                hidden: true,
                ..Default::default()
            },
            stream: StreamConfig::default(),
            encoding: ResourceEncoding::Uncompressed,
        }),
        FramedItem::Resource(FramedResource::from(second.as_slice())),
    ];
    let bytes = FramedCompressor::new(chunked(4096))
        .expect("config")
        .compress(items.as_slice().into())
        .expect("compress");
    let visible = [first.as_slice(), &second].concat();
    let whole = oracle::seekable_decompress(&bytes, 0, visible.len()).expect("seekable");
    assert_eq!(whole.data, visible);
    assert_eq!(whole.decompressed_size, visible.len() as u64);
    assert_eq!(whole.frames.len(), parse(&bytes).seek.len());
    let mut rng = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..64 {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let offset = (rng % visible.len() as u64) as usize;
        let len = ((rng >> 32) % 9000) as usize;
        let range = oracle::seekable_decompress(&bytes, offset as u64, len).expect("range");
        let end = (offset + len).min(visible.len());
        assert_eq!(range.data, &visible[offset..end]);
    }
}

#[test]
fn many_chunks_and_exact_fills_keep_the_frame_and_seek_table_structure() {
    let mut owner = FramedCompressor::new(chunked(16)).expect("config");
    for length in [0usize, 15, 16, 17, 32, 33, 1000] {
        let data: Vec<u8> = (0..length).map(|i| (i * 31) as u8).collect();
        let items = [FramedItem::Resource(FramedResource::from(data.as_slice()))];
        let bytes = owner.compress(items.as_slice().into()).expect("compress");
        let container = parse(&bytes);
        let kinds: Vec<u8> = container
            .chunks
            .iter()
            .filter(|c| (2..=5).contains(&c.kind))
            .map(|c| c.kind)
            .collect();
        let pieces = length.div_ceil(16).max(1);
        assert_eq!(kinds.len(), pieces, "length {length}");
        if pieces == 1 {
            assert_eq!(kinds, [2]);
        } else {
            assert_eq!(kinds[0], 3);
            assert_eq!(kinds[pieces - 1], 5);
            assert!(kinds[1..pieces - 1].iter().all(|k| *k == 4));
        }
        assert_eq!(resources(&bytes, None), [data]);
    }
}

#[test]
fn command_admission_requires_drained_output() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    let mut s = owner
        .start(FramedEncodeStreamConfig::default())
        .expect("start");
    assert!(matches!(
        s.padding(1),
        Err(FramedEncodeError::OutputPending)
    ));
    assert!(matches!(
        s.metadata(MetadataKind::Global, &[]),
        Err(FramedEncodeError::OutputPending)
    ));
    assert!(matches!(
        s.resource(ResourceOptions::default(), StreamConfig::default()),
        Err(FramedEncodeError::OutputPending)
    ));
    assert_eq!(
        s.process(&mut [], FramedEncodeOperation::Process)
            .expect("zero width")
            .status,
        FramedEncoderStatus::NeedsOutput
    );
    assert_eq!(
        s.process(&mut [0; 13], FramedEncodeOperation::Process)
            .expect("header")
            .produced,
        13
    );
    assert_eq!(s.next_chunk_offset(), 13);
}

#[test]
fn a_changed_final_boundary_fails_the_resource() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    let mut s = owner
        .start(FramedEncodeStreamConfig::default())
        .expect("start");
    header(&mut s);
    let mut r = s
        .uncompressed_resource(ResourceOptions::default())
        .expect("resource");
    let p = r
        .process(b"1234567", &mut [], Operation::Continue)
        .expect("staged");
    assert_eq!(
        (p.consumed, p.produced, p.status),
        (7, 0, FramedEncoderStatus::NeedsInput)
    );
    let p = r.process(&[], &mut [], Operation::End).expect("end");
    assert_eq!(p.status, FramedEncoderStatus::NeedsOutput);
    let failure = r.process(b"x", &mut [0; 64], Operation::End).unwrap_err();
    assert_eq!((failure.consumed, failure.produced), (0, 0));
    assert!(matches!(
        failure.into_error(),
        FramedEncodeError::InvalidState
    ));
    assert!(r.process(&[], &mut [0; 64], Operation::End).is_err());
}

#[test]
fn a_too_small_container_contract_fails_before_accepting_payload() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    let mut s = owner.start(InputSize::Exact(2).into()).expect("start");
    header(&mut s);
    let mut r = s
        .uncompressed_resource(ResourceOptions {
            hidden: true,
            ..Default::default()
        })
        .expect("resource");
    let e = r
        .process(b"abc", &mut [0; 128], Operation::End)
        .unwrap_err();
    assert_eq!(e.consumed, 0);
    assert!(matches!(
        e.error,
        FramedEncodeError::InputSizeMismatch {
            scope: "container",
            expected: 2,
            actual: 3
        }
    ));
}

#[test]
fn a_too_large_container_contract_fails_at_finish() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    let mut s = owner.start(InputSize::Exact(4).into()).expect("start");
    header(&mut s);
    {
        let mut r = s
            .uncompressed_resource(ResourceOptions::default())
            .expect("resource");
        r.process(b"abc", &mut [0; 128], Operation::End)
            .expect("resource");
    }
    let e = s
        .process(&mut [0; 128], FramedEncodeOperation::Finish)
        .unwrap_err();
    assert!(matches!(
        e.error,
        FramedEncodeError::InputSizeMismatch {
            scope: "container",
            ..
        }
    ));
}

#[test]
fn a_resource_contract_is_checked_against_its_payload() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    let mut r = FramedResource::from(&b"abc"[..]);
    r.stream = InputSize::Exact(4).into();
    let items = [FramedItem::Resource(r)];
    assert!(matches!(
        owner.compress(items.as_slice().into()),
        Err(FramedEncodeError::InputSizeMismatch {
            scope: "resource",
            ..
        })
    ));
    let mut s = owner
        .start(FramedEncodeStreamConfig::default())
        .expect("start");
    header(&mut s);
    let mut r = s
        .resource(ResourceOptions::default(), InputSize::Exact(2).into())
        .expect("resource");
    assert!(matches!(
        r.process(b"abc", &mut [0; 128], Operation::Continue)
            .unwrap_err()
            .error,
        FramedEncodeError::InputSizeMismatch {
            scope: "resource",
            expected: 2,
            actual: 3
        }
    ));
}

#[test]
fn forgotten_and_dropped_guards_obey_recovery_and_retention() {
    for policy in [
        RetentionPolicy::Aggressive,
        RetentionPolicy::Bounded { max_bytes: 1 },
        RetentionPolicy::ReleaseAll,
    ] {
        let mut owner = FramedCompressor::builder(config())
            .with_retention(policy)
            .build()
            .expect("config");
        assert_eq!(owner.retention(), policy);
        assert_eq!(owner.retained_bytes(), 0);
        {
            let mut s = owner
                .start(FramedEncodeStreamConfig::default())
                .expect("start");
            header(&mut s);
            let r = s
                .resource(ResourceOptions::default(), StreamConfig::default())
                .expect("resource");
            std::mem::forget(r);
            assert!(matches!(
                s.padding(1),
                Err(FramedEncodeError::AbandonedResource)
            ));
            assert!(matches!(
                s.process(&mut [0; 128], FramedEncodeOperation::Finish)
                    .unwrap_err()
                    .error,
                FramedEncodeError::AbandonedResource
            ));
        }
        drop(
            owner
                .start(FramedEncodeStreamConfig::default())
                .expect("start"),
        );
        std::mem::forget(
            owner
                .start(FramedEncodeStreamConfig::default())
                .expect("start"),
        );
        owner.trim(RetentionPolicy::ReleaseAll);
        assert!(matches!(
            owner.start(FramedEncodeStreamConfig::default()),
            Err(FramedEncodeError::AbandonedSession)
        ));
        let bad = config().with_framing_config(FramingConfig {
            chunk_bytes: 0,
            ..Default::default()
        });
        assert!(owner.reconfigure(bad).is_err());
        assert_eq!(*owner.config(), config());
        assert!(matches!(
            owner.start(FramedEncodeStreamConfig::default()),
            Err(FramedEncodeError::AbandonedSession)
        ));
        owner.reconfigure(config()).expect("reconfigure");
        drop(
            owner
                .start(FramedEncodeStreamConfig::default())
                .expect("start"),
        );
        std::mem::forget(
            owner
                .start(FramedEncodeStreamConfig::default())
                .expect("start"),
        );
        owner.recover();
        assert_eq!(owner.retained_bytes(), 0);
        let items = [FramedItem::Resource(FramedResource::from(&b"abc"[..]))];
        let bytes = owner.compress(items.as_slice().into()).expect("compress");
        let mut fork = owner.fork_empty();
        assert_eq!(fork.config(), owner.config());
        assert_eq!(fork.retention(), policy);
        assert_eq!(fork.retained_bytes(), 0);
        assert_eq!(
            fork.compress(items.as_slice().into()).expect("compress"),
            bytes
        );
        if policy != RetentionPolicy::Aggressive {
            assert_eq!(owner.retained_bytes(), 0);
        } else {
            assert!(owner.retained_bytes() > 0);
        }
        owner.recover();
        assert_eq!(owner.retained_bytes(), 0);
    }
}

#[test]
fn reconfiguring_changes_the_level_of_later_containers() {
    let items = [FramedItem::Resource(FramedResource::from(
        &b"reconfigured payload, reconfigured payload"[..],
    ))];
    let mut owner = FramedCompressor::new(config()).expect("config");
    let fast = config().with_encoder_config(
        EncoderConfig::default().with_level(Level::try_from(1).expect("level")),
    );
    owner.reconfigure(fast).expect("reconfigure");
    assert_eq!(*owner.config(), fast);
    assert_eq!(
        owner.compress(items.as_slice().into()).expect("compress"),
        FramedCompressor::new(fast)
            .expect("config")
            .compress(items.as_slice().into())
            .expect("compress")
    );
    let invalid = config().with_encoder_config(
        EncoderConfig::default().with_params(CompressionParams::default().with_window_log(5)),
    );
    assert!(matches!(
        owner.reconfigure(invalid),
        Err(FramedEncodeError::Config(_))
    ));
    assert_eq!(*owner.config(), fast);
}

#[test]
fn reconfiguring_an_active_owner_releases_and_restarts_it() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    std::mem::forget(
        owner
            .start(FramedEncodeStreamConfig::default())
            .expect("start"),
    );
    owner.reconfigure(config()).expect("reconfigure");
    assert_eq!(owner.retained_bytes(), 0);
    assert!(owner.start(FramedEncodeStreamConfig::default()).is_ok());
}

#[test]
fn late_metadata_failure_rolls_back_append_and_preserves_slice_prefix() {
    let fields = [MetadataField {
        code: *b"xx",
        value: b"bad",
    }];
    let items = [
        FramedItem::Resource(FramedResource::from(&b"abc"[..])),
        FramedItem::Metadata {
            kind: MetadataKind::Global,
            fields: &fields,
            options: MetadataOptions::default(),
        },
    ];
    let input = FramedInput::from(items.as_slice());
    let mut owner = FramedCompressor::new(config()).expect("config");
    let mut dst = b"prefix".to_vec();
    assert!(matches!(
        owner.compress_into(input, &mut dst),
        Err(FramedEncodeError::Invalid(
            "invalid or duplicate reserved metadata field"
        ))
    ));
    assert_eq!(dst, b"prefix");
    let mut dst = [0xa5; 256];
    let e = owner.compress_to_slice(input, &mut dst).unwrap_err();
    assert_eq!(e.consumed, 3);
    assert_eq!(e.location.item_index, Some(1));
    assert_eq!(e.location.wire_offset, e.produced as u64);
    assert!(e.produced > 13);
    assert!(dst[e.produced..].iter().all(|b| *b == 0xa5));
}

#[cfg(not(feature = "no_std"))]
#[test]
fn a_reader_delivers_produced_bytes_before_a_deferred_failure() {
    use std::io::Read;
    let fields = [MetadataField {
        code: *b"xx",
        value: b"bad",
    }];
    let items = [
        FramedItem::Resource(FramedResource::from(&b"abc"[..])),
        FramedItem::Metadata {
            kind: MetadataKind::Global,
            fields: &fields,
            options: MetadataOptions::default(),
        },
    ];
    let mut owner = FramedCompressor::new(config()).expect("config");
    let mut reader = owner
        .framed_reader(items.as_slice().into(), FramedEncodeStreamConfig::default())
        .expect("reader");
    let mut dst = [0; 256];
    assert!(reader.read(&mut dst).expect("produced bytes") > 0);
    assert!(reader.read(&mut dst).is_err());
    assert!(reader.read(&mut dst).is_err());
    assert_eq!(reader.into_inner().items.len(), 2);
}

#[test]
fn explicit_flush_is_repeatable_without_duplicate_empty_chunks() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    let mut s = owner
        .start(FramedEncodeStreamConfig::default())
        .expect("start");
    header(&mut s);
    let mut r = s
        .uncompressed_resource(ResourceOptions::default())
        .expect("resource");
    let p = r.process(b"abc", &mut [], Operation::Flush).expect("flush");
    assert_eq!(p.consumed, 3);
    assert_eq!(p.status, FramedEncoderStatus::NeedsOutput);
    let p = r
        .process(&[], &mut [0; 128], Operation::Flush)
        .expect("flush");
    assert_eq!(p.status, FramedEncoderStatus::NeedsInput);
    let p = r
        .process(&[], &mut [0; 128], Operation::Flush)
        .expect("flush");
    assert_eq!(p.produced, 0);
    assert_eq!(
        r.process(&[], &mut [0; 128], Operation::End)
            .expect("end")
            .status,
        FramedEncoderStatus::Finished
    );
    assert_eq!(
        r.process(&[], &mut [0; 128], Operation::End)
            .expect("finished")
            .produced,
        0
    );
}

#[test]
fn flushed_chunks_split_a_resource_into_independent_frames() {
    let mut owner = FramedCompressor::new(chunked(1000)).expect("config");
    let mut wire = Vec::new();
    let mut output = [0; 4096];
    {
        let mut s = owner
            .start(FramedEncodeStreamConfig::default())
            .expect("start");
        let p = s
            .process(&mut output, FramedEncodeOperation::Process)
            .expect("header");
        wire.extend_from_slice(&output[..p.produced]);
        {
            let mut r = s
                .resource(ResourceOptions::default(), StreamConfig::default())
                .expect("resource");
            for piece in [&b"first piece "[..], b"second piece ", b"third"] {
                let p = r
                    .process(piece, &mut output, Operation::Flush)
                    .expect("flush");
                wire.extend_from_slice(&output[..p.produced]);
            }
            let p = r.finish(&mut output).expect("end");
            wire.extend_from_slice(&output[..p.produced]);
        }
        loop {
            let p = s.finish(&mut output).expect("finish");
            wire.extend_from_slice(&output[..p.produced]);
            if p.status == FramedEncoderStatus::Finished {
                break;
            }
        }
    }
    let container = parse(&wire);
    let kinds: Vec<u8> = container
        .chunks
        .iter()
        .filter(|c| c.body.is_some())
        .map(|c| c.kind)
        .collect();
    assert_eq!(kinds, [3, 4, 4, 5]);
    assert_eq!(
        resources(&wire, None),
        [b"first piece second piece third".to_vec()]
    );
}

fn all_chunk_types<'a>(
    payload: &'a [u8],
    global: &'a [MetadataField<'a>],
    resource: &'a [MetadataField<'a>],
    footer: &'a [MetadataField<'a>],
) -> [FramedItem<'a>; 6] {
    [
        FramedItem::Metadata {
            kind: MetadataKind::Global,
            fields: global,
            options: MetadataOptions::default(),
        },
        FramedItem::Metadata {
            kind: MetadataKind::Resource,
            fields: resource,
            options: MetadataOptions::default(),
        },
        FramedItem::Padding { bytes: 2 },
        FramedItem::Resource(FramedResource {
            data: payload,
            options: ResourceOptions::default(),
            stream: StreamConfig::default(),
            encoding: ResourceEncoding::Zstd,
        }),
        FramedItem::Metadata {
            kind: MetadataKind::Footer,
            fields: footer,
            options: MetadataOptions::default(),
        },
        FramedItem::Resource(FramedResource {
            data: b"raw",
            options: ResourceOptions::default(),
            stream: StreamConfig::default(),
            encoding: ResourceEncoding::Uncompressed,
        }),
    ]
}

#[test]
fn every_chunk_type_appears_in_order_and_every_driver_agrees() {
    let payload = b"framed resource contents ".repeat(9);
    let global = [MetadataField {
        code: *b"XX",
        value: b"global",
    }];
    let resource = [MetadataField {
        code: *b"id",
        value: b"example.txt",
    }];
    let footer = [MetadataField {
        code: *b"YY",
        value: b"footer",
    }];
    let items = all_chunk_types(&payload, &global, &resource, &footer);
    let config = config().with_framing_config(FramingConfig {
        chunk_bytes: 32,
        repeat_metadata: true,
        ..Default::default()
    });
    let mut owner = FramedCompressor::new(config).expect("config");
    let input = FramedInput::from(items.as_slice());
    let bytes = owner.compress(input).expect("compress");
    let container = parse(&bytes);
    let kinds: Vec<u8> = container.chunks.iter().map(|c| c.kind).collect();
    let mut expected = vec![7, 1, 0, 3];
    expected.extend(std::iter::repeat_n(4, payload.len().div_ceil(32) - 2));
    expected.extend([5, 6, 2, 8, 8, 9, 10]);
    assert_eq!(kinds, expected);
    assert!(container.chunks[3].body.is_some());
    assert_eq!(container.footer.map(|f| f.1), Some(bytes.len() as u64));
    for width in [1, 2, 7, 31, 4096] {
        assert_eq!(structured_native(&mut owner, input, width), bytes);
    }
    #[cfg(not(feature = "no_std"))]
    {
        use std::io::Read;
        let mut actual = Vec::new();
        owner
            .framed_reader(input, FramedEncodeStreamConfig::default())
            .expect("reader")
            .read_to_end(&mut actual)
            .expect("read");
        assert_eq!(actual, bytes);
    }
}

#[test]
fn metadata_content_is_the_field_serialization() {
    let fields = [
        MetadataField {
            code: *b"id",
            value: b"name",
        },
        MetadataField {
            code: *b"mt",
            value: &7i64.to_le_bytes(),
        },
        MetadataField {
            code: *b"AA",
            value: b"x",
        },
        MetadataField {
            code: *b"AA",
            value: b"y",
        },
    ];
    let mut serialized = Vec::new();
    for field in &fields {
        serialized.extend_from_slice(&field.code);
        serialized.push(field.value.len() as u8);
        serialized.extend_from_slice(field.value);
    }
    for encoding in [MetadataEncoding::Uncompressed, MetadataEncoding::Zstd] {
        let items = [
            FramedItem::Metadata {
                kind: MetadataKind::Resource,
                fields: &fields,
                options: MetadataOptions {
                    encoding,
                    ..Default::default()
                },
            },
            FramedItem::Resource(FramedResource::from(&b""[..])),
        ];
        let bytes = FramedCompressor::new(config())
            .expect("config")
            .compress(items.as_slice().into())
            .expect("compress");
        let container = parse(&bytes);
        let metadata = &container.chunks[0];
        assert_eq!(metadata.kind, 1);
        let codec = if matches!(encoding, MetadataEncoding::Zstd) {
            2
        } else {
            0
        };
        assert_eq!(metadata.codec, Some(codec));
        assert!(metadata.body.is_none());
        assert_eq!(content(&bytes, metadata, None), serialized);
    }
}

#[test]
fn dictionaries_and_repeat_selections_use_the_shared_driver() {
    let dictionary_bytes = b"some dictionary words and payload";
    let config = config().with_framing_config(FramingConfig {
        chunk_bytes: 7,
        repeat_metadata: true,
        ..Default::default()
    });
    let dictionary = prepared(dictionary_bytes, &config);
    let references = [DictionaryReference::PrefixId(DictionaryId([7; 32]))];
    let fields = [
        MetadataField {
            code: *b"id",
            value: b"file",
        },
        MetadataField {
            code: *b"AB",
            value: b"some dictionary words and payload",
        },
    ];
    let mut owner = FramedCompressor::new(config).expect("config");
    for encoding in [
        MetadataEncoding::Uncompressed,
        MetadataEncoding::Zstd,
        MetadataEncoding::Shared {
            dictionary: &dictionary,
            references: &references,
        },
    ] {
        let items = [
            FramedItem::Metadata {
                kind: MetadataKind::Resource,
                fields: &fields,
                options: MetadataOptions {
                    encoding,
                    repeated_encoding: encoding,
                },
            },
            FramedItem::Resource(FramedResource {
                data: b"some dictionary words and payload",
                options: ResourceOptions {
                    hidden: true,
                    id: Some(DictionaryId([9; 32])),
                },
                stream: StreamConfig::default(),
                encoding: ResourceEncoding::Shared {
                    dictionary: &dictionary,
                    references: &references,
                },
            }),
            FramedItem::Metadata {
                kind: MetadataKind::Footer,
                fields: &fields[1..],
                options: MetadataOptions {
                    encoding,
                    repeated_encoding: encoding,
                },
            },
        ];
        for codes in [None, Some(&[][..]), Some(&[*b"id"][..])] {
            let input = FramedInput {
                items: &items,
                repeat_metadata_fields: codes,
            };
            let bytes = owner.compress(input).expect("compress");
            let container = parse(&bytes);
            assert_eq!(
                container.chunks.iter().filter(|c| c.kind == 8).count(),
                2,
                "both metadata chunks repeat"
            );
            let last = container
                .chunks
                .iter()
                .rfind(|c| (2..=5).contains(&c.kind))
                .expect("data");
            assert_eq!(last.checksum, Some([9; 32]));
            assert_eq!(
                resources(&bytes, Some(dictionary_bytes)),
                [dictionary_bytes.to_vec()]
            );
            assert_eq!(structured_native(&mut owner, input, 1), bytes);
        }
    }
}

#[test]
fn repeated_metadata_keeps_only_the_selected_fields() {
    let fields = [
        MetadataField {
            code: *b"id",
            value: b"file",
        },
        MetadataField {
            code: *b"AB",
            value: b"kept out",
        },
    ];
    let items = [
        FramedItem::Metadata {
            kind: MetadataKind::Resource,
            fields: &fields,
            options: MetadataOptions::default(),
        },
        FramedItem::Resource(FramedResource::from(&b""[..])),
    ];
    let config = config().with_framing_config(FramingConfig {
        repeat_metadata: true,
        ..Default::default()
    });
    let mut owner = FramedCompressor::new(config).expect("config");
    for (codes, expected) in [
        (None, &b"id\x04fileAB\x08kept out"[..]),
        (Some(&[*b"id"][..]), &b"id\x04file"[..]),
        (Some(&[][..]), &b""[..]),
    ] {
        let bytes = owner
            .compress(FramedInput {
                items: &items,
                repeat_metadata_fields: codes,
            })
            .expect("compress");
        let container = parse(&bytes);
        let repeat = container
            .chunks
            .iter()
            .find(|c| c.kind == 8)
            .expect("a repeat");
        assert_eq!(repeat.repeated_kind, Some(1));
        assert_eq!(content(&bytes, repeat, None), expected);
    }
}

#[test]
fn repeat_selection_is_validated_before_any_metadata() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    let mut s = owner
        .start(FramedEncodeStreamConfig::default())
        .expect("start");
    header(&mut s);
    assert!(matches!(
        s.repeat_metadata_fields(&[]),
        Err(FramedEncodeError::Invalid(
            "select repeated fields before any metadata with repetition enabled"
        ))
    ));
    drop(s);
    let config = config().with_framing_config(FramingConfig {
        repeat_metadata: true,
        ..Default::default()
    });
    let mut owner = FramedCompressor::new(config).expect("config");
    let mut s = owner
        .start(FramedEncodeStreamConfig::default())
        .expect("start");
    header(&mut s);
    assert!(matches!(
        s.repeat_metadata_fields(&[*b"ab"]),
        Err(FramedEncodeError::Invalid(
            "invalid or duplicate repeated field code"
        ))
    ));
    assert!(matches!(
        s.repeat_metadata_fields(&[*b"AB", *b"AB"]),
        Err(FramedEncodeError::Invalid(
            "invalid or duplicate repeated field code"
        ))
    ));
    let codes: Vec<[u8; 2]> = (0..679)
        .map(|i| [b'A' + (i / 26 % 26) as u8, b'A'])
        .collect();
    assert!(matches!(
        s.repeat_metadata_fields(&codes),
        Err(FramedEncodeError::Limit {
            kind: "repeated field codes",
            limit: 678
        })
    ));
    s.repeat_metadata_fields(&[*b"AB"])
        .expect("first selection");
    s.repeat_metadata_fields(&[*b"id", *b"mt"])
        .expect("replaced selection");
    s.metadata(MetadataKind::Global, &[]).expect("metadata");
    header_drain(&mut s);
    assert!(s.repeat_metadata_fields(&[*b"AB"]).is_err());
}

fn header_drain(s: &mut FramedEncoderSession<'_>) {
    let mut output = [0; 512];
    while s
        .process(&mut output, FramedEncodeOperation::Process)
        .expect("drain")
        .status
        == FramedEncoderStatus::NeedsOutput
    {}
}

#[test]
fn metadata_fields_and_adjacency_are_validated_before_commit() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    let mut s = owner
        .start(FramedEncodeStreamConfig::default())
        .expect("start");
    header(&mut s);
    for (kind, fields) in [
        (MetadataKind::Global, vec![(*b"id", &b"x"[..])]),
        (MetadataKind::Resource, vec![(*b"id", &b"\xff"[..])]),
        (MetadataKind::Resource, vec![(*b"mt", &b"short"[..])]),
        (
            MetadataKind::Resource,
            vec![(*b"id", &b"a"[..]), (*b"id", &b"b"[..])],
        ),
        (MetadataKind::Resource, vec![(*b"Ab", &b""[..])]),
    ] {
        let fields: Vec<MetadataField<'_>> = fields
            .into_iter()
            .map(|(code, value)| MetadataField { code, value })
            .collect();
        assert!(matches!(
            s.metadata(kind, &fields),
            Err(FramedEncodeError::Invalid(
                "invalid or duplicate reserved metadata field"
            ))
        ));
    }
    assert!(matches!(
        s.metadata(MetadataKind::Footer, &[]),
        Err(FramedEncodeError::Invalid(
            "metadata is not adjacent to its resource"
        ))
    ));
    s.metadata(MetadataKind::Resource, &[])
        .expect("resource metadata");
    header_drain(&mut s);
    assert!(matches!(
        s.metadata(MetadataKind::Resource, &[]),
        Err(FramedEncodeError::Invalid(
            "metadata is not adjacent to its resource"
        ))
    ));
    let e = s
        .process(&mut [0; 64], FramedEncodeOperation::Finish)
        .unwrap_err();
    assert!(matches!(
        e.error,
        FramedEncodeError::Invalid("resource is missing, unfinished, or abandoned")
    ));
}

#[test]
fn the_metadata_budget_is_enforced() {
    let config = config().with_framing_config(FramingConfig {
        max_metadata_bytes: 10,
        ..Default::default()
    });
    let mut owner = FramedCompressor::new(config).expect("config");
    let mut s = owner
        .start(FramedEncodeStreamConfig::default())
        .expect("start");
    header(&mut s);
    let fields = [MetadataField {
        code: *b"AA",
        value: b"0123456789",
    }];
    assert!(matches!(
        s.metadata(MetadataKind::Global, &fields),
        Err(FramedEncodeError::Limit {
            kind: "metadata bytes",
            limit: 10
        })
    ));
}

#[test]
fn dictionary_borrow_ends_with_resource_and_forgotten_guard_retains_no_borrow() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    let mut s = owner
        .start(FramedEncodeStreamConfig::default())
        .expect("start");
    header(&mut s);
    {
        let dictionary = prepared(b"dictionary", &config());
        let mut r = s
            .resource_with_dictionary(
                ResourceOptions::default(),
                StreamConfig::default(),
                &dictionary,
                &[DictionaryReference::PrefixId(DictionaryId([1; 32]))],
            )
            .expect("resource");
        r.process(b"abc", &mut [0; 512], Operation::End)
            .expect("resource");
    }
    s.metadata(MetadataKind::Global, &[]).expect("metadata");
    let mut wire = Vec::new();
    let mut output = [0; 512];
    loop {
        let p = s
            .process(&mut output, FramedEncodeOperation::Finish)
            .expect("finish");
        wire.extend_from_slice(&output[..p.produced]);
        if p.status == FramedEncoderStatus::Finished {
            break;
        }
    }
    drop(s);
    {
        let mut s = owner
            .start(FramedEncodeStreamConfig::default())
            .expect("start");
        header(&mut s);
        {
            let dictionary = prepared(b"dictionary", &config());
            let mut r = s
                .resource_with_dictionary(
                    ResourceOptions::default(),
                    StreamConfig::default(),
                    &dictionary,
                    &[DictionaryReference::PrefixId(DictionaryId([1; 32]))],
                )
                .expect("resource");
            r.process(b"dictionary dictionary", &mut [0; 512], Operation::Continue)
                .expect("resource");
            std::mem::forget(r);
        }
        std::mem::forget(s);
    }
    owner.recover();
    assert_eq!(owner.retained_bytes(), 0);
    assert!(
        owner
            .compress(
                [FramedItem::Resource(FramedResource::from(&b"next"[..]))]
                    .as_slice()
                    .into()
            )
            .is_ok()
    );
}

#[test]
fn every_reference_form_is_written_container_relative() {
    let config = chunked(32);
    let dictionary = prepared(b"dictionary", &config);
    let refs = [
        [DictionaryReference::PrefixId(DictionaryId([1; 32]))],
        [DictionaryReference::SerializedId(DictionaryId([2; 32]))],
        [DictionaryReference::PrefixResource(13)],
        [DictionaryReference::SerializedResource(13)],
        [DictionaryReference::PrefixChunk(13)],
    ];
    let mut items = vec![FramedItem::Resource(FramedResource {
        data: b"dictionary",
        options: ResourceOptions {
            hidden: true,
            ..Default::default()
        },
        stream: StreamConfig::default(),
        encoding: ResourceEncoding::Uncompressed,
    })];
    for references in &refs {
        items.push(FramedItem::Resource(FramedResource {
            data: b"dictionary dictionary",
            options: ResourceOptions::default(),
            stream: StreamConfig::default(),
            encoding: ResourceEncoding::Shared {
                dictionary: &dictionary,
                references,
            },
        }));
    }
    let mut owner = FramedCompressor::new(config).expect("config");
    let input = FramedInput::from(items.as_slice());
    let expected = owner.compress(input).expect("compress");
    let container = parse(&expected);
    let written: Vec<(u8, Vec<u8>)> = container
        .chunks
        .iter()
        .filter(|c| c.codec == Some(3))
        .map(|c| c.references[0].clone())
        .collect();
    let mut id1 = vec![3];
    id1.extend([1; 32]);
    let mut id2 = vec![3];
    id2.extend([2; 32]);
    assert_eq!(
        written,
        [
            (2, id1),
            (6, id2),
            (0, vec![13]),
            (4, vec![13]),
            (1, vec![13])
        ]
    );
    assert_eq!(structured_native(&mut owner, input, 1), expected);
    let mut appended = b"caller prefix".to_vec();
    let range = owner.compress_into(input, &mut appended).expect("append");
    assert_eq!(&appended[range], expected);
}

#[test]
fn references_must_name_one_serialized_dictionary_or_prefixes_of_earlier_chunks() {
    let config = config();
    let dictionary = prepared(b"dictionary", &config);
    let mut owner = FramedCompressor::new(config).expect("config");
    let mut s = owner
        .start(FramedEncodeStreamConfig::default())
        .expect("start");
    header(&mut s);
    let id = DictionaryId([1; 32]);
    let seventeen = [DictionaryReference::PrefixId(id); 17];
    for references in [
        &[][..],
        &seventeen[..],
        &[
            DictionaryReference::SerializedId(id),
            DictionaryReference::PrefixId(id),
        ][..],
    ] {
        assert!(matches!(
            s.resource_with_dictionary(
                ResourceOptions::default(),
                StreamConfig::default(),
                &dictionary,
                references
            ),
            Err(FramedEncodeError::Invalid(
                "a chunk takes one serialized dictionary or 1..=16 prefixes"
            ))
        ));
    }
    assert!(matches!(
        s.resource_with_dictionary(
            ResourceOptions::default(),
            StreamConfig::default(),
            &dictionary,
            &[DictionaryReference::PrefixResource(13)]
        ),
        Err(FramedEncodeError::Invalid(
            "dictionary pointer must address an earlier content chunk or resource"
        ))
    ));
    let sixteen = [DictionaryReference::PrefixId(id); 16];
    let r = s
        .resource_with_dictionary(
            ResourceOptions::default(),
            StreamConfig::default(),
            &dictionary,
            &sixteen,
        )
        .expect("sixteen prefixes");
    drop(r);
}

#[test]
fn repeated_metadata_takes_only_external_dictionary_references() {
    let config = config().with_framing_config(FramingConfig {
        repeat_metadata: true,
        ..Default::default()
    });
    let dictionary = prepared(b"dictionary", &config);
    let items = [
        FramedItem::Resource(FramedResource::from(&b"first"[..])),
        FramedItem::Metadata {
            kind: MetadataKind::Footer,
            fields: &[],
            options: MetadataOptions {
                encoding: MetadataEncoding::Uncompressed,
                repeated_encoding: MetadataEncoding::Shared {
                    dictionary: &dictionary,
                    references: &[DictionaryReference::PrefixChunk(13)],
                },
            },
        },
    ];
    assert!(matches!(
        FramedCompressor::new(config)
            .expect("config")
            .compress(items.as_slice().into()),
        Err(FramedEncodeError::Invalid(
            "pre-encoded repeated metadata requires external dictionary references"
        ))
    ));
}

#[test]
fn a_dictionary_prepared_for_another_configuration_is_rejected_before_output() {
    let other = FramedEncodeConfig::default().with_encoder_config(
        EncoderConfig::default().with_level(Level::try_from(19).expect("level")),
    );
    let dictionary = prepared(b"dictionary", &other);
    let mut owner = FramedCompressor::new(config()).expect("config");
    let items = [FramedItem::Resource(FramedResource {
        data: b"dictionary",
        options: ResourceOptions::default(),
        stream: StreamConfig::default(),
        encoding: ResourceEncoding::Shared {
            dictionary: &dictionary,
            references: &[DictionaryReference::PrefixId(DictionaryId([7; 32]))],
        },
    })];
    let failure = owner
        .compress_to_slice(items.as_slice().into(), &mut [0; 512])
        .unwrap_err();
    assert_eq!(failure.location.resource_index, Some(0));
    assert_eq!(failure.location.resource_input_offset, Some(0));
    assert!(matches!(
        failure.error,
        FramedEncodeError::Encode(EncodeError::DictionaryMismatch)
    ));
    let source = std::error::Error::source(&failure).expect("the framing error");
    assert!(source.source().is_some(), "the codec error is chained");
    assert!(owner.compress([].as_slice().into()).is_ok());
}

#[test]
fn a_serialized_dictionary_compresses_its_resources() {
    let records = corpus::records(400, 3);
    let samples: Vec<&[u8]> = records[..300].iter().map(Vec::as_slice).collect();
    let trained = oracle::train_dictionary(&samples, 4096).expect("trains");
    let config = chunked(4096);
    let dictionary = DictionaryBuilder::new(&trained)
        .with_content(DictionaryContent::Zstd)
        .with_config(*config.encoder_config())
        .build()
        .expect("dictionary");
    let data: Vec<u8> = records[300..].concat();
    let items = [FramedItem::Resource(FramedResource {
        data: &data,
        options: ResourceOptions::default(),
        stream: StreamConfig::default(),
        encoding: ResourceEncoding::Shared {
            dictionary: &dictionary,
            references: &[DictionaryReference::SerializedId(DictionaryId([3; 32]))],
        },
    })];
    let bytes = FramedCompressor::new(config)
        .expect("config")
        .compress(items.as_slice().into())
        .expect("compress");
    assert_eq!(resources(&bytes, Some(&trained)), [data]);
}

#[test]
fn the_single_resource_profile_holds_exactly_one_resource() {
    let config = config().with_framing_config(FramingConfig {
        container: false,
        central_directory: false,
        ..Default::default()
    });
    let mut owner = FramedCompressor::new(config).expect("config");
    assert!(matches!(
        owner.compress([].as_slice().into()),
        Err(FramedEncodeError::Invalid(
            "single-resource profile requires exactly one resource"
        ))
    ));
    let r = FramedItem::Resource(FramedResource {
        encoding: ResourceEncoding::Uncompressed,
        ..FramedResource::from(&b"abc"[..])
    });
    let bytes = owner.compress([r].as_slice().into()).expect("compress");
    let container = parse(&bytes);
    assert!(!container.is_container());
    assert_eq!(container.footer, None);
    assert_eq!(&bytes[..13], b"\x5b\x2a\x4d\x18\x05\0\0\0MZFR\0");
    assert_eq!(zstd_decompress(&bytes), b"abc");
    assert!(matches!(
        owner.compress([r, r].as_slice().into()),
        Err(FramedEncodeError::Limit {
            kind: "resource count",
            ..
        })
    ));
    let mut s = owner
        .start(FramedEncodeStreamConfig::default())
        .expect("start");
    header(&mut s);
    assert!(matches!(
        s.metadata(MetadataKind::Global, &[]),
        Err(FramedEncodeError::Invalid(
            "metadata requires a container footer"
        ))
    ));
    s.padding(3).expect("padding is allowed");
}

#[test]
fn inconsistent_profiles_and_chunk_sizes_are_rejected() {
    for framing in [
        FramingConfig {
            container: false,
            ..Default::default()
        },
        FramingConfig {
            central_directory: false,
            repeat_metadata: true,
            ..Default::default()
        },
        FramingConfig {
            chunk_bytes: 0,
            ..Default::default()
        },
        FramingConfig {
            chunk_bytes: (1 << 24) + 1,
            max_buffer_bytes: usize::MAX,
            ..Default::default()
        },
        FramingConfig {
            chunk_bytes: 4096,
            max_buffer_bytes: 4 * 4096 + 8191,
            ..Default::default()
        },
    ] {
        assert!(matches!(
            FramedCompressor::new(FramedEncodeConfig::default().with_framing_config(framing)),
            Err(FramedEncodeError::Invalid(_))
        ));
    }
    let invalid = FramedEncodeConfig::default().with_encoder_config(
        EncoderConfig::default().with_params(CompressionParams::default().with_window_log(5)),
    );
    let error = FramedCompressor::new(invalid).unwrap_err();
    assert!(matches!(error, FramedEncodeError::Config(_)));
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn empty_containers_hold_only_the_suffix() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    let bytes = owner.compress([].as_slice().into()).expect("compress");
    let container = parse(&bytes);
    assert_eq!(
        container.chunks.iter().map(|c| c.kind).collect::<Vec<_>>(),
        [9, 10]
    );
    assert_eq!(container.seek, [(13, 0), (8 + 2, 0), (25, 0)]);
    assert!(
        owner
            .compress(FramedInput {
                items: &[],
                repeat_metadata_fields: Some(&[])
            })
            .is_err()
    );
    let without_directory = config().with_framing_config(FramingConfig {
        central_directory: false,
        ..Default::default()
    });
    let bytes = FramedCompressor::new(without_directory)
        .expect("config")
        .compress([].as_slice().into())
        .expect("compress");
    assert_eq!(parse(&bytes).footer, Some((0, bytes.len() as u64)));
}

#[test]
fn stream_configuration_and_slice_abandonment_report_without_progress() {
    let stream = FramedEncodeStreamConfig::default().with_input_size(InputSize::Exact(3));
    assert_eq!(stream.input_size(), InputSize::Exact(3));
    assert_eq!(FramedEncodeStreamConfig::from(InputSize::Exact(3)), stream);
    let mut owner = FramedCompressor::new(config()).expect("config");
    std::mem::forget(owner.start(stream).expect("start"));
    let failure = owner
        .compress_to_slice([].as_slice().into(), &mut [0; 128])
        .unwrap_err();
    assert!(matches!(failure.error, FramedEncodeError::AbandonedSession));
    assert_eq!((failure.consumed, failure.produced), (0, 0));
    assert!(matches!(
        owner.compress([].as_slice().into()),
        Err(FramedEncodeError::AbandonedSession)
    ));
}

#[test]
fn late_chunk_limit_reports_exact_partial_input_and_output() {
    let mut owner = FramedCompressor::new(config().with_framing_config(FramingConfig {
        chunk_bytes: 7,
        max_chunks: 1,
        ..Default::default()
    }))
    .expect("config");
    let mut s = owner
        .start(FramedEncodeStreamConfig::default())
        .expect("start");
    header(&mut s);
    let mut r = s
        .uncompressed_resource(ResourceOptions {
            hidden: true,
            ..Default::default()
        })
        .expect("resource");
    let mut output = [0xa5; 128];
    let e = r
        .process(&[b'x'; 21], &mut output, Operation::End)
        .unwrap_err();
    assert_eq!(e.consumed, 14);
    // One inline chunk: frame header, type, codec, flags and seven bytes.
    assert_eq!(e.produced, 8 + 3 + 7);
    assert_eq!(e.location.resource_input_offset, Some(14));
    assert_eq!(e.location.resource_index, Some(0));
    assert!(output[e.produced..].iter().all(|b| *b == 0xa5));
    assert!(matches!(
        e.error,
        FramedEncodeError::Limit {
            kind: "chunk count",
            limit: 1
        }
    ));
    assert!(r.process(&[], &mut output, Operation::End).is_err());
}

#[test]
fn resource_and_chunk_ceilings_are_checked_before_commands() {
    let mut owner = FramedCompressor::new(config().with_framing_config(FramingConfig {
        max_resources: 1,
        ..Default::default()
    }))
    .expect("config");
    let items = [
        FramedItem::Resource(FramedResource::from(&b"a"[..])),
        FramedItem::Resource(FramedResource::from(&b"b"[..])),
    ];
    assert!(matches!(
        owner.compress(items.as_slice().into()),
        Err(FramedEncodeError::Limit {
            kind: "resource count",
            limit: 1
        })
    ));
    let mut owner = FramedCompressor::new(config().with_framing_config(FramingConfig {
        max_chunks: 1,
        ..Default::default()
    }))
    .expect("config");
    let mut s = owner
        .start(FramedEncodeStreamConfig::default())
        .expect("start");
    header(&mut s);
    s.padding(1).expect("first chunk");
    header_drain(&mut s);
    assert!(matches!(
        s.padding(1),
        Err(FramedEncodeError::Limit {
            kind: "chunk count",
            limit: 1
        })
    ));
    assert!(matches!(
        s.metadata(MetadataKind::Global, &[]),
        Err(FramedEncodeError::Limit {
            kind: "chunk count",
            limit: 1
        })
    ));
    assert!(matches!(
        s.resource(ResourceOptions::default(), StreamConfig::default()),
        Err(FramedEncodeError::Limit {
            kind: "chunk count",
            limit: 1
        })
    ));
    let e = s
        .process(&mut [0; 64], FramedEncodeOperation::Finish)
        .unwrap_err();
    assert!(matches!(
        e.error,
        FramedEncodeError::Limit {
            kind: "chunk count",
            ..
        }
    ));
}

#[test]
fn the_framing_buffer_budget_rejects_oversized_padding() {
    let config = config().with_framing_config(FramingConfig {
        chunk_bytes: 7,
        max_buffer_bytes: 8192 + 28,
        ..Default::default()
    });
    let mut owner = FramedCompressor::new(config).expect("config");
    let mut s = owner
        .start(FramedEncodeStreamConfig::default())
        .expect("start");
    header(&mut s);
    assert!(matches!(
        s.padding(4096),
        Err(FramedEncodeError::Limit {
            kind: "retained framing bytes",
            ..
        })
    ));
}

#[test]
fn a_failed_container_refuses_further_work() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    let mut s = owner.start(InputSize::Exact(1).into()).expect("start");
    header(&mut s);
    assert!(
        s.process(&mut [0; 64], FramedEncodeOperation::Finish)
            .is_err()
    );
    assert!(matches!(s.padding(1), Err(FramedEncodeError::InvalidState)));
    assert!(matches!(
        s.process(&mut [0; 64], FramedEncodeOperation::Process)
            .unwrap_err()
            .error,
        FramedEncodeError::InvalidState
    ));
}

#[test]
fn a_finishing_container_rejects_commands_and_process() {
    let mut owner = FramedCompressor::new(config()).expect("config");
    let mut s = owner
        .start(FramedEncodeStreamConfig::default())
        .expect("start");
    header(&mut s);
    let p = s
        .process(&mut [0; 1], FramedEncodeOperation::Finish)
        .expect("partial suffix");
    assert_eq!(p.status, FramedEncoderStatus::NeedsOutput);
    assert!(matches!(s.padding(1), Err(FramedEncodeError::InvalidState)));
    let e = s
        .process(&mut [0; 64], FramedEncodeOperation::Process)
        .unwrap_err();
    assert!(matches!(e.error, FramedEncodeError::InvalidState));
}

#[cfg(not(feature = "no_std"))]
#[test]
fn writer_defers_failure_after_acceptance_and_recovers_oversized_sink_counts() {
    use std::io::{self, Write};
    let mut owner = FramedCompressor::new(config().with_framing_config(FramingConfig {
        chunk_bytes: 7,
        max_chunks: 1,
        ..Default::default()
    }))
    .expect("config");
    {
        let mut writer = owner
            .framed_writer(Vec::new(), FramedEncodeStreamConfig::default())
            .expect("writer");
        let mut r = writer
            .uncompressed_resource(ResourceOptions::default())
            .expect("resource");
        assert_eq!(r.write(&[b'x'; 21]).expect("accepted"), 14);
        assert!(r.write(b"x").is_err());
    }
    #[derive(Debug, Default)]
    struct Oversized {
        bad: bool,
        bytes: Vec<u8>,
    }
    impl Write for Oversized {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.bad {
                Ok(bytes.len() + 1)
            } else {
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    owner.reconfigure(config()).expect("reconfigure");
    let expected = owner.compress([].as_slice().into()).expect("compress");
    let mut writer = owner
        .framed_writer(
            Oversized {
                bad: true,
                ..Default::default()
            },
            FramedEncodeStreamConfig::default(),
        )
        .expect("writer");
    assert!(matches!(writer.try_finish(), Err(FramedEncodeError::Io(_))));
    writer.get_mut().bad = false;
    assert!(writer.get_ref().bytes.is_empty());
    assert_eq!(writer.finish().expect("retried").bytes, expected);
}

/// A sink that fails in a scripted way, then behaves.
#[cfg(not(feature = "no_std"))]
#[derive(Debug, Default)]
struct Scripted {
    script: Vec<std::io::Result<usize>>,
    bytes: Vec<u8>,
    flushes: usize,
}

#[cfg(not(feature = "no_std"))]
impl std::io::Write for Scripted {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.script.is_empty() {
            self.bytes.extend_from_slice(bytes);
            return Ok(bytes.len());
        }
        match self.script.remove(0) {
            Ok(n) => {
                let n = n.min(bytes.len());
                self.bytes.extend_from_slice(&bytes[..n]);
                Ok(n)
            }
            Err(e) => Err(e),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

#[cfg(not(feature = "no_std"))]
#[test]
fn writer_transport_retries_short_interrupted_and_blocked_writes() {
    use std::io::{ErrorKind, Write};
    let payload = corpus::text(5000, 8);
    let items = [FramedItem::Resource(FramedResource {
        stream: StreamConfig::default(),
        ..FramedResource::from(payload.as_slice())
    })];
    let mut owner = FramedCompressor::new(chunked(1024)).expect("config");
    let expected = owner.compress(items.as_slice().into()).expect("compress");
    let sink = Scripted {
        script: vec![
            Ok(1),
            Err(ErrorKind::Interrupted.into()),
            Ok(3),
            Err(ErrorKind::WouldBlock.into()),
        ],
        ..Default::default()
    };
    let mut writer = owner
        .framed_writer(sink, FramedEncodeStreamConfig::default())
        .expect("writer");
    let mut resource = loop {
        match writer.resource(ResourceOptions::default(), StreamConfig::default()) {
            Ok(resource) => break resource,
            Err(FramedEncodeError::Io(e)) => assert_eq!(e.kind(), ErrorKind::WouldBlock),
            Err(e) => panic!("{e}"),
        }
    };
    resource.write_all(&payload).expect("write");
    resource.flush().expect("flush");
    resource.try_finish().expect("finish resource");
    drop(resource);
    writer.flush().expect("flush");
    assert!(writer.get_ref().flushes >= 2);
    let next = writer.next_chunk_offset();
    let bytes = writer.finish().expect("finish").bytes;
    let container = parse(&bytes);
    let directory = container
        .chunks
        .iter()
        .find(|c| c.kind == 9)
        .expect("directory");
    assert_eq!(directory.offset, next);
    assert!(bytes.len() > expected.len(), "the flush cut an extra chunk");
    assert_eq!(resources(&bytes, None), [payload]);
}

#[cfg(not(feature = "no_std"))]
#[test]
fn a_failed_finish_keeps_the_writer_for_a_retry() {
    use std::io::ErrorKind;
    let mut owner = FramedCompressor::new(config()).expect("config");
    let expected = owner.compress([].as_slice().into()).expect("compress");
    let sink = Scripted {
        script: vec![Ok(0)],
        ..Default::default()
    };
    let writer = owner
        .framed_writer(sink, FramedEncodeStreamConfig::default())
        .expect("writer");
    let failure = writer.finish().unwrap_err();
    assert!(matches!(&failure.error, FramedEncodeError::Io(e) if e.kind() == ErrorKind::WriteZero));
    let failure = *failure;
    assert_eq!(failure.writer.finish().expect("retried").bytes, expected);
}

#[cfg(not(feature = "no_std"))]
#[test]
fn writer_commands_mirror_session_commands() {
    use std::io::Write;
    let config = config().with_framing_config(FramingConfig {
        chunk_bytes: 32,
        repeat_metadata: true,
        ..Default::default()
    });
    let dictionary = prepared(b"dictionary words", &config);
    let references = [DictionaryReference::PrefixId(DictionaryId([4; 32]))];
    let fields = [MetadataField {
        code: *b"id",
        value: b"name",
    }];
    let items = [
        FramedItem::Metadata {
            kind: MetadataKind::Resource,
            fields: &fields,
            options: MetadataOptions::default(),
        },
        FramedItem::Resource(FramedResource {
            data: b"dictionary words, dictionary words",
            options: ResourceOptions::default(),
            stream: StreamConfig::default(),
            encoding: ResourceEncoding::Shared {
                dictionary: &dictionary,
                references: &references,
            },
        }),
        FramedItem::Metadata {
            kind: MetadataKind::Footer,
            fields: &[],
            options: MetadataOptions::default(),
        },
        FramedItem::Padding { bytes: 4 },
        FramedItem::Resource(FramedResource {
            data: b"verbatim",
            options: ResourceOptions::default(),
            stream: StreamConfig::default(),
            encoding: ResourceEncoding::Uncompressed,
        }),
    ];
    let mut owner = FramedCompressor::new(config).expect("config");
    let expected = owner
        .compress(FramedInput {
            items: &items,
            repeat_metadata_fields: Some(&[*b"id"]),
        })
        .expect("compress");
    let mut writer = owner
        .framed_writer(Vec::new(), FramedEncodeStreamConfig::default())
        .expect("writer");
    writer.repeat_metadata_fields(&[*b"id"]).expect("selection");
    writer
        .metadata(MetadataKind::Resource, &fields)
        .expect("metadata");
    {
        let mut r = writer
            .resource_with_dictionary(
                ResourceOptions::default(),
                StreamConfig::default(),
                &dictionary,
                &references,
            )
            .expect("resource");
        r.write_all(b"dictionary words, dictionary words")
            .expect("write");
        r.try_finish().expect("finish resource");
        assert!(r.get_mut().len() > 13);
    }
    writer
        .metadata_with_options(MetadataKind::Footer, &[], MetadataOptions::default())
        .expect("footer");
    writer.padding(4).expect("padding");
    {
        let mut r = writer
            .uncompressed_resource(ResourceOptions::default())
            .expect("resource");
        r.write_all(b"verbatim").expect("write");
        assert_eq!(r.write(&[]).expect("empty write"), 0);
        r.try_finish().expect("finish resource");
        assert!(r.write(b"late").is_err());
    }
    assert_eq!(writer.finish().expect("finish"), expected);
    let mut writer = owner
        .framed_writer(Vec::new(), FramedEncodeStreamConfig::default())
        .expect("writer");
    writer.padding(1).expect("padding");
    assert!(writer.into_inner().len() >= 13);
}

#[test]
fn exact_and_unknown_resource_hints_produce_the_same_frames() {
    let data = corpus::text(200_000, 2);
    let mut owner = FramedCompressor::new(chunked(65_536)).expect("config");
    let exact = [FramedItem::Resource(FramedResource::from(data.as_slice()))];
    let unknown = [FramedItem::Resource(FramedResource {
        stream: StreamConfig::default(),
        ..FramedResource::from(data.as_slice())
    })];
    let exact_bytes = owner.compress(exact.as_slice().into()).expect("compress");
    let unknown_bytes = owner.compress(unknown.as_slice().into()).expect("compress");
    assert_eq!(
        unknown_bytes, exact_bytes,
        "every chunk is compressed one-shot"
    );
    assert_eq!(
        structured_native(&mut owner, unknown.as_slice().into(), 31),
        unknown_bytes
    );
}

#[test]
fn large_resources_preserve_bytes_under_deterministic_random_backpressure() {
    let mut rng = 0xa076_1d64_78bd_642fu64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng as usize
    };
    let data: Vec<u8> = (0..65_553).map(|_| next() as u8).collect();
    let mut owner = FramedCompressor::new(chunked(4096)).expect("config");
    for stored in [false, true] {
        let mut description = FramedResource::from(data.as_slice());
        if stored {
            description.encoding = ResourceEncoding::Uncompressed;
        }
        let items = [FramedItem::Resource(description)];
        let expected = owner.compress(items.as_slice().into()).expect("compress");
        for _ in 0..4 {
            let mut session = owner
                .start(FramedEncodeStreamConfig::default())
                .expect("start");
            let mut output = [0; 4096];
            let mut actual = Vec::new();
            loop {
                let width = [0, 1, 7, 31, 4096][next() % 5];
                let p = session
                    .process(&mut output[..width], FramedEncodeOperation::Process)
                    .expect("header");
                actual.extend_from_slice(&output[..p.produced]);
                if p.status == FramedEncoderStatus::NeedsInput {
                    break;
                }
            }
            {
                let mut resource = if stored {
                    session
                        .uncompressed_resource(ResourceOptions::default())
                        .expect("resource")
                } else {
                    session
                        .resource(ResourceOptions::default(), description.stream)
                        .expect("resource")
                };
                let mut cursor = 0;
                let mut end = 0;
                loop {
                    if cursor == end && end < data.len() {
                        end = (end + 1 + next() % 8192).min(data.len());
                    }
                    let width = [0, 1, 7, 31, 4096][next() % 5];
                    let p = resource
                        .process(
                            &data[cursor..end],
                            &mut output[..width],
                            if end == data.len() {
                                Operation::End
                            } else {
                                Operation::Continue
                            },
                        )
                        .expect("payload");
                    cursor += p.consumed;
                    actual.extend_from_slice(&output[..p.produced]);
                    if p.status == FramedEncoderStatus::Finished {
                        assert_eq!(cursor, data.len());
                        break;
                    }
                }
            }
            loop {
                let width = [0, 1, 7, 31, 4096][next() % 5];
                let p = session
                    .process(&mut output[..width], FramedEncodeOperation::Finish)
                    .expect("finish");
                actual.extend_from_slice(&output[..p.produced]);
                if p.status == FramedEncoderStatus::Finished {
                    break;
                }
            }
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn small_repeated_metadata_headers_fit_the_existing_framing_budget() {
    let config = config().with_framing_config(FramingConfig {
        chunk_bytes: 31,
        repeat_metadata: true,
        max_buffer_bytes: 128 << 10,
        ..Default::default()
    });
    let mut items = Vec::new();
    for _ in 0..150 {
        items.push(FramedItem::Metadata {
            kind: MetadataKind::Resource,
            fields: &[],
            options: MetadataOptions::default(),
        });
        items.push(FramedItem::Resource(FramedResource {
            encoding: ResourceEncoding::Uncompressed,
            ..FramedResource::from(&b""[..])
        }));
    }
    let bytes = FramedCompressor::new(config)
        .expect("config")
        .compress(items.as_slice().into())
        .expect("compress");
    assert_eq!(parse(&bytes).resources.len(), 150);
}

#[test]
fn the_const_framed_encoder_configuration_equals_the_default() {
    const CONFIG: FramedEncodeConfig = FramedEncodeConfig::new();
    assert_eq!(CONFIG, FramedEncodeConfig::default());
    assert_eq!(*CONFIG.encoder_config(), EncoderConfig::default());
    assert_eq!(*CONFIG.framing_config(), FramingConfig::default());
}

#[test]
fn error_messages_name_their_cause() {
    assert_eq!(
        FramedEncodeError::InputSizeMismatch {
            scope: "resource",
            expected: 1,
            actual: 2
        }
        .to_string(),
        "resource input size mismatch: expected 1, actual 2"
    );
    assert_eq!(
        FramedEncodeError::Invalid("x").to_string(),
        "invalid framing operation: x"
    );
    assert_eq!(
        FramedEncodeError::Overflow.to_string(),
        "framing size or offset overflow"
    );
    assert_eq!(
        FramedEncodeError::AbandonedSession.to_string(),
        "a framed session was abandoned; recover the owner"
    );
    let location = FramedEncodeLocation::default();
    assert_eq!(location.wire_offset, 0);
    assert_eq!(location.item_index, None);
}
