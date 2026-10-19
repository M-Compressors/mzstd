//! The framed decoder: detection, profiles, streaming shapes, validation of
//! every structure, dictionaries, limits and owner lifecycle.
#![cfg(all(feature = "framed", feature = "compression", feature = "decompression"))]

mod framed_decoder_support;

use framed_decoder_support::*;
use mzstd::framing::*;
use mzstd::{Backend, DecodeError, DecodeOperation, RetentionPolicy};
use zstd_ffi::{corpus, oracle};

fn decoder() -> FramedDecompressor {
    FramedDecompressor::new(FramedDecodeConfig::default()).expect("decoder")
}

fn auto() -> FramedDecompressor {
    FramedDecompressor::new(FramedDecodeConfig::default().with_input_mode(InputMode::Auto))
        .expect("decoder")
}

fn with_limits(limits: FramedDecodeLimits) -> FramedDecompressor {
    FramedDecompressor::new(FramedDecodeConfig::default().with_limits(limits)).expect("decoder")
}

fn error(decoder: &mut FramedDecompressor, bytes: &[u8]) -> FramedDecodeError {
    decoder.decompress(bytes).expect_err("must fail")
}

fn encode(items: &[FramedItem<'_>]) -> Vec<u8> {
    FramedCompressor::new(FramedEncodeConfig::default())
        .expect("encoder")
        .compress(items.into())
        .expect("encode")
}

fn encode_with(config: FramingConfig, items: &[FramedItem<'_>]) -> Vec<u8> {
    FramedCompressor::new(FramedEncodeConfig::default().with_framing_config(config))
        .expect("encoder")
        .compress(items.into())
        .expect("encode")
}

fn hidden(data: &[u8]) -> FramedItem<'_> {
    FramedItem::Resource(FramedResource {
        data,
        options: ResourceOptions {
            hidden: true,
            ..Default::default()
        },
        stream: Default::default(),
        encoding: ResourceEncoding::Zstd,
    })
}

fn visible(data: &[u8]) -> FramedItem<'_> {
    FramedItem::Resource(FramedResource::from(data))
}

/// Offset of the first byte of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("present")
}

#[test]
fn hand_built_containers_decode_like_the_encoder_writes_them() {
    let mut builder = Builder::new(4);
    builder.resource(b"abc");
    builder.resource(b"de");
    let directory = builder.directory(0);
    builder.footer();
    let output = decoder().decompress(&builder.finish()).expect("decode");
    assert_eq!(output.resources.len(), 2);
    assert_eq!(output.resources[0].data, b"abc");
    assert_eq!(output.resources[1].data, b"de");
    let OutputStructure::Framed { header, layout } = output.structure else {
        panic!("framed");
    };
    assert!(header.has_footer());
    assert_eq!(layout.directory.as_ref().map(|d| d.entries.len()), Some(2));
    assert_eq!(
        layout.footer.and_then(|f| f.directory),
        Some(ChunkOffset(directory))
    );
    assert_eq!(
        decoder()
            .decompress(&single(b"xyz"))
            .expect("single")
            .resources[0]
            .data,
        b"xyz"
    );
}

#[test]
fn detection_separates_raw_frames_dictionaries_and_bad_signatures() {
    let raw = frame(b"raw payload");
    assert!(matches!(
        error(&mut decoder(), &raw),
        FramedDecodeError::InvalidSignature
    ));
    let output = auto().decompress(&raw).expect("raw");
    assert!(matches!(output.structure, OutputStructure::Raw));
    assert_eq!(output.resources[0].data, b"raw payload");
    assert_eq!(output.resources[0].source, ResourceSource::Raw);
    let mut dictionary = 0xEC30_A437u32.to_le_bytes().to_vec();
    dictionary.extend([1, 0, 0, 0]);
    for mut decoder in [decoder(), auto()] {
        assert!(matches!(
            error(&mut decoder, &dictionary),
            FramedDecodeError::UnexpectedInputKind(UnexpectedInputKind::SerializedDictionary)
        ));
    }
    let mut versioned = single(b"v");
    versioned[12] |= 1;
    assert!(matches!(
        error(&mut decoder(), &versioned),
        FramedDecodeError::UnsupportedVersion(1)
    ));
    for at in [4, 8, 11] {
        let mut bad = single(b"v");
        bad[at] ^= 0x40;
        assert!(matches!(
            error(&mut decoder(), &bad),
            FramedDecodeError::InvalidSignature
        ));
    }
    assert!(matches!(
        error(&mut decoder(), &[]),
        FramedDecodeError::UnexpectedEndOfInput
    ));
    assert!(matches!(
        error(&mut auto(), &[0x28, 0xb5]),
        FramedDecodeError::UnexpectedEndOfInput
    ));
}

#[test]
fn auto_mode_replays_a_skippable_prefix_that_only_resembles_a_container() {
    // A chunk-magic skippable frame whose size is not the header's is raw
    // input: the detection bytes are replayed into the codec.
    let mut raw = 0x184D_2A5Bu32.to_le_bytes().to_vec();
    raw.extend(6u32.to_le_bytes());
    raw.extend(b"MZFR\x04\x00");
    raw.extend(frame(b"after"));
    raw.extend(frame(b" two"));
    let expected = auto().decompress(&raw).expect("raw").resources[0]
        .data
        .clone();
    assert_eq!(expected, b"after two");
    for split in [1, 3, 7, raw.len()] {
        let (events, payload) = drive(&mut auto(), &raw, split, 2).expect("streamed");
        assert_eq!(payload, expected, "split {split}");
        assert!(events[0].contains("StreamStart(Raw)"));
    }
    let mut auto = auto();
    let mut session = auto.start(Default::default()).expect("start");
    assert_eq!(session.input_format(), None);
    let mut output = [0; 4];
    let progress = session
        .process(&raw[..12], &mut output, DecodeOperation::Process)
        .expect("detect");
    assert!(matches!(
        progress.status,
        FramedDecoderStatus::Event(FramedEvent::StreamStart(StreamInfo::Raw))
    ));
    assert_eq!(session.input_format(), Some(StreamInfo::Raw));
}

#[test]
fn raw_input_is_one_resource_of_every_concatenated_frame() {
    let raw = [EMPTY_FRAME.as_slice(), &frame(b"one"), &EMPTY_FRAME].concat();
    let (events, payload) = drive(&mut auto(), &raw, 1, 1).expect("raw");
    assert_eq!(payload, b"one");
    assert!(events[0].contains("StreamStart(Raw)"));
    assert!(events[1].starts_with("ResourceStart"));
    assert!(events.last().expect("end").starts_with("ResourceDataEnd"));
    let output = auto().decompress(&EMPTY_FRAME).expect("empty");
    assert!(output.resources[0].data.is_empty());
    assert!(output.global_metadata.is_empty());
}

#[test]
fn raw_input_must_end_with_a_complete_frame() {
    let mut truncated = frame(b"truncated raw frame");
    truncated.pop();
    assert!(matches!(
        error(&mut auto(), &truncated),
        FramedDecodeError::Decode {
            source: DecodeError::UnexpectedEndOfInput,
            ..
        }
    ));
}

#[test]
fn the_full_profile_permits_an_empty_container_and_the_single_profile_one_resource() {
    let empty = encode(&[]);
    let output = decoder().decompress(&empty).expect("empty");
    assert!(output.resources.is_empty());
    let mut builder = Builder::new(0);
    builder.padding(2);
    builder.resource(b"one");
    builder.padding(0);
    let output = decoder().decompress(&builder.finish()).expect("padded");
    assert_eq!(output.resources[0].data, b"one");
    // No resource, two resources, metadata: not the single-resource profile.
    let none = Builder::new(0).finish();
    assert!(matches!(
        error(&mut decoder(), &none),
        FramedDecodeError::InvalidOrder
    ));
    let mut two = Builder::new(0);
    two.resource(b"a");
    two.resource(b"b");
    assert!(matches!(
        error(&mut decoder(), &two.finish()),
        FramedDecodeError::InvalidOrder
    ));
    let mut metadata = Builder::new(0);
    metadata.metadata(7, b"AA\x01z");
    assert!(matches!(
        error(&mut decoder(), &metadata.finish()),
        FramedDecodeError::InvalidOrder
    ));
}

#[test]
fn partial_hidden_metadata_and_padding_preserve_structure() {
    let id = [MetadataField {
        code: *b"id",
        value: b"name",
    }];
    let tail = [MetadataField {
        code: *b"ZZ",
        value: b"after",
    }];
    let global = [MetadataField {
        code: *b"GL",
        value: b"global",
    }];
    let big = corpus::text(10_000, 4);
    let items = [
        FramedItem::Metadata {
            kind: MetadataKind::Global,
            fields: &global,
            options: Default::default(),
        },
        FramedItem::Metadata {
            kind: MetadataKind::Resource,
            fields: &id,
            options: MetadataOptions {
                encoding: MetadataEncoding::Zstd,
                ..Default::default()
            },
        },
        visible(&big),
        FramedItem::Metadata {
            kind: MetadataKind::Footer,
            fields: &tail,
            options: Default::default(),
        },
        FramedItem::Padding { bytes: 5 },
        hidden(b"hidden bytes"),
    ];
    let bytes = encode_with(
        FramingConfig {
            chunk_bytes: 4096,
            ..Default::default()
        },
        &items,
    );
    let output = decoder().decompress(&bytes).expect("decode");
    assert_eq!(output.resources[0].data, big);
    assert_eq!(
        output.resources[0]
            .metadata
            .as_ref()
            .and_then(Metadata::name),
        Some("name")
    );
    assert_eq!(
        output.resources[0]
            .footer_metadata
            .as_ref()
            .map(Metadata::as_bytes),
        Some(&b"ZZ\x05after"[..])
    );
    assert!(output.resources[1].hidden);
    assert_eq!(output.resources[1].data, b"hidden bytes");
    assert_eq!(output.global_metadata.len(), 1);
    assert_eq!(output.global_metadata[0].as_bytes(), b"GL\x06global");
    let OutputStructure::Framed { layout, .. } = output.structure else {
        panic!("framed");
    };
    let kinds: Vec<_> = layout.chunks.iter().map(|c| c.kind).collect();
    assert!(kinds.contains(&ChunkType::FirstPartial));
    assert!(kinds.contains(&ChunkType::MiddlePartial));
    assert!(kinds.contains(&ChunkType::LastPartial));
    assert!(kinds.contains(&ChunkType::Padding));
    let detached = layout
        .chunks
        .iter()
        .filter(|c| c.body_length.is_some())
        .count();
    assert_eq!(detached, 3);
    assert_eq!(oracle::decompress(&bytes, 20_000).expect("reference"), big);
}

#[test]
fn every_input_split_and_output_size_reproduces_the_owned_result() {
    let fields = [MetadataField {
        code: *b"AB",
        value: b"cd",
    }];
    let payload = corpus::text(3000, 5);
    let items = [
        FramedItem::Metadata {
            kind: MetadataKind::Resource,
            fields: &fields,
            options: Default::default(),
        },
        visible(&payload),
        FramedItem::Padding { bytes: 1 },
        hidden(b"secret"),
        visible(b""),
    ];
    let bytes = encode_with(
        FramingConfig {
            chunk_bytes: 1000,
            repeat_metadata: true,
            ..Default::default()
        },
        &items,
    );
    let (reference, data) = drive(&mut decoder(), &bytes, bytes.len(), 8192).expect("whole");
    assert_eq!(data, [payload.as_slice(), b"secret"].concat());
    for split in [1, 2, 3, 7, 64, 500] {
        for capacity in [0, 1, 5, 4096] {
            let (events, data2) = drive(&mut decoder(), &bytes, split, capacity).expect("split");
            assert_eq!(data2, data, "split {split} capacity {capacity}");
            let filter = |e: &Vec<String>| {
                let mut v: Vec<String> = e
                    .iter()
                    .filter(|s| !s.starts_with("data"))
                    .cloned()
                    .collect();
                v.dedup();
                v
            };
            assert_eq!(filter(&events), filter(&reference));
        }
    }
    assert!(reference.iter().any(|e| e.starts_with("DirectoryStart")));
    assert!(reference.iter().any(|e| e.starts_with("Footer")));
    assert!(reference.iter().any(|e| e.starts_with("Padding")));
    assert!(reference.iter().any(|e| e.starts_with("ResourceDataEnd")));
    assert!(reference.iter().any(|e| e.contains("original: Some")));
}

#[test]
fn process_flush_and_finish_are_the_same_operation_in_three_shapes() {
    let bytes = single(b"shapes");
    let mut decoder = decoder();
    let mut session = decoder.start(Default::default()).expect("start");
    let mut payload = Vec::new();
    let mut remaining = &bytes[..];
    let mut buffer = [0; 2];
    while !remaining.is_empty() {
        let progress = session
            .process(remaining, &mut buffer, DecodeOperation::Process)
            .expect("process");
        remaining = &remaining[progress.consumed..];
        if let FramedDecoderStatus::Event(FramedEvent::ResourceData(data)) = progress.status {
            payload.extend_from_slice(data.bytes);
        }
    }
    loop {
        let progress = session.flush(&mut buffer).expect("flush");
        match progress.status {
            FramedDecoderStatus::Event(FramedEvent::ResourceData(data)) => {
                payload.extend_from_slice(data.bytes);
            }
            // A container ends itself with its seek table.
            FramedDecoderStatus::NeedsInput | FramedDecoderStatus::Finished => break,
            _ => {}
        }
    }
    assert!(session.is_finished());
    loop {
        let progress = session.finish(&mut buffer).expect("finish");
        if matches!(progress.status, FramedDecoderStatus::Finished) {
            break;
        }
    }
    assert_eq!(payload, b"shapes");
    assert!(session.is_finished());
    assert_eq!(session.total_in(), bytes.len() as u64);
    assert_eq!(session.total_out(), 6);
    assert_eq!(session.total_decoded(), 6);
    assert_eq!(session.resources_decoded(), 1);
    // Finished stays finished, with no progress.
    let progress = session.finish(&mut buffer).expect("finished");
    assert!(matches!(progress.status, FramedDecoderStatus::Finished));
    assert_eq!((progress.consumed, progress.produced), (0, 0));
    // Changing the final boundary or reverting to Process after Finish fails.
    let mut decoder2 = self::decoder();
    let mut session = decoder2.start(Default::default()).expect("start");
    session
        .process(&bytes[..3], &mut [0; 1], DecodeOperation::Finish)
        .expect_err("truncated");
    let failure = session
        .process(&bytes[3..], &mut [0; 1], DecodeOperation::Finish)
        .expect_err("failed");
    assert!(matches!(failure.error, FramedDecodeError::InvalidState));
    let mut decoder3 = self::decoder();
    let mut session = decoder3.start(Default::default()).expect("start");
    session
        .process(&bytes[..2], &mut [0; 1], DecodeOperation::Finish)
        .expect_err("truncated");
    let mut decoder4 = self::decoder();
    let mut session = decoder4.start(Default::default()).expect("start");
    let _ = session
        .process(&bytes[..13], &mut [0; 1], DecodeOperation::Finish)
        .expect("header");
    let failure = session
        .process(&bytes[13..], &mut [0; 1], DecodeOperation::Process)
        .expect_err("operation changed");
    assert!(matches!(failure.error, FramedDecodeError::InvalidState));
}

#[test]
fn every_truncated_prefix_fails_with_unexpected_end() {
    let bytes = two_resources();
    for end in 0..bytes.len() {
        let error = error(&mut decoder(), &bytes[..end]);
        assert!(
            matches!(
                error,
                FramedDecodeError::UnexpectedEndOfInput
                    | FramedDecodeError::InvalidSeekTable
                    | FramedDecodeError::InvalidChunk
            ),
            "end {end}: {error:?}"
        );
    }
    // Truncation inside the seek table entries is reported as an end of input.
    assert!(matches!(
        error(&mut decoder(), &bytes[..bytes.len() - 1]),
        FramedDecodeError::UnexpectedEndOfInput
    ));
    let mut session_decoder = decoder();
    let mut session = session_decoder.start(Default::default()).expect("start");
    let mut accepted = 0;
    let failure = loop {
        match session.process(&bytes[accepted..20], &mut [0; 8], DecodeOperation::Finish) {
            Ok(progress) => accepted += progress.consumed,
            Err(failure) => break failure,
        }
    };
    assert_eq!(accepted + failure.consumed, 20);
    assert_eq!(session.total_in(), 20);
    assert!(matches!(
        failure.error,
        FramedDecodeError::UnexpectedEndOfInput
    ));
}

#[test]
fn trailing_data_and_short_slices_are_reported_with_exact_progress() {
    let bytes = two_resources();
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(matches!(
        error(&mut decoder(), &trailing),
        FramedDecodeError::TrailingData
    ));
    let mut target = vec![b'p'; 3];
    let error = decoder()
        .decompress_into(&trailing, &mut target)
        .expect_err("trailing");
    assert!(matches!(error, FramedDecodeError::TrailingData));
    assert_eq!(target, b"ppp");
    let failure = decoder()
        .decompress_to_slice(&trailing, &mut [0; 16])
        .expect_err("trailing");
    assert!(matches!(failure.error, FramedDecodeError::TrailingData));
    assert_eq!(failure.consumed, bytes.len());
    assert_eq!(failure.produced, 5);
    let mut short = [0; 4];
    let failure = decoder()
        .decompress_to_slice(&bytes, &mut short)
        .expect_err("short");
    assert!(matches!(failure.error, FramedDecodeError::OutputTooSmall));
    assert_eq!(failure.produced, 4);
    assert_eq!(&short, b"abcd");
    let last = failure.last_output.expect("fragment");
    assert_eq!(last.range, 3..4);
    assert_eq!(last.position.resource, ResourceIndex(1));
    assert_eq!(last.position.offset, 0);
    let mut exact = [0; 5];
    let output = decoder()
        .decompress_to_slice(&bytes, &mut exact)
        .expect("fits");
    assert_eq!(output.resources[1].data, 3..5);
    assert_eq!(&exact, b"abcde");
    let mut appended = b"xy".to_vec();
    let output = decoder()
        .decompress_into(&bytes, &mut appended)
        .expect("append");
    assert_eq!(output.resources[0].data, 2..5);
    assert_eq!(appended, b"xyabcde");
}

#[test]
fn chunk_headers_reject_unknown_types_codecs_and_flags() {
    let cases: [(&[u8], &[u8]); 5] = [
        (&[11], b""),
        (&[2, 1, 0, 0], b"x"),
        (&[2, 0, 8], b"x"),
        (&[4, 0, 1], b"x"),
        (&[2, 0, 4, 1], b""),
    ];
    for (fields, inline) in cases {
        let mut builder = Builder::new(4);
        builder.chunk(fields, inline, None);
        assert!(
            matches!(
                error(&mut decoder(), &builder.finish()),
                FramedDecodeError::InvalidChunk | FramedDecodeError::InvalidOrder
            ),
            "{fields:?}"
        );
    }
    let mut builder = Builder::new(4);
    builder.chunk(&[11], b"", None);
    assert!(matches!(
        error(&mut decoder(), &builder.finish()),
        FramedDecodeError::InvalidChunk
    ));
    let mut builder = Builder::new(4);
    builder.chunk(&[8, 0, 2], b"", None);
    assert!(matches!(
        error(&mut decoder(), &builder.finish()),
        FramedDecodeError::InvalidMetadata
    ));
    // A bare Zstandard frame where a chunk header belongs.
    let mut bare = Builder::new(4);
    bare.bytes.extend(frame(b"bare"));
    assert!(matches!(
        error(&mut decoder(), &bare.finish()),
        FramedDecodeError::InvalidChunk
    ));
    // Another skippable magic.
    let mut other = Builder::new(4);
    other.bytes.extend(0x184D_2A50u32.to_le_bytes());
    other.bytes.extend(0u32.to_le_bytes());
    assert!(matches!(
        error(&mut decoder(), &other.finish()),
        FramedDecodeError::InvalidChunk
    ));
    // Padding must be zero.
    let mut padding = Builder::new(4);
    padding.chunk(&[0], &[0, 1], None);
    assert!(matches!(
        error(&mut decoder(), &padding.finish()),
        FramedDecodeError::InvalidChunk
    ));
}

#[test]
fn visible_payload_is_detached_and_hidden_payload_inline() {
    // A visible resource stored inline.
    let mut inline = Builder::new(4);
    inline.chunk(&[2, 0, 0], b"inline", None);
    assert!(matches!(
        error(&mut decoder(), &inline.finish()),
        FramedDecodeError::InvalidChunk
    ));
    // A hidden resource with a detached body.
    let body = frame(b"body");
    let mut detached = Builder::new(4);
    detached.chunk(&visible_fields(2, 4, 1, &body), &[], Some((&body, 4)));
    assert!(matches!(
        error(&mut decoder(), &detached.finish()),
        FramedDecodeError::InvalidChunk
    ));
    // A hidden first chunk makes its continuation inline too.
    let mut partial = Builder::new(4);
    partial.chunk(&[3, 0, 1], b"ab", None);
    let body = frame(b"cd");
    partial.chunk(&visible_fields(5, 2, 0, &body), &[], Some((&body, 2)));
    assert!(matches!(
        error(&mut decoder(), &partial.finish()),
        FramedDecodeError::InvalidChunk
    ));
    // Inline compressed hidden content is one complete frame.
    let mut hidden = Builder::new(4);
    let content = frame(b"inner");
    let mut fields = vec![2, 2];
    fields.extend(var(5));
    fields.push(1);
    hidden.chunk(&fields, &content, None);
    hidden.footer();
    let output = decoder()
        .decompress(&hidden.finish())
        .expect("inline frame");
    assert_eq!(output.resources[0].data, b"inner");
}

#[test]
fn compressed_content_is_exactly_one_zstandard_frame() {
    let payload = b"exactly one frame of content";
    let body = frame(payload);
    let check = |body: &[u8], decoded: usize| {
        let mut builder = Builder::new(0);
        builder.chunk(
            &visible_fields(2, decoded, 0, body),
            &[],
            Some((body, decoded as u32)),
        );
        error(&mut decoder(), &builder.finish())
    };
    let mut short = body.clone();
    short.pop();
    assert!(matches!(
        check(&short, payload.len()),
        FramedDecodeError::InvalidChunk
    ));
    let mut long = body.clone();
    long.push(0);
    assert!(matches!(
        check(&long, payload.len()),
        FramedDecodeError::InvalidChunk
    ));
    let mut skippable = 0x184D_2A50u32.to_le_bytes().to_vec();
    skippable.extend(0u32.to_le_bytes());
    assert!(matches!(
        check(&skippable, 0),
        FramedDecodeError::InvalidChunk
    ));
    // Declared sizes are contracts in both directions.
    assert!(matches!(
        check(&body, payload.len() + 3),
        FramedDecodeError::DeclaredSizeMismatch { expected, actual }
            if expected == payload.len() as u64 + 3 && actual == payload.len() as u64
    ));
    assert!(matches!(
        check(&body, 4),
        FramedDecodeError::DeclaredSizeMismatch { expected: 4, .. }
    ));
    // A corrupt checksum surfaces the codec error as the source.
    let mut checked = oracle::compress2(
        payload,
        &oracle::Params::level(3).with(zstd_ffi::ZSTD_c_checksumFlag, 1),
    )
    .expect("reference");
    *checked.last_mut().expect("checksum") ^= 1;
    let error = check(&checked, payload.len());
    assert!(matches!(
        &error,
        FramedDecodeError::Decode {
            source: DecodeError::ChecksumMismatch { .. },
            location
        } if location.chunk() == Some(ChunkOffset(13)) && location.resource() == Some(ResourceIndex(0))
    ));
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn chunk_order_follows_resources_metadata_directory_and_footer() {
    let order = |build: &dyn Fn(&mut Builder)| {
        let mut builder = Builder::new(4);
        build(&mut builder);
        builder.footer();
        error(&mut decoder(), &builder.finish())
    };
    let cases: [&dyn Fn(&mut Builder); 7] = [
        // Continuation without a first chunk.
        &|b| {
            b.chunk(&[4, 0, 0], b"x", None);
        },
        // A complete resource inside a partial one.
        &|b| {
            b.chunk(&[3, 0, 1], b"x", None);
            b.hidden(b"y");
        },
        // Resource metadata not followed by its resource.
        &|b| {
            b.metadata(1, b"AA\x01x");
            b.metadata(7, b"BB\x01y");
        },
        // Footer metadata without a preceding resource.
        &|b| {
            b.metadata(6, b"AA\x01x");
        },
        // Anything but the footer after the directory.
        &|b| {
            b.hidden(b"x");
            b.directory(0);
            b.hidden(b"y");
        },
        // Original metadata after a repeated copy.
        &|b| {
            b.metadata(1, b"AA\x01x");
            b.hidden(b"r");
            b.chunk(&[8, 0, 1], b"AA\x01x", None);
            b.metadata(7, b"AA\x01x");
        },
        // Unterminated partial resource at the footer.
        &|b| {
            b.chunk(&[3, 0, 1], b"x", None);
        },
    ];
    for (i, case) in cases.iter().enumerate() {
        assert!(
            matches!(order(case), FramedDecodeError::InvalidOrder),
            "case {i}"
        );
    }
    // A chunk after the footer, and the seek table before it.
    let mut after = Builder::new(4);
    after.footer();
    after.hidden(b"late");
    assert!(matches!(
        error(&mut decoder(), &after.finish()),
        FramedDecodeError::InvalidOrder
    ));
    let mut early = Builder::new(4);
    early.hidden(b"x");
    assert!(matches!(
        error(&mut decoder(), &early.finish()),
        FramedDecodeError::InvalidOrder
    ));
    let mut foreign = Builder::new(4);
    foreign.footer();
    foreign.bytes.extend(frame(b"z"));
    assert!(matches!(
        error(&mut decoder(), &foreign.finish()),
        FramedDecodeError::InvalidOrder
    ));
}

#[test]
fn metadata_fields_reject_reserved_duplicates_and_broken_lengths() {
    let cases: [(u8, &[u8]); 7] = [
        (7, b"id\x01x"),
        (1, b"id\x01xid\x01y"),
        (1, b"mt\x01x"),
        (1, b"id\x01\xff"),
        (7, b"aB\x00"),
        (7, b"AB\x05x"),
        (7, b"A"),
    ];
    for (kind, fields) in cases {
        let mut builder = Builder::new(4);
        builder.metadata(kind, fields);
        builder.hidden(b"r");
        builder.footer();
        assert!(
            matches!(
                error(&mut decoder(), &builder.finish()),
                FramedDecodeError::InvalidMetadata
            ),
            "{fields:?}"
        );
    }
    // Repeated custom codes are lossless.
    let mut builder = Builder::new(4);
    builder.metadata(7, b"AA\x01xAA\x01y");
    builder.footer();
    let output = decoder().decompress(&builder.finish()).expect("repeats");
    let fields: Vec<_> = output.global_metadata[0]
        .fields()
        .map(|f| f.value)
        .collect();
    assert_eq!(fields, [b"x", b"y"]);
}

#[test]
fn repeated_metadata_must_match_its_original_on_selected_codes() {
    let fields = [
        MetadataField {
            code: *b"AA",
            value: b"1",
        },
        MetadataField {
            code: *b"BB",
            value: b"2",
        },
    ];
    let items = [
        FramedItem::Metadata {
            kind: MetadataKind::Resource,
            fields: &fields,
            options: Default::default(),
        },
        hidden(b"r"),
    ];
    let input = FramedInput {
        items: &items,
        repeat_metadata_fields: Some(&[*b"BB"]),
    };
    let bytes = FramedCompressor::new(FramedEncodeConfig::default().with_framing_config(
        FramingConfig {
            repeat_metadata: true,
            ..Default::default()
        },
    ))
    .expect("encoder")
    .compress(input)
    .expect("encode");
    let output = decoder().decompress(&bytes).expect("decode");
    let OutputStructure::Framed { layout, .. } = &output.structure else {
        panic!("framed");
    };
    assert_eq!(layout.repeated_metadata.len(), 1);
    assert_eq!(layout.repeated_metadata[0].metadata.as_bytes(), b"BB\x012");
    assert_eq!(layout.repeated_metadata[0].kind, MetadataKind::Resource);
    // Changing the copied value breaks the match.
    let mut changed = bytes.clone();
    let at = changed.len() - 1 - changed.iter().rev().position(|&b| b == b'2').expect("2");
    changed[at] = b'3';
    assert!(matches!(
        error(&mut decoder(), &changed),
        FramedDecodeError::InvalidMetadata | FramedDecodeError::InvalidDirectory
    ));
    // A repeat of the wrong kind, and more repeats than originals.
    for repeat in [&[8u8, 0, 6][..], &[8, 0, 1][..]] {
        let mut builder = Builder::new(4);
        builder.metadata(1, b"AA\x01x");
        builder.hidden(b"r");
        let first = builder.chunk(repeat, b"AA\x01x", None);
        if repeat[2] == 1 {
            builder.chunk(repeat, b"AA\x01x", None);
        }
        builder.directory(first);
        builder.footer();
        assert!(matches!(
            error(&mut decoder(), &builder.finish()),
            FramedDecodeError::InvalidMetadata
        ));
    }
    // Repeats without a directory are not a complete series.
    let mut builder = Builder::new(4);
    builder.metadata(1, b"AA\x01x");
    builder.hidden(b"r");
    builder.chunk(&[8, 0, 1], b"AA\x01x", None);
    builder.footer();
    assert!(matches!(
        error(&mut decoder(), &builder.finish()),
        FramedDecodeError::InvalidDirectory | FramedDecodeError::InvalidOrder
    ));
}

#[test]
fn the_directory_lists_every_content_chunk_header_exactly() {
    let mut good = Builder::new(4);
    good.hidden(b"x");
    good.resource(b"yz");
    good.directory(0);
    good.footer();
    let output = decoder().decompress(&good.finish()).expect("good");
    let OutputStructure::Framed { layout, .. } = output.structure else {
        panic!("framed");
    };
    let directory = layout.directory.expect("directory");
    assert_eq!(directory.entries[1].chunk_index, 1);
    assert_eq!(directory.repeated_metadata, None);
    let broken: [&dyn Fn(&mut Builder); 4] = [
        // A record is missing.
        &|b| {
            b.records.pop();
        },
        // A header byte differs.
        &|b| {
            let last = b.records[0].1.len() - 1;
            b.records[0].1[last] ^= 1;
        },
        // An offset differs.
        &|b| b.records[1].0 += 1,
        // A record the container lacks.
        &|b| {
            let copy = b.records[0].clone();
            b.records.push(copy);
        },
    ];
    for (i, mutate) in broken.iter().enumerate() {
        let mut builder = Builder::new(4);
        builder.hidden(b"x");
        builder.resource(b"yz");
        mutate(&mut builder);
        builder.directory(0);
        builder.footer();
        assert!(
            matches!(
                error(&mut decoder(), &builder.finish()),
                FramedDecodeError::InvalidDirectory
            ),
            "case {i}"
        );
    }
    // A repeat pointer without repeats.
    let mut builder = Builder::new(4);
    builder.hidden(b"x");
    builder.directory(13);
    builder.footer();
    assert!(matches!(
        error(&mut decoder(), &builder.finish()),
        FramedDecodeError::InvalidDirectory
    ));
}

#[test]
fn the_footer_names_the_directory_and_the_container_size() {
    let mut wrong_directory = Builder::new(4);
    wrong_directory.hidden(b"x");
    wrong_directory.directory(0);
    wrong_directory.footer_with(13, 0);
    assert!(matches!(
        error(&mut decoder(), &wrong_directory.finish()),
        FramedDecodeError::InvalidFooter
    ));
    let mut wrong_size = Builder::new(4);
    wrong_size.footer_with(0, 7);
    assert!(matches!(
        error(&mut decoder(), &wrong_size.finish()),
        FramedDecodeError::InvalidFooter
    ));
    let mut unspecified = Builder::new(4);
    unspecified.footer_with(0, 0);
    let output = decoder().decompress(&unspecified.finish()).expect("size 0");
    let OutputStructure::Framed { layout, .. } = output.structure else {
        panic!("framed");
    };
    assert_eq!(layout.footer.expect("footer").file_size, None);
    let mut short = Builder::new(4);
    short.chunk(
        &[10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        &[],
        None,
    );
    assert!(matches!(
        error(&mut decoder(), &short.finish()),
        FramedDecodeError::InvalidFooter
    ));
    // The footer is not part of the single-resource profile.
    let mut footerless = Builder::new(0);
    footerless.resource(b"x");
    footerless.footer_with(0, 0);
    assert!(matches!(
        error(&mut decoder(), &footerless.finish()),
        FramedDecodeError::InvalidOrder
    ));
}

#[test]
fn the_seek_table_must_list_every_frame_exactly() {
    let build = || {
        let mut builder = Builder::new(4);
        builder.resource(b"payload");
        builder.footer();
        builder
    };
    let frames = build().frames.clone();
    let mut fewer = frames.clone();
    fewer.pop();
    let mut entry = frames.clone();
    entry[2].1 += 1;
    let mut header = frames.clone();
    header[0].0 += 1;
    let mut more = frames.clone();
    more.push((8, 0));
    for (table, descriptor, magic) in [
        (fewer, 0, SEEKABLE_MAGIC),
        (entry, 0, SEEKABLE_MAGIC),
        (header, 0, SEEKABLE_MAGIC),
        (more, 0, SEEKABLE_MAGIC),
        (frames.clone(), 0x80, SEEKABLE_MAGIC),
        (frames.clone(), 0x04, SEEKABLE_MAGIC),
        (frames.clone(), 0, 0x1234_5678),
    ] {
        let bytes = build().finish_with(&table, descriptor, magic);
        assert!(
            matches!(
                error(&mut decoder(), &bytes),
                FramedDecodeError::InvalidSeekTable | FramedDecodeError::InvalidFooter
            ),
            "{descriptor} {magic:x}"
        );
    }
    // Unused descriptor bits are ignored.
    let mut unused = Builder::new(4);
    unused.footer_with(0, 0);
    let frames = unused.frames.clone();
    decoder()
        .decompress(&unused.finish_with(&frames, 0x03, SEEKABLE_MAGIC))
        .expect("unused bits");
    // A size that is not 8n + 9.
    let mut bytes = build().finish();
    let at = find(&bytes, &SEEK_MAGIC.to_le_bytes()) + 4;
    bytes[at] += 1;
    assert!(matches!(
        error(&mut decoder(), &bytes),
        FramedDecodeError::InvalidSeekTable
    ));
}

#[test]
fn declared_output_size_is_a_contract_across_resources() {
    let bytes = two_resources();
    for (expected, ok) in [(5, true), (4, false), (6, false)] {
        let mut decoder = decoder();
        let session = decoder
            .start(OutputSize::Exact(expected).into())
            .expect("start");
        drop(session);
        let mut session = decoder
            .start(OutputSize::Exact(expected).into())
            .expect("start");
        let mut output = [0; 64];
        let mut remaining = &bytes[..];
        let result = loop {
            match session.process(remaining, &mut output, DecodeOperation::Finish) {
                Ok(progress) => {
                    remaining = &remaining[progress.consumed..];
                    if matches!(progress.status, FramedDecoderStatus::Finished) {
                        break Ok(());
                    }
                }
                Err(failure) => break Err(failure.error),
            }
        };
        match (ok, result) {
            (true, Ok(())) => {}
            (false, Err(FramedDecodeError::DeclaredSizeMismatch { expected: e, .. })) => {
                assert_eq!(e, expected);
            }
            (_, other) => panic!("{expected}: {other:?}"),
        }
    }
    // An exact size above the output ceiling is rejected at start.
    let mut limited = with_limits(FramedDecodeLimits::default().with_max_output_bytes(Some(4)));
    assert!(matches!(
        limited.start(OutputSize::Exact(5).into()),
        Err(FramedDecodeError::LimitExceeded {
            kind: FramedLimitKind::OutputBytes,
            limit: 4
        })
    ));
    // Stored inline content honors the contract too.
    let mut builder = Builder::new(0);
    builder.hidden(b"stored");
    let bytes = builder.finish();
    let mut decoder = decoder();
    let mut session = decoder.start(OutputSize::Exact(2).into()).expect("start");
    let mut remaining = &bytes[..];
    let error = loop {
        match session.process(remaining, &mut [0; 64], DecodeOperation::Finish) {
            Ok(progress) => remaining = &remaining[progress.consumed..],
            Err(failure) => break failure.error,
        }
    };
    assert!(matches!(
        error,
        FramedDecodeError::DeclaredSizeMismatch { expected: 2, .. }
    ));
}

#[test]
fn every_dictionary_reference_kind_decodes_through_its_resolver() {
    let fixture = dictionaries();
    let output = decoder()
        .decompress_with_dictionaries(&fixture, &fixture.bytes)
        .expect("decode");
    assert_eq!(output.resources.len(), fixture.payloads.len());
    for (resource, payload) in output.resources.iter().zip(&fixture.payloads) {
        assert_eq!(&resource.data, payload);
    }
    assert!(output.resources[0].hidden && output.resources[1].hidden);
    let mut appended = Vec::new();
    let ranges = decoder()
        .decompress_with_dictionaries_into(&fixture, &fixture.bytes, &mut appended)
        .expect("append");
    assert_eq!(
        &appended[ranges.resources[2].data.clone()],
        &fixture.payloads[2][..]
    );
    let mut slice = vec![0; appended.len()];
    let ranges = decoder()
        .decompress_with_dictionaries_to_slice(&fixture, &fixture.bytes, &mut slice)
        .expect("slice");
    assert_eq!(
        &slice[ranges.resources[7].data.clone()],
        &fixture.payloads[7][..]
    );
    // Every split streams identically.
    let mut decoder = decoder();
    for split in [1, 97, 4096] {
        let mut session = decoder
            .start_with_dictionaries(&fixture, Default::default())
            .expect("start");
        let mut payload = Vec::new();
        let mut position = 0;
        let mut output = [0; 333];
        'outer: loop {
            let end = (position + split).min(fixture.bytes.len());
            let operation = if end == fixture.bytes.len() {
                DecodeOperation::Finish
            } else {
                DecodeOperation::Process
            };
            loop {
                let progress = session
                    .process(&fixture.bytes[position..end], &mut output, operation)
                    .expect("stream");
                position += progress.consumed;
                match progress.status {
                    FramedDecoderStatus::Finished => break 'outer,
                    FramedDecoderStatus::NeedsInput => break,
                    FramedDecoderStatus::Event(FramedEvent::ResourceData(data)) => {
                        payload.extend_from_slice(data.bytes);
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(payload, fixture.payloads.concat(), "split {split}");
    }
}

#[test]
fn missing_and_malformed_dictionaries_fail_where_they_are_attached() {
    let fixture = dictionaries();
    let error = decoder()
        .decompress(&fixture.bytes)
        .expect_err("no resolver");
    let FramedDecodeError::MissingDictionary { request, location } = error else {
        panic!("{error:?}");
    };
    assert_eq!(request.id, EXTERNAL);
    assert_eq!(request.kind, ExternalDictionaryKind::Prefix);
    assert_eq!(location.resource(), Some(ResourceIndex(5)));
    struct Wrong;
    impl DictionaryResolver for Wrong {
        fn resolve(&self, _: ExternalDictionaryRequest) -> Option<&[u8]> {
            Some(b"not a zstandard dictionary")
        }
    }
    // The prefix decodes wrongly but undetectably; the serialized one is
    // rejected when it is parsed.
    let error = decoder()
        .decompress_with_dictionaries(&Wrong, &fixture.bytes)
        .expect_err("wrong");
    assert!(
        matches!(
            &error,
            FramedDecodeError::Dictionary { .. } | FramedDecodeError::Decode { .. }
        ),
        "{error:?}"
    );
    if let FramedDecodeError::Dictionary { source, .. } = &error {
        assert_eq!(*source, mzstd::dictionary::DictionaryError::NotZstdFormat);
    }
    let reject = FramedDecompressor::new(
        FramedDecodeConfig::default().with_internal_dictionaries(InternalDictionaryPolicy::Reject),
    )
    .expect("decoder")
    .decompress_with_dictionaries(&fixture, &fixture.bytes)
    .expect_err("reject");
    assert!(matches!(
        reject,
        FramedDecodeError::InternalDictionaryReferencesDisabled
    ));
    // The dictionary budget covers the retained content.
    let error = with_limits(FramedDecodeLimits::default().with_max_dictionary_bytes(Some(1500)))
        .decompress_with_dictionaries(&fixture, &fixture.bytes)
        .expect_err("budget");
    assert!(matches!(
        error,
        FramedDecodeError::LimitExceeded {
            kind: FramedLimitKind::DictionaryBytes,
            limit: 1500
        }
    ));
}

#[test]
fn references_must_address_earlier_chunks_of_the_right_kind() {
    let body = frame(b"body");
    let reference = |flag: u8, offset: u64| {
        let mut builder = Builder::new(4);
        let hidden = builder.hidden(b"prefix");
        builder.chunk(&[0], &[], None);
        let mut fields = vec![2, 3, 4, 1, flag];
        fields.extend(var(if offset == 0 { hidden } else { offset }));
        fields.extend([4]);
        fields.extend(var(body.len() as u64));
        builder.chunk(&fields, &[], Some((&body, 4)));
        builder.footer();
        builder.finish()
    };
    // Prefix resource at the right offset decodes (the frame needs no history).
    let output = decoder().decompress(&reference(0, 0)).expect("valid");
    assert_eq!(output.resources[1].data, b"body");
    for (flag, offset) in [(0, 9999), (1, 9999), (4, 9999), (0, 13 + 13 + 6 + 1)] {
        assert!(matches!(
            error(&mut decoder(), &reference(flag, offset)),
            FramedDecodeError::InvalidDictionaryReference
        ));
    }
    // The serialized resource is not a Zstandard dictionary.
    assert!(matches!(
        error(&mut decoder(), &reference(4, 0)),
        FramedDecodeError::Dictionary { .. }
    ));
}

#[test]
fn every_numeric_limit_reports_its_kind_and_ceiling() {
    let bytes = two_resources();
    let metadata = {
        let mut builder = Builder::new(4);
        builder.metadata(7, b"AA\x02xyBB\x01z");
        builder.resource(b"abc");
        builder.footer();
        builder.finish()
    };
    let cases = [
        (
            FramedDecodeLimits::default().with_max_input_bytes(Some(20)),
            FramedLimitKind::InputBytes,
            20,
            &bytes,
        ),
        (
            FramedDecodeLimits::default().with_max_output_bytes(Some(4)),
            FramedLimitKind::OutputBytes,
            4,
            &bytes,
        ),
        (
            FramedDecodeLimits::default().with_max_decoded_bytes(Some(4)),
            FramedLimitKind::DecodedBytes,
            4,
            &bytes,
        ),
        (
            FramedDecodeLimits::default().with_max_resource_bytes(Some(2)),
            FramedLimitKind::ResourceBytes,
            2,
            &bytes,
        ),
        (
            FramedDecodeLimits::default().with_max_resources(Some(1)),
            FramedLimitKind::Resources,
            1,
            &bytes,
        ),
        (
            FramedDecodeLimits::default().with_max_chunks(Some(1)),
            FramedLimitKind::Chunks,
            1,
            &bytes,
        ),
        (
            FramedDecodeLimits::default().with_max_framing_bytes(Some(8)),
            FramedLimitKind::FramingBytes,
            8,
            &bytes,
        ),
        (
            FramedDecodeLimits::default().with_max_workspace_bytes(Some(16)),
            FramedLimitKind::WorkspaceBytes,
            16,
            &bytes,
        ),
        (
            FramedDecodeLimits::default().with_max_metadata_bytes(Some(5)),
            FramedLimitKind::MetadataBytes,
            5,
            &metadata,
        ),
        (
            FramedDecodeLimits::default().with_max_metadata_fields(Some(1)),
            FramedLimitKind::MetadataFields,
            1,
            &metadata,
        ),
    ];
    for (limits, kind, limit, input) in cases {
        let error = error(&mut with_limits(limits), input);
        assert!(
            matches!(error, FramedDecodeError::LimitExceeded { kind: k, limit: l } if k == kind && l == limit),
            "{kind:?}: {error:?}"
        );
    }
    // Exact boundaries pass; disabled ceilings never apply.
    let exact = FramedDecodeLimits::default()
        .with_max_input_bytes(Some(bytes.len() as u64))
        .with_max_output_bytes(Some(5))
        .with_max_decoded_bytes(Some(5))
        .with_max_resource_bytes(Some(3))
        .with_max_resources(Some(2));
    with_limits(exact).decompress(&bytes).expect("exact");
    let none = FramedDecodeLimits::default()
        .with_max_workspace_bytes(None)
        .with_max_framing_bytes(None)
        .with_max_dictionary_bytes(None)
        .with_max_chunks(None)
        .with_max_resources(None)
        .with_max_metadata_bytes(None)
        .with_max_metadata_fields(None);
    assert_eq!(none.max_chunks(), None);
    with_limits(none).decompress(&metadata).expect("unlimited");
    // A raw input's output ceiling applies to the codec.
    let raw = frame(&corpus::text(1000, 9));
    let error = FramedDecompressor::new(
        FramedDecodeConfig::default()
            .with_input_mode(InputMode::Auto)
            .with_limits(FramedDecodeLimits::default().with_max_output_bytes(Some(10))),
    )
    .expect("decoder")
    .decompress(&raw)
    .expect_err("limit");
    assert!(matches!(
        error,
        FramedDecodeError::LimitExceeded {
            kind: FramedLimitKind::OutputBytes,
            limit: 10
        }
    ));
    let error = FramedDecompressor::new(
        FramedDecodeConfig::default()
            .with_input_mode(InputMode::Auto)
            .with_limits(FramedDecodeLimits::default().with_max_input_bytes(Some(10))),
    )
    .expect("decoder")
    .decompress(&raw)
    .expect_err("limit");
    assert!(matches!(
        error,
        FramedDecodeError::LimitExceeded {
            kind: FramedLimitKind::InputBytes,
            limit: 10
        }
    ));
}

#[test]
fn a_frame_window_above_the_configured_log_is_a_codec_error() {
    let payload = corpus::text(5000, 10);
    let body = oracle::compress2(
        &payload,
        &oracle::Params::level(3)
            .with(zstd_ffi::ZSTD_c_windowLog, 20)
            .with(zstd_ffi::ZSTD_c_contentSizeFlag, 0),
    )
    .expect("reference");
    let mut builder = Builder::new(0);
    builder.chunk(
        &visible_fields(2, payload.len(), 0, &body),
        &[],
        Some((&body, payload.len() as u32)),
    );
    let bytes = builder.finish();
    decoder().decompress(&bytes).expect("default window");
    let small = FramedDecompressor::new(
        FramedDecodeConfig::default()
            .with_max_window_log(10)
            .expect("log"),
    )
    .expect("decoder")
    .decompress(&bytes)
    .expect_err("window");
    assert!(matches!(
        small,
        FramedDecodeError::Decode {
            source: DecodeError::WindowLimitExceeded { .. },
            ..
        }
    ));
}

#[test]
fn checksums_are_recorded_without_authentication() {
    let id = DictionaryId([9; 32]);
    let payload = corpus::text(5000, 11);
    let items = [FramedItem::Resource(FramedResource {
        data: &payload,
        options: ResourceOptions {
            hidden: false,
            id: Some(id),
        },
        stream: Default::default(),
        encoding: ResourceEncoding::Zstd,
    })];
    let bytes = encode_with(
        FramingConfig {
            chunk_bytes: 1024,
            ..Default::default()
        },
        &items,
    );
    let output = decoder().decompress(&bytes).expect("decode");
    assert_eq!(output.resources[0].checksum, Some(id));
    let OutputStructure::Framed { layout, .. } = output.structure else {
        panic!("framed");
    };
    let checksummed: Vec<_> = layout.chunks.iter().filter_map(|c| c.checksum).collect();
    assert_eq!(checksummed, [id]);
}

#[test]
fn backends_decode_identically() {
    let fixture = dictionaries();
    let mut expected = FramedDecompressor::builder(FramedDecodeConfig::default())
        .build()
        .expect("decoder");
    let expected = expected
        .decompress_with_dictionaries(&fixture, &fixture.bytes)
        .expect("decode");
    for backend in Backend::available() {
        let mut decoder = FramedDecompressor::builder(FramedDecodeConfig::default())
            .with_backend(backend)
            .build()
            .expect("decoder");
        assert_eq!(
            decoder
                .decompress_with_dictionaries(&fixture, &fixture.bytes)
                .expect("decode"),
            expected,
            "{}",
            backend.name()
        );
    }
}

#[test]
fn random_containers_match_the_reference_decoder() {
    for seed in 0..24u64 {
        let mut rng = corpus::Rng::new(seed);
        let count = 1 + (rng.next_u64() % 6) as usize;
        let payloads: Vec<Vec<u8>> = (0..count)
            .map(|i| match rng.next_u64() % 4 {
                0 => corpus::text((rng.next_u64() % 20_000) as usize, seed * 31 + i as u64),
                1 => corpus::binary((rng.next_u64() % 5_000) as usize, seed * 31 + i as u64),
                2 => corpus::random((rng.next_u64() % 3_000) as usize, seed * 31 + i as u64),
                _ => Vec::new(),
            })
            .collect();
        let hidden_mask = rng.next_u64();
        let items: Vec<FramedItem<'_>> = payloads
            .iter()
            .enumerate()
            .map(|(i, data)| {
                FramedItem::Resource(FramedResource {
                    data,
                    options: ResourceOptions {
                        hidden: hidden_mask >> i & 1 == 1,
                        ..Default::default()
                    },
                    stream: Default::default(),
                    encoding: if hidden_mask >> (i + 8) & 1 == 1 {
                        ResourceEncoding::Uncompressed
                    } else {
                        ResourceEncoding::Zstd
                    },
                })
            })
            .collect();
        let chunk_bytes = 512 + (rng.next_u64() % 8192) as usize;
        let bytes = encode_with(
            FramingConfig {
                chunk_bytes,
                ..Default::default()
            },
            &items,
        );
        let output = decoder().decompress(&bytes).expect("decode");
        let mut visible = Vec::new();
        for ((resource, payload), i) in output.resources.iter().zip(&payloads).zip(0..) {
            assert_eq!(&resource.data, payload, "seed {seed} resource {i}");
            if !resource.hidden {
                visible.extend_from_slice(payload);
            }
        }
        assert_eq!(
            oracle::decompress(&bytes, visible.len()).expect("reference"),
            visible,
            "seed {seed}"
        );
    }
}

#[test]
fn the_owner_keeps_policies_across_reuse_trim_and_recovery() {
    let bytes = two_resources();
    let config = FramedDecodeConfig::default().with_input_mode(InputMode::Auto);
    let mut decoder = FramedDecompressor::builder(config)
        .with_retention(RetentionPolicy::Aggressive)
        .build()
        .expect("decoder");
    assert_eq!(decoder.config(), &config);
    assert_eq!(decoder.retention(), RetentionPolicy::Aggressive);
    assert_eq!(decoder.retained_bytes(), 0);
    decoder.decompress(&bytes).expect("first");
    let retained = decoder.retained_bytes();
    assert!(retained > 0);
    decoder.decompress(&bytes).expect("second");
    decoder.trim(RetentionPolicy::Bounded {
        max_bytes: usize::MAX,
    });
    assert_eq!(decoder.retained_bytes(), retained);
    decoder.trim(RetentionPolicy::Bounded { max_bytes: 0 });
    assert_eq!(decoder.retained_bytes(), 0);
    let fork = decoder.fork_empty();
    assert_eq!(fork.config(), decoder.config());
    assert_eq!(fork.retention(), decoder.retention());
    // A forgotten session protects the owner until recovery or reconfiguration.
    std::mem::forget(decoder.start(Default::default()).expect("start"));
    assert!(matches!(
        decoder.decompress(&bytes),
        Err(FramedDecodeError::AbandonedSession)
    ));
    decoder.recover();
    decoder.decompress(&bytes).expect("recovered");
    std::mem::forget(decoder.start(Default::default()).expect("start"));
    decoder
        .reconfigure(FramedDecodeConfig::default())
        .expect("reconfigure");
    assert_eq!(decoder.config().input_mode(), InputMode::FramedOnly);
    decoder.decompress(&bytes).expect("reconfigured");
    // Release-all keeps nothing between operations.
    let mut release = FramedDecompressor::builder(FramedDecodeConfig::default())
        .with_retention(RetentionPolicy::ReleaseAll)
        .build()
        .expect("decoder");
    release.decompress(&bytes).expect("decode");
    assert_eq!(release.retained_bytes(), 0);
    // Over-budget retained storage is released at the next start.
    let mut bounded = with_limits(FramedDecodeLimits::default());
    bounded.decompress(&dictionaries().bytes).ok();
    bounded
        .reconfigure(
            FramedDecodeConfig::default()
                .with_limits(FramedDecodeLimits::default().with_max_workspace_bytes(Some(4096))),
        )
        .expect("reconfigure");
    bounded.decompress(&bytes).expect("fits after release");
}

#[test]
fn configuration_accessors_round_trip() {
    let limits = FramedDecodeLimits::default()
        .with_max_input_bytes(Some(1))
        .with_max_output_bytes(Some(2))
        .with_max_decoded_bytes(Some(3))
        .with_max_resource_bytes(Some(4))
        .with_max_workspace_bytes(Some(5))
        .with_max_framing_bytes(Some(6))
        .with_max_dictionary_bytes(Some(7))
        .with_max_metadata_bytes(Some(8))
        .with_max_metadata_fields(Some(9))
        .with_max_resources(Some(10))
        .with_max_chunks(Some(11));
    assert_eq!(
        [
            limits.max_input_bytes(),
            limits.max_output_bytes(),
            limits.max_decoded_bytes(),
            limits.max_resource_bytes(),
            limits.max_workspace_bytes().map(|n| n as u64),
            limits.max_framing_bytes().map(|n| n as u64),
            limits.max_dictionary_bytes().map(|n| n as u64),
            limits.max_metadata_bytes(),
            limits.max_metadata_fields(),
            limits.max_resources(),
            limits.max_chunks(),
        ],
        (1..=11).map(Some).collect::<Vec<_>>()[..]
    );
    let config = FramedDecodeConfig::new()
        .with_limits(limits)
        .with_internal_dictionaries(InternalDictionaryPolicy::Reject)
        .with_max_window_log(20)
        .expect("log");
    assert_eq!(config.limits(), limits);
    assert_eq!(
        config.internal_dictionaries(),
        InternalDictionaryPolicy::Reject
    );
    assert_eq!(config.max_window_log(), 20);
    assert!(matches!(
        FramedDecodeConfig::new().with_max_window_log(32),
        Err(mzstd::DecodeConfigError::WindowLog { log: 32 })
    ));
    assert_eq!(FramedDecodeConfig::new(), FramedDecodeConfig::default());
    assert_eq!(
        FramedDecodeStreamConfig::from(OutputSize::from(3)).output_size(),
        OutputSize::Exact(3)
    );
}

#[test]
fn failed_sessions_stay_failed_and_report_exact_progress() {
    let bytes = two_resources();
    let mut decoder = decoder();
    let mut session = decoder.start(Default::default()).expect("start");
    let mut bad = bytes.clone();
    bad[13] ^= 0xff;
    let mut position = 0;
    let failure = loop {
        match session.process(&bad[position..], &mut [0; 8], DecodeOperation::Finish) {
            Ok(progress) => position += progress.consumed,
            Err(failure) => break failure,
        }
    };
    assert!(matches!(failure.error, FramedDecodeError::InvalidChunk));
    assert_eq!(position + failure.consumed, 17);
    assert!(failure.last_output.is_none());
    let again = session
        .process(&bad[17..], &mut [0; 8], DecodeOperation::Finish)
        .expect_err("failed");
    assert!(matches!(again.error, FramedDecodeError::InvalidState));
    assert!(format!("{session:?}").contains("FramedDecoderSession"));
    drop(session);
    decoder.decompress(&bytes).expect("reusable after drop");
}

#[cfg(not(feature = "no_std"))]
mod std_reader {
    use super::*;
    use std::io::{self, BufReader, Read};

    /// A source returning at most `step` bytes, interrupting before every
    /// read and blocking once before every other read.
    struct Flaky {
        data: Vec<u8>,
        position: usize,
        step: usize,
        calls: usize,
    }

    impl Read for Flaky {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            match self.calls % 4 {
                1 => return Err(io::ErrorKind::Interrupted.into()),
                3 => return Err(io::ErrorKind::WouldBlock.into()),
                _ => {}
            }
            let n = buffer
                .len()
                .min(self.step)
                .min(self.data.len() - self.position);
            buffer[..n].copy_from_slice(&self.data[self.position..self.position + n]);
            self.position += n;
            Ok(n)
        }
    }

    #[test]
    fn the_reader_retries_interrupts_and_survives_would_block() {
        let fixture = dictionaries();
        let source = BufReader::with_capacity(
            64,
            Flaky {
                data: fixture.bytes.clone(),
                position: 0,
                step: 50,
                calls: 0,
            },
        );
        let mut decoder = decoder();
        let mut reader = decoder
            .framed_reader_with_dictionaries(&fixture, source, Default::default())
            .expect("reader");
        let mut payload = Vec::new();
        let mut blocked = 0;
        loop {
            match reader.next_event() {
                Ok(Some(FramedEvent::ResourceData(data))) => payload.extend_from_slice(data.bytes),
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(FramedReadError::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {
                    blocked += 1;
                }
                Err(error) => panic!("{error}"),
            }
        }
        assert!(blocked > 0);
        assert!(reader.is_finished());
        assert_eq!(payload, fixture.payloads.concat());
        assert!(reader.partial_output().is_none());
        assert_eq!(reader.get_ref().get_ref().position, fixture.bytes.len());
        reader.get_mut();
        assert!(reader.into_inner().buffer().is_empty());
    }

    #[test]
    fn the_reader_leaves_a_suffix_and_exposes_a_failed_payload() {
        let mut bytes = two_resources();
        bytes.extend(b"suffix");
        let mut decoder = decoder();
        let mut reader = decoder
            .framed_reader(&bytes[..], Default::default())
            .expect("reader");
        while reader.next_event().expect("event").is_some() {}
        assert_eq!(reader.into_inner(), b"suffix");
        // A corrupt checksum fails after the frame's payload was delivered.
        let payload = b"payload before a bad checksum";
        let mut body = oracle::compress2(
            payload,
            &oracle::Params::level(3).with(zstd_ffi::ZSTD_c_checksumFlag, 1),
        )
        .expect("reference");
        *body.last_mut().expect("checksum") ^= 1;
        let mut builder = Builder::new(0);
        builder.chunk(
            &visible_fields(2, payload.len(), 0, &body),
            &[],
            Some((&body, payload.len() as u32)),
        );
        let bad = builder.finish();
        let mut reader = decoder
            .framed_reader(&bad[..], Default::default())
            .expect("reader");
        let error = loop {
            match reader.next_event() {
                Ok(Some(_)) => {}
                Ok(None) => panic!("finished"),
                Err(error) => break error,
            }
        };
        let FramedReadError::Decode(failure) = error else {
            panic!("{error}");
        };
        assert!(matches!(failure.error, FramedDecodeError::Decode { .. }));
        let partial = reader.partial_output().expect("partial");
        assert_eq!(partial.bytes, payload);
        assert_eq!(partial.position.resource, ResourceIndex(0));
        drop(reader);
        let failure = decoder
            .decompress_to_slice(&bad, &mut [0; 64])
            .expect_err("bad");
        assert_eq!(failure.produced, payload.len());
        assert_eq!(
            failure.last_output.expect("fragment").range,
            0..payload.len()
        );
    }
}

#[test]
fn content_past_the_dictionary_budget_is_simply_not_retained() {
    let payload = corpus::text(3 << 20, 41);
    let bytes = encode(&[visible(&payload), visible(b"tail")]);
    let limits = FramedDecodeLimits::default().with_max_dictionary_bytes(Some(1 << 20));
    let output = with_limits(limits).decompress(&bytes).expect("decode");
    assert_eq!(output.resources[0].data, payload);
    assert_eq!(output.resources[1].data, b"tail");
}

#[test]
fn a_reference_to_content_the_cache_dropped_reports_the_dictionary_budget() {
    use mzstd::dictionary::{DictionaryBuilder, DictionaryContent};
    let prefix = corpus::text(4096, 42);
    let dictionary = DictionaryBuilder::new(&prefix)
        .with_content(DictionaryContent::Raw)
        .build()
        .expect("dictionary");
    let references = [DictionaryReference::PrefixResource(13)];
    let chunk_references = [DictionaryReference::PrefixChunk(13)];
    for references in [&references[..], &chunk_references[..]] {
        let bytes = encode(&[
            visible(&prefix),
            FramedItem::Resource(FramedResource {
                data: &prefix,
                options: Default::default(),
                stream: Default::default(),
                encoding: ResourceEncoding::Shared {
                    dictionary: &dictionary,
                    references,
                },
            }),
        ]);
        let output = decoder().decompress(&bytes).expect("decode");
        assert_eq!(output.resources[1].data, prefix);
        let limits = FramedDecodeLimits::default().with_max_dictionary_bytes(Some(1024));
        assert!(matches!(
            with_limits(limits).decompress(&bytes),
            Err(FramedDecodeError::LimitExceeded {
                kind: FramedLimitKind::DictionaryBytes,
                limit: 1024
            })
        ));
    }
}

#[test]
fn the_seek_table_closes_the_container_before_a_progress_free_finish() {
    let bytes = encode(&[visible(b"abc")]);
    let mut decoder = decoder();
    let mut session = decoder.start(Default::default()).expect("start");
    let mut remaining = &bytes[..];
    let mut output = [0; 64];
    let mut seek = None;
    loop {
        let progress = session
            .process(remaining, &mut output, DecodeOperation::Finish)
            .expect("decode");
        remaining = &remaining[progress.consumed..];
        match progress.status {
            FramedDecoderStatus::Event(FramedEvent::SeekTable(info)) => {
                assert!(remaining.is_empty());
                seek = Some(info);
            }
            FramedDecoderStatus::Finished => {
                assert_eq!((progress.consumed, progress.produced), (0, 0));
                break;
            }
            _ => assert!(seek.is_none(), "no event follows the seek table"),
        }
    }
    let seek = seek.expect("seek table event");
    // Container header, chunk header, body, directory and footer.
    assert_eq!(seek.frames, 5);
    assert_eq!(seek.offset.0 + 8 + 5 * 8 + 9, bytes.len() as u64);
    drop(session);
    let OutputStructure::Framed { layout, .. } =
        decoder.decompress(&bytes).expect("decode").structure
    else {
        panic!("framed");
    };
    assert_eq!(layout.seek_table, Some(seek));
}
