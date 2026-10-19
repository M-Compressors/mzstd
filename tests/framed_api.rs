//! Small public accessors of the framed decoder, outside the larger suites.
#![cfg(all(
    feature = "framed",
    feature = "compression",
    feature = "decompression",
    not(feature = "no_std")
))]

use std::io::Cursor;
use std::sync::Arc;

use mzstd::framing::*;

struct Prefix;

impl DictionaryResolver for Prefix {
    fn resolve(&self, request: ExternalDictionaryRequest) -> Option<&[u8]> {
        (request.kind == ExternalDictionaryKind::Prefix).then_some(b"prefix")
    }
}

fn container() -> Vec<u8> {
    let time = 1_700_000_000_000_000_i64.to_le_bytes();
    let fields = [MetadataField {
        code: *b"mt",
        value: &time,
    }];
    let items = [
        FramedItem::Metadata {
            kind: MetadataKind::Resource,
            fields: &fields,
            options: MetadataOptions::default(),
        },
        FramedItem::Resource(FramedResource::from(&b"payload"[..])),
    ];
    FramedCompressor::new(FramedEncodeConfig::default())
        .expect("encoder")
        .compress(items.as_slice().into())
        .expect("encode")
}

#[test]
fn metadata_timestamps_decode_as_signed_microseconds() {
    let output = FramedDecompressor::new(FramedDecodeConfig::default())
        .expect("decoder")
        .decompress(&container())
        .expect("decode");
    let metadata = output.resources[0].metadata.as_ref().expect("metadata");
    assert_eq!(metadata.timestamp(), Some(1_700_000_000_000_000));
    assert_eq!(metadata.name(), None);
}

#[test]
fn an_exact_output_size_is_a_stream_contract() {
    let stream = FramedDecodeStreamConfig::default().with_output_size(OutputSize::Exact(7));
    assert_eq!(stream.output_size(), OutputSize::Exact(7));
    let mut decoder = FramedDecompressor::new(FramedDecodeConfig::default()).expect("decoder");
    let session = decoder.start(stream).expect("start");
    assert_eq!(session.total_out(), 0);
}

#[test]
fn failures_expose_their_error_and_location() {
    let mut decoder = FramedDecompressor::new(FramedDecodeConfig::default()).expect("decoder");
    let mut bytes = container();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    let failure = decoder
        .decompress_to_slice(&bytes, &mut [0; 64])
        .unwrap_err();
    assert!(matches!(
        failure.into_error(),
        FramedDecodeError::InvalidSeekTable
    ));
    let mut missing = FramedDecompressor::new(FramedDecodeConfig::default()).expect("decoder");
    let location = FramedDecodeLocation::default();
    assert_eq!((location.chunk(), location.resource()), (None, None));
    let error = missing.decompress(&[0x5b, 0x2a, 0x4d]).unwrap_err();
    assert!(matches!(error, FramedDecodeError::UnexpectedEndOfInput));
}

#[test]
fn seek_resources_report_their_index() {
    let bytes = container();
    let mut decoder = FramedDecompressor::new(FramedDecodeConfig::default()).expect("decoder");
    let reader = decoder
        .framed_seek_reader(Cursor::new(bytes.as_slice()))
        .expect("index");
    assert_eq!(reader.resources()[0].index(), ResourceIndex(0));
}

/// Resolves through `resolver` used by value, as an owned session holds it.
fn resolved<R: DictionaryResolver>(
    resolver: R,
    request: ExternalDictionaryRequest,
) -> Option<Vec<u8>> {
    resolver.resolve(request).map(<[u8]>::to_vec)
}

#[test]
fn boxed_shared_and_borrowed_resolvers_resolve_alike() {
    let request = ExternalDictionaryRequest {
        id: DictionaryId([7; 32]),
        kind: ExternalDictionaryKind::Prefix,
    };
    let boxed: Box<dyn DictionaryResolver> = Box::new(Prefix);
    let shared = Arc::new(Prefix);
    assert_eq!(boxed.resolve(request), Some(&b"prefix"[..]));
    assert_eq!(shared.resolve(request), Some(&b"prefix"[..]));
    assert_eq!(resolved(&Prefix, request), Some(b"prefix".to_vec()));
    let reference = DictionaryResolverRef::from(&Prefix);
    assert_eq!(format!("{reference:?}"), "DictionaryResolverRef { .. }");
}
