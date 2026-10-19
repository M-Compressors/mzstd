//! Framed containers against zstd's own decoders: `ZSTD_decompress` sees
//! the concatenated visible resources, and the contrib seekable decoder
//! reads every range of them through the seek table.
#![cfg(all(feature = "framed", feature = "compression"))]

use mzstd::framing::*;
use mzstd::{EncoderConfig, Level};
use zstd_ffi::{corpus, oracle};

fn decompress_all(bytes: &[u8]) -> Vec<u8> {
    let size = oracle::seekable_decompress(bytes, 0, 0)
        .expect("seek table")
        .decompressed_size;
    let mut output = vec![0u8; size as usize];
    let mut decoder = oracle::Decoder::new();
    let written = decoder.decompress_into(bytes, &mut output);
    output.truncate(written);
    output
}

#[test]
fn visible_resources_decode_with_the_reference_and_its_seekable_reader() {
    let text = corpus::text(300_000, 1);
    let binary = corpus::binary(70_000, 2);
    let fields = [MetadataField {
        code: *b"id",
        value: b"a.txt",
    }];
    let items = [
        FramedItem::Metadata {
            kind: MetadataKind::Resource,
            fields: &fields,
            options: MetadataOptions {
                encoding: MetadataEncoding::Zstd,
                ..Default::default()
            },
        },
        FramedItem::Resource(FramedResource::from(text.as_slice())),
        FramedItem::Padding { bytes: 7 },
        FramedItem::Resource(FramedResource {
            data: b"hidden",
            options: ResourceOptions {
                hidden: true,
                ..Default::default()
            },
            stream: Default::default(),
            encoding: ResourceEncoding::Zstd,
        }),
        FramedItem::Resource(FramedResource {
            data: &binary,
            options: Default::default(),
            stream: Default::default(),
            encoding: ResourceEncoding::Uncompressed,
        }),
        FramedItem::Resource(FramedResource::from(&b""[..])),
    ];
    for level in [1, 3, 19] {
        let config = FramedEncodeConfig::default().with_encoder_config(
            EncoderConfig::default().with_level(Level::try_from(level).unwrap()),
        );
        let mut encoder = FramedCompressor::new(config).unwrap();
        let bytes = encoder.compress(items.as_slice().into()).unwrap();
        let visible = [text.as_slice(), binary.as_slice()].concat();
        assert_eq!(decompress_all(&bytes), visible);
        let seekable = oracle::seekable_decompress(&bytes, 0, 0).unwrap();
        assert_eq!(seekable.decompressed_size, visible.len() as u64);
        for (offset, len) in [
            (0, 10),
            (65_530, 20),
            (299_990, 30),
            (300_000, 70_000),
            (369_000, 5_000),
        ] {
            let range = oracle::seekable_decompress(&bytes, offset, len)
                .unwrap()
                .data;
            let end = (offset as usize + len).min(visible.len());
            assert_eq!(
                range,
                &visible[offset as usize..end],
                "level {level} offset {offset}"
            );
        }
    }
}

#[test]
fn the_framed_decoder_reads_back_every_resource_and_metadata() {
    let text = corpus::text(300_000, 3);
    let fields = [MetadataField {
        code: *b"id",
        value: b"a.txt",
    }];
    let items = [
        FramedItem::Metadata {
            kind: MetadataKind::Resource,
            fields: &fields,
            options: MetadataOptions {
                encoding: MetadataEncoding::Zstd,
                ..Default::default()
            },
        },
        FramedItem::Resource(FramedResource::from(text.as_slice())),
        FramedItem::Padding { bytes: 3 },
        FramedItem::Resource(FramedResource {
            data: b"hidden",
            options: ResourceOptions {
                hidden: true,
                ..Default::default()
            },
            stream: Default::default(),
            encoding: ResourceEncoding::Zstd,
        }),
        FramedItem::Resource(FramedResource {
            data: b"raw",
            options: ResourceOptions {
                hidden: true,
                ..Default::default()
            },
            stream: Default::default(),
            encoding: ResourceEncoding::Uncompressed,
        }),
        FramedItem::Resource(FramedResource::from(&b""[..])),
    ];
    let bytes = FramedCompressor::new(Default::default())
        .unwrap()
        .compress(items.as_slice().into())
        .unwrap();
    let output = FramedDecompressor::new(Default::default())
        .unwrap()
        .decompress(&bytes)
        .unwrap();
    assert_eq!(output.resources.len(), 4);
    assert_eq!(output.resources[0].data, text);
    assert_eq!(
        output.resources[0].metadata.as_ref().unwrap().name(),
        Some("a.txt")
    );
    assert_eq!(output.resources[1].data, b"hidden");
    assert!(output.resources[1].hidden);
    assert_eq!(output.resources[2].data, b"raw");
    assert!(output.resources[3].data.is_empty());
}
