//! The framed seek reader: indexing through the seek table, footer and
//! directory, random-access resource reads with dictionary dependencies,
//! and every structural failure it detects.
#![cfg(all(feature = "framed", feature = "compression", feature = "decompression"))]
#![cfg(not(feature = "no_std"))]

mod framed_decoder_support;

use std::io::{self, Cursor, Read, Seek, SeekFrom};

use framed_decoder_support::*;
use mzstd::framing::*;
use zstd_ffi::{corpus, oracle};

fn decoder() -> FramedDecompressor {
    FramedDecompressor::new(FramedDecodeConfig::default()).expect("decoder")
}

fn with_limits(limits: FramedDecodeLimits) -> FramedDecompressor {
    FramedDecompressor::new(FramedDecodeConfig::default().with_limits(limits)).expect("decoder")
}

fn payload<R: Read + Seek>(reader: &mut FramedSeekReader<'_, '_, R>, i: u64) -> Vec<u8> {
    let mut out = Vec::new();
    reader
        .resource(ResourceIndex(i))
        .expect("resource")
        .read_to_end(&mut out)
        .expect("read");
    out
}

fn open_error(decoder: &mut FramedDecompressor, bytes: &[u8]) -> FramedSeekError {
    decoder
        .framed_seek_reader(Cursor::new(bytes))
        .expect_err("must fail")
}

/// The framing error carried by a failed `Read`.
fn seek_error(error: &io::Error) -> &FramedSeekError {
    error
        .get_ref()
        .and_then(|e| e.downcast_ref::<FramedSeekError>())
        .expect("a seek error")
}

fn structured() -> (Vec<u8>, Vec<Vec<u8>>) {
    let big = corpus::text(20_000, 21);
    let id = [MetadataField {
        code: *b"id",
        value: b"big.txt",
    }];
    let tail = [MetadataField {
        code: *b"TA",
        value: b"tail",
    }];
    let items = [
        FramedItem::Resource(FramedResource::from(&b"first"[..])),
        FramedItem::Padding { bytes: 4 },
        FramedItem::Metadata {
            kind: MetadataKind::Resource,
            fields: &id,
            options: MetadataOptions {
                encoding: MetadataEncoding::Zstd,
                ..Default::default()
            },
        },
        FramedItem::Resource(FramedResource::from(big.as_slice())),
        FramedItem::Metadata {
            kind: MetadataKind::Footer,
            fields: &tail,
            options: Default::default(),
        },
        FramedItem::Resource(FramedResource {
            data: b"hidden",
            options: ResourceOptions {
                hidden: true,
                ..Default::default()
            },
            stream: Default::default(),
            encoding: ResourceEncoding::Uncompressed,
        }),
        FramedItem::Resource(FramedResource::from(&b""[..])),
    ];
    let bytes = FramedCompressor::new(FramedEncodeConfig::default().with_framing_config(
        FramingConfig {
            chunk_bytes: 4096,
            repeat_metadata: true,
            ..Default::default()
        },
    ))
    .expect("encoder")
    .compress(items.as_slice().into())
    .expect("encode");
    (
        bytes,
        vec![b"first".to_vec(), big, b"hidden".to_vec(), Vec::new()],
    )
}

#[test]
fn resources_read_in_any_order_repeatedly_and_after_an_early_drop() {
    let (bytes, payloads) = structured();
    let mut decoder = decoder();
    let mut reader = decoder
        .framed_seek_reader(Cursor::new(bytes.clone()))
        .expect("open");
    assert!(reader.header().has_footer());
    assert!(reader.footer().directory.is_some());
    assert_eq!(reader.footer().file_size, Some(bytes.len() as u64));
    assert_eq!(reader.resources().len(), 4);
    for (info, payload) in reader.resources().iter().zip(&payloads) {
        assert_eq!(info.decoded_size(), Some(payload.len() as u64));
        assert_eq!(info.checksum(), None);
    }
    assert!(
        reader
            .resource_info(ResourceIndex(2))
            .expect("info")
            .hidden()
    );
    assert!(reader.resource_info(ResourceIndex(4)).is_none());
    for i in [3, 1, 0, 2, 1] {
        assert_eq!(
            payload(&mut reader, i),
            payloads[i as usize],
            "resource {i}"
        );
    }
    {
        let mut resource = reader.resource(ResourceIndex(1)).expect("resource");
        assert_eq!(resource.index(), ResourceIndex(1));
        assert_eq!(resource.info().decoded_size(), Some(20_000));
        let mut prefix = [0; 10];
        resource.read_exact(&mut prefix).expect("prefix");
        assert_eq!(resource.total_out(), 10);
        assert_eq!(resource.read(&mut []).expect("empty"), 0);
    }
    assert_eq!(payload(&mut reader, 1), payloads[1]);
    assert!(matches!(
        reader.resource(ResourceIndex(9)),
        Err(FramedSeekError::ResourceNotFound)
    ));
    assert!(matches!(
        reader.resource_metadata(ResourceIndex(9)),
        Err(FramedSeekError::ResourceNotFound)
    ));
    assert_eq!(reader.get_ref().get_ref().len(), bytes.len());
    reader.get_mut();
    let source = reader.into_inner();
    assert_eq!(source.into_inner(), bytes);
    // The owner opens another container after `into_inner`.
    let mut again = decoder
        .framed_seek_reader(Cursor::new(&bytes))
        .expect("reopen");
    assert_eq!(payload(&mut again, 0), b"first");
}

#[test]
fn metadata_is_decoded_lazily_cached_and_matches_the_sequential_decoder() {
    let (bytes, _) = structured();
    let sequential = decoder().decompress(&bytes).expect("sequential");
    let mut decoder = decoder();
    let mut reader = decoder
        .framed_seek_reader(Cursor::new(&bytes))
        .expect("open");
    assert!(
        reader
            .resource_metadata(ResourceIndex(0))
            .expect("none")
            .is_none()
    );
    assert!(
        reader
            .resource_footer_metadata(ResourceIndex(0))
            .expect("none")
            .is_none()
    );
    let metadata = reader
        .resource_metadata(ResourceIndex(1))
        .expect("metadata")
        .expect("present");
    assert_eq!(metadata.name(), Some("big.txt"));
    assert_eq!(Some(metadata), sequential.resources[1].metadata.as_ref());
    let cached = reader
        .resource_metadata(ResourceIndex(1))
        .expect("cached")
        .expect("present");
    assert_eq!(cached.as_bytes(), b"id\x07big.txt");
    let footer = reader
        .resource_footer_metadata(ResourceIndex(1))
        .expect("footer")
        .expect("present");
    assert_eq!(footer.as_bytes(), b"TA\x04tail");
    assert_eq!(
        Some(footer),
        sequential.resources[1].footer_metadata.as_ref()
    );
}

#[test]
fn seek_reads_agree_with_the_contrib_seekable_decoder() {
    let (bytes, payloads) = structured();
    let visible: Vec<u8> = [payloads[0].as_slice(), &payloads[1]].concat();
    let reference = oracle::seekable_decompress(&bytes, 5, 20_000).expect("seekable");
    assert_eq!(reference.data, &visible[5..]);
    let mut decoder = decoder();
    let mut reader = decoder
        .framed_seek_reader(Cursor::new(&bytes))
        .expect("open");
    assert_eq!(payload(&mut reader, 1), &reference.data[..]);
}

#[test]
fn dependencies_are_decoded_but_only_the_target_is_delivered() {
    let fixture = dictionaries();
    let mut decoder = decoder();
    let mut reader = decoder
        .framed_seek_reader_with_dictionaries(&fixture, Cursor::new(&fixture.bytes))
        .expect("open");
    for i in (0..fixture.payloads.len()).rev() {
        assert_eq!(
            payload(&mut reader, i as u64),
            fixture.payloads[i],
            "resource {i}"
        );
    }
    // Reading a dependent resource never retains its own payload.
    let mut small =
        with_limits(FramedDecodeLimits::default().with_max_dictionary_bytes(Some(4096)));
    let mut reader = small
        .framed_seek_reader_with_dictionaries(&fixture, Cursor::new(&fixture.bytes))
        .expect("open");
    assert_eq!(payload(&mut reader, 2), fixture.payloads[2]);
    assert_eq!(payload(&mut reader, 5), fixture.payloads[5]);
}

#[test]
fn dictionary_failures_surface_typed_and_leave_the_reader_reusable() {
    let fixture = dictionaries();
    let mut decoder = decoder();
    let mut reader = decoder
        .framed_seek_reader(Cursor::new(&fixture.bytes))
        .expect("open");
    let mut out = Vec::new();
    let error = reader
        .resource(ResourceIndex(5))
        .expect("plan")
        .read_to_end(&mut out)
        .expect_err("missing");
    assert!(matches!(
        seek_error(&error),
        FramedSeekError::Decode(FramedDecodeError::MissingDictionary { .. })
    ));
    // A failed reader stays failed.
    {
        let mut resource = reader.resource(ResourceIndex(7)).expect("plan");
        resource
            .read_to_end(&mut out)
            .expect_err("serialized external");
        let again = resource.read(&mut [0; 4]).expect_err("failed");
        assert!(matches!(
            seek_error(&again),
            FramedSeekError::Decode(FramedDecodeError::InvalidState)
        ));
    }
    assert_eq!(payload(&mut reader, 2), fixture.payloads[2]);
    drop(reader);
    let mut reject = FramedDecompressor::new(
        FramedDecodeConfig::default().with_internal_dictionaries(InternalDictionaryPolicy::Reject),
    )
    .expect("decoder");
    let mut reader = reject
        .framed_seek_reader_with_dictionaries(&fixture, Cursor::new(&fixture.bytes))
        .expect("open");
    assert!(matches!(
        reader.resource(ResourceIndex(2)),
        Err(FramedSeekError::Decode(
            FramedDecodeError::InternalDictionaryReferencesDisabled
        ))
    ));
    assert_eq!(payload(&mut reader, 0), fixture.payloads[0]);
}

#[test]
fn a_changed_chunk_header_is_detected_before_its_payload() {
    let (mut bytes, _) = structured();
    let directory = decoder()
        .framed_seek_reader(Cursor::new(&bytes))
        .expect("open")
        .footer()
        .directory
        .expect("directory");
    assert!(directory.0 > 13);
    // The first chunk's type byte follows the container header and its
    // frame header; complete (2) becomes first partial (3).
    assert_eq!(bytes[21], 2);
    bytes[21] = 3;
    let mut decoder = decoder();
    let mut reader = decoder
        .framed_seek_reader(Cursor::new(&bytes))
        .expect("open");
    assert!(matches!(
        reader.resource(ResourceIndex(0)).map(|mut r| r.read(&mut [0; 4])),
        Ok(Err(ref error)) if matches!(seek_error(error), FramedSeekError::DirectoryMismatch)
    ));
    assert_eq!(payload(&mut reader, 3), b"");
}

#[test]
fn random_access_requires_the_full_profile_and_a_directory() {
    let mut d = decoder();
    assert!(matches!(
        open_error(&mut d, &single(b"single")),
        FramedSeekError::CentralDirectoryRequired
    ));
    let without = FramedCompressor::new(FramedEncodeConfig::default().with_framing_config(
        FramingConfig {
            central_directory: false,
            ..Default::default()
        },
    ))
    .expect("encoder")
    .compress(
        [FramedItem::Resource(FramedResource::from(&b"x"[..]))]
            .as_slice()
            .into(),
    )
    .expect("encode");
    assert!(matches!(
        open_error(&mut d, &without),
        FramedSeekError::CentralDirectoryRequired
    ));
}

#[test]
fn malformed_tables_footers_directories_and_gaps_are_distinct_errors() {
    let bytes = two_resources();
    let decode = |e: FramedSeekError| match e {
        FramedSeekError::Decode(e) => e,
        other => panic!("{other}"),
    };
    let mut d = decoder();
    let mut bad = bytes.clone();
    bad[0] ^= 1;
    assert!(matches!(
        decode(open_error(&mut d, &bad)),
        FramedDecodeError::InvalidSignature
    ));
    let mut bad = bytes.clone();
    bad[12] |= 2;
    assert!(matches!(
        decode(open_error(&mut d, &bad)),
        FramedDecodeError::UnsupportedVersion(2)
    ));
    for at in [1, 5, 9] {
        let mut bad = bytes.clone();
        let len = bad.len();
        bad[len - at] ^= 0x80;
        assert!(
            matches!(
                decode(open_error(&mut d, &bad)),
                FramedDecodeError::InvalidSeekTable
            ),
            "{at}"
        );
    }
    // An entry that no longer adds up to the table's offset.
    let mut bad = bytes.clone();
    let at = bad.len() - 9 - 8;
    bad[at] += 1;
    assert!(matches!(
        decode(open_error(&mut d, &bad)),
        FramedDecodeError::InvalidSeekTable
    ));
    // Footer fields.
    let mut builder = Builder::new(4);
    builder.resource(b"a");
    builder.directory(0);
    builder.footer_with(builder.directory, 99);
    assert!(matches!(
        decode(open_error(&mut d, &builder.finish())),
        FramedDecodeError::InvalidFooter
    ));
    let mut builder = Builder::new(4);
    builder.resource(b"a");
    builder.directory(0);
    builder.footer_with(14, 0);
    assert!(matches!(
        decode(open_error(&mut d, &builder.finish())),
        FramedDecodeError::InvalidDirectory
    ));
    // A directory missing a record leaves a content chunk in a gap.
    let mut builder = Builder::new(4);
    builder.resource(b"a");
    builder.resource(b"b");
    builder.records.remove(0);
    builder.directory(0);
    builder.footer();
    assert!(matches!(
        decode(open_error(&mut d, &builder.finish())),
        FramedDecodeError::InvalidDirectory
    ));
    // Truncation is an I/O error from the source.
    assert!(matches!(
        open_error(&mut d, &bytes[..10]),
        FramedSeekError::Io(_) | FramedSeekError::Decode(FramedDecodeError::InvalidSeekTable)
    ));
    // Padding gaps are counted against the chunk budget.
    let mut builder = Builder::new(4);
    builder.padding(1);
    builder.resource(b"a");
    builder.padding(0);
    builder.directory(0);
    builder.padding(2);
    builder.footer();
    let padded = builder.finish();
    let mut reader = d.framed_seek_reader(Cursor::new(&padded)).expect("padded");
    assert_eq!(payload(&mut reader, 0), b"a");
    drop(reader);
    let mut limited = with_limits(FramedDecodeLimits::default().with_max_chunks(Some(5)));
    assert!(matches!(
        decode(open_error(&mut limited, &padded)),
        FramedDecodeError::LimitExceeded {
            kind: FramedLimitKind::Chunks,
            limit: 5
        }
    ));
}

#[test]
fn opening_budgets_and_per_read_limits_apply() {
    let (bytes, payloads) = structured();
    let mut tiny = with_limits(FramedDecodeLimits::default().with_max_input_bytes(Some(64)));
    assert!(matches!(
        open_error(&mut tiny, &bytes),
        FramedSeekError::Decode(FramedDecodeError::LimitExceeded {
            kind: FramedLimitKind::InputBytes,
            ..
        })
    ));
    let mut framing = with_limits(FramedDecodeLimits::default().with_max_framing_bytes(Some(64)));
    assert!(matches!(
        open_error(&mut framing, &bytes),
        FramedSeekError::Decode(FramedDecodeError::LimitExceeded {
            kind: FramedLimitKind::FramingBytes,
            ..
        })
    ));
    // Output limits apply per read and reset for the next one.
    let mut limited =
        with_limits(FramedDecodeLimits::default().with_max_output_bytes(Some(10_000)));
    let mut reader = limited
        .framed_seek_reader(Cursor::new(&bytes))
        .expect("open");
    let mut out = Vec::new();
    let error = reader
        .resource(ResourceIndex(1))
        .expect("plan")
        .read_to_end(&mut out)
        .expect_err("limit");
    assert!(matches!(
        seek_error(&error),
        FramedSeekError::Decode(FramedDecodeError::LimitExceeded {
            kind: FramedLimitKind::OutputBytes,
            limit: 10_000
        })
    ));
    assert_eq!(payload(&mut reader, 0), payloads[0]);
    // A large resource streams under a small workspace.
    let big = corpus::text(2 << 20, 22);
    let container = FramedCompressor::new(Default::default())
        .expect("encoder")
        .compress(
            [FramedItem::Resource(FramedResource::from(big.as_slice()))]
                .as_slice()
                .into(),
        )
        .expect("encode");
    let mut small = with_limits(
        FramedDecodeLimits::default()
            .with_max_workspace_bytes(Some(1 << 20))
            .with_max_dictionary_bytes(Some(0)),
    );
    let mut reader = small
        .framed_seek_reader(Cursor::new(&container))
        .expect("open");
    assert_eq!(payload(&mut reader, 0), big);
}

#[test]
fn forgotten_readers_keep_the_owner_protected_until_recovery() {
    let bytes = two_resources();
    let mut decoder = decoder();
    {
        let mut reader = decoder
            .framed_seek_reader(Cursor::new(&bytes))
            .expect("open");
        std::mem::forget(reader.resource(ResourceIndex(0)).expect("resource"));
        // A forgotten child is cancelled by the next operation.
        assert_eq!(payload(&mut reader, 1), b"de");
    }
    decoder.decompress(&bytes).expect("parent dropped");
    std::mem::forget(
        decoder
            .framed_seek_reader(Cursor::new(&bytes))
            .expect("open"),
    );
    assert!(matches!(
        decoder.framed_seek_reader(Cursor::new(&bytes)),
        Err(FramedSeekError::Decode(FramedDecodeError::AbandonedSession))
    ));
    decoder.recover();
    let mut reader = decoder
        .framed_seek_reader(Cursor::new(&bytes))
        .expect("recovered");
    assert_eq!(payload(&mut reader, 0), b"abc");
}

/// A source failing every read past `limit` bytes.
#[derive(Debug)]
struct Failing {
    inner: Cursor<Vec<u8>>,
    limit: u64,
}

impl Read for Failing {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.inner.position() >= self.limit {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "source gone",
            ));
        }
        self.inner.read(buffer)
    }
}

impl Seek for Failing {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

#[test]
fn source_failures_keep_their_original_error() {
    let bytes = two_resources();
    let error = decoder()
        .framed_seek_reader(Failing {
            inner: Cursor::new(bytes.clone()),
            limit: 0,
        })
        .expect_err("fails");
    let FramedSeekError::Io(io) = &error else {
        panic!("{error}");
    };
    assert_eq!(io.kind(), io::ErrorKind::ConnectionReset);
    assert!(error.to_string().starts_with("framed source failed"));
    // A source that fails only in payload fails the read, not the open.
    let mut decoder = decoder();
    let mut reader = decoder
        .framed_seek_reader(Failing {
            inner: Cursor::new(bytes.clone()),
            limit: u64::MAX,
        })
        .expect("open");
    reader.get_mut().limit = 0;
    let mut out = Vec::new();
    let error = reader
        .resource(ResourceIndex(0))
        .expect("plan")
        .read_to_end(&mut out)
        .expect_err("source");
    assert!(matches!(seek_error(&error), FramedSeekError::Io(_)));
    reader.get_mut().limit = u64::MAX;
    assert_eq!(payload(&mut reader, 1), b"de");
    for (error, message) in [
        (
            FramedSeekError::CentralDirectoryRequired,
            "central directory is required for random access",
        ),
        (
            FramedSeekError::ResourceNotFound,
            "resource index is out of range",
        ),
        (
            FramedSeekError::DirectoryMismatch,
            "central directory entry does not match the original chunk",
        ),
    ] {
        assert_eq!(error.to_string(), message);
    }
}

#[test]
fn hand_built_containers_read_like_the_sequential_decoder() {
    let mut builder = Builder::new(4);
    builder.metadata(1, b"id\x01h");
    builder.hidden(b"hidden payload");
    builder.chunk(
        &visible_fields(2, 0, 0, &EMPTY_FRAME),
        &[],
        Some((&EMPTY_FRAME, 0)),
    );
    builder.resource(b"tail");
    builder.directory(0);
    builder.footer();
    let bytes = builder.finish();
    let (_, sequential) = drive(&mut decoder(), &bytes, 7, 3).expect("sequential");
    assert_eq!(sequential, b"hidden payloadtail");
    let mut decoder = decoder();
    let mut reader = decoder
        .framed_seek_reader(Cursor::new(&bytes))
        .expect("open");
    let read: Vec<u8> = (0..3).flat_map(|i| payload(&mut reader, i)).collect();
    assert_eq!(read, sequential);
    assert_eq!(
        reader
            .resource_metadata(ResourceIndex(0))
            .expect("metadata")
            .and_then(Metadata::name),
        Some("h")
    );
}
