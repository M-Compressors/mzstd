//! An independent reading of the framed wire format, oracle helpers and
//! session drivers shared by the framed encoder's integration tests.
//!
//! The parser deliberately shares no code with the crate: it walks a
//! container frame by frame and asserts every structural rule of
//! `architecture/framing.md`.

#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::ops::Range;

use mzstd::framing::*;
use mzstd::{EncoderConfig, Operation};
use zstd_ffi::oracle::{self, Params};
use zstd_ffi::*;

/// Magic number of the container header and every chunk header frame.
pub const CHUNK_MAGIC: u32 = 0x184D_2A5B;
/// Magic number of the seek table frame.
pub const SEEK_TABLE_MAGIC: u32 = 0x184D_2A5E;
/// The seek table footer's magic number, the container's last four bytes.
pub const SEEKABLE_MAGIC: u32 = 0x8F92_EAB1;
/// Magic number of a Zstandard frame.
pub const ZSTD_MAGIC: u32 = 0xFD2F_B528;

/// One chunk as read from the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// Offset of the chunk's header frame.
    pub offset: u64,
    /// Chunk type, 0 to 10.
    pub kind: u8,
    /// Codec byte, for types 1-8.
    pub codec: Option<u8>,
    /// Declared decoded size, for compressed codecs.
    pub declared: Option<u64>,
    /// Raw reference entries: flag and pointer or identity.
    pub references: Vec<(u8, Vec<u8>)>,
    /// Resource flags, for types 2-5.
    pub flags: u8,
    /// Whole-resource identity, when flagged.
    pub checksum: Option<[u8; 32]>,
    /// Repeated metadata kind, for type 8.
    pub repeated_kind: Option<u8>,
    /// The header frame's header bytes: frame header and chunk header.
    pub header: Range<usize>,
    /// Inline content inside the header frame.
    pub inline: Range<usize>,
    /// The detached Zstandard body frame.
    pub body: Option<Range<usize>>,
}

impl Chunk {
    /// Where the content lives: the body when detached, else inline.
    pub fn content(&self) -> Range<usize> {
        self.body.clone().unwrap_or_else(|| self.inline.clone())
    }
}

/// A fully walked container.
#[derive(Clone, Debug)]
pub struct Container {
    /// Container flags.
    pub flags: u8,
    /// Every chunk in wire order.
    pub chunks: Vec<Chunk>,
    /// Seek table entries: compressed and decompressed size of every frame.
    pub seek: Vec<(u32, u32)>,
    /// Footer directory offset and file size, in the container profile.
    pub footer: Option<(u64, u64)>,
    /// Resources: their chunk indices and visibility.
    pub resources: Vec<(Vec<usize>, bool)>,
}

impl Container {
    /// Whether the full profile is in use.
    pub fn is_container(&self) -> bool {
        self.flags & 4 != 0
    }
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn le64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

fn varint(bytes: &[u8], cursor: &mut usize, end: usize) -> u64 {
    let mut value = 0u64;
    for shift in (0..63).step_by(7) {
        assert!(*cursor < end, "varint runs past its field");
        let byte = bytes[*cursor];
        *cursor += 1;
        value |= u64::from(byte & 0x7F) << shift;
        if byte < 0x80 {
            return value;
        }
    }
    panic!("varint longer than nine bytes");
}

/// Size of a Zstandard frame at the start of `bytes`, according to the
/// reference.
pub fn zstd_frame_size(bytes: &[u8]) -> usize {
    // SAFETY: `bytes` is valid for its length.
    let size = unsafe { ZSTD_findFrameCompressedSize(bytes.as_ptr().cast(), bytes.len()) };
    // SAFETY: a pure function of its argument.
    assert_eq!(unsafe { ZSTD_isError(size) }, 0, "not a Zstandard frame");
    size
}

/// Walks `bytes` and asserts every structural invariant of the format.
///
/// # Panics
///
/// Panics on any violation.
pub fn parse(bytes: &[u8]) -> Container {
    assert!(bytes.len() >= 13 + 17, "too short for a container");
    assert_eq!(le32(bytes, 0), CHUNK_MAGIC);
    assert_eq!(le32(bytes, 4), 5);
    assert_eq!(&bytes[8..12], b"MZFR");
    let flags = bytes[12];
    assert_eq!(flags & 3, 0, "version bits");
    let mut frames: Vec<(u32, u32)> = vec![(13, 0)];
    let mut chunks = Vec::new();
    let mut resources: Vec<(Vec<usize>, bool)> = Vec::new();
    let mut partial = false;
    let mut p = 13;
    loop {
        let magic = le32(bytes, p);
        if magic == SEEK_TABLE_MAGIC {
            break;
        }
        assert_eq!(magic, CHUNK_MAGIC, "frame at {p} is not a chunk");
        let offset = p as u64;
        let size = le32(bytes, p + 4) as usize;
        let end = p + 8 + size;
        assert!(end <= bytes.len());
        let mut c = p + 8;
        let kind = if size == 0 {
            0
        } else {
            c += 1;
            bytes[c - 1]
        };
        assert!(kind <= 10, "chunk type {kind}");
        let mut chunk = Chunk {
            offset,
            kind,
            codec: None,
            declared: None,
            references: Vec::new(),
            flags: 0,
            checksum: None,
            repeated_kind: None,
            header: p..p,
            inline: end..end,
            body: None,
        };
        if (1..=8).contains(&kind) {
            let codec = bytes[c];
            c += 1;
            assert!(matches!(codec, 0 | 2 | 3), "codec {codec}");
            chunk.codec = Some(codec);
            if codec != 0 {
                chunk.declared = Some(varint(bytes, &mut c, end));
            }
            if codec == 3 {
                let count = bytes[c] as usize;
                c += 1;
                assert!((1..=16).contains(&count));
                for _ in 0..count {
                    let flag = bytes[c];
                    c += 1;
                    let start = c;
                    match flag {
                        0 | 1 | 4 => {
                            varint(bytes, &mut c, end);
                        }
                        2 | 6 => {
                            assert_eq!(bytes[c], 3);
                            c += 33;
                        }
                        _ => panic!("reference flag {flag}"),
                    }
                    chunk.references.push((flag, bytes[start..c].to_vec()));
                }
                let serialized = chunk.references.iter().filter(|r| r.0 & 4 != 0).count();
                assert!(serialized == 0 || count == 1, "mixed dictionary kinds");
            }
        }
        if (2..=5).contains(&kind) {
            let flags = bytes[c];
            c += 1;
            let allowed = match kind {
                2 => 7,
                3 => 5,
                4 => 4,
                _ => 6,
            };
            assert_eq!(flags & !allowed, 0, "flags {flags:#x} on type {kind}");
            chunk.flags = flags;
            if flags & 2 != 0 {
                assert_eq!(bytes[c], 3);
                chunk.checksum = Some(bytes[c + 1..c + 33].try_into().expect("id"));
                c += 33;
            }
            if flags & 4 != 0 {
                assert_ne!(chunk.codec, Some(0), "a detached chunk is compressed");
                let body = varint(bytes, &mut c, end) as usize;
                chunk.body = Some(end..end + body);
            }
        }
        if kind == 8 {
            let original = bytes[c];
            c += 1;
            assert!(matches!(original, 1 | 6));
            chunk.repeated_kind = Some(original);
        }
        assert!(c <= end, "header runs past its frame");
        chunk.header = p..c;
        chunk.inline = c..end;
        frames.push(((end - p) as u32, 0));
        p = end;
        if let Some(body) = chunk.body.clone() {
            assert!(
                chunk.inline.is_empty(),
                "a detached chunk has inline content"
            );
            assert_eq!(le32(bytes, body.start), ZSTD_MAGIC);
            assert_eq!(zstd_frame_size(&bytes[body.clone()]), body.len());
            frames.push((body.len() as u32, chunk.declared.expect("declared") as u32));
            p = body.end;
        } else if matches!(chunk.codec, Some(2 | 3)) {
            let inline = &bytes[chunk.inline.clone()];
            assert_eq!(le32(inline, 0), ZSTD_MAGIC);
            assert_eq!(zstd_frame_size(inline), inline.len());
        }
        if kind == 0 {
            assert!(bytes[chunk.inline.clone()].iter().all(|b| *b == 0));
        }
        // Resource structure and canonical placement.
        let index = chunks.len();
        match kind {
            2 | 3 => {
                assert!(!partial);
                let hidden = chunk.flags & 1 != 0;
                resources.push((vec![index], hidden));
                partial = kind == 3;
            }
            4 | 5 => {
                assert!(partial);
                resources.last_mut().expect("resource").0.push(index);
                partial = kind == 4;
            }
            _ => assert!(!partial || kind == 0),
        }
        if (2..=5).contains(&kind) {
            let hidden = resources.last().expect("resource").1;
            assert_eq!(chunk.body.is_some(), !hidden, "canonical placement");
        }
        chunks.push(chunk);
    }
    assert!(!partial);
    // Seek table.
    let table = p;
    let size = le32(bytes, table + 4) as usize;
    assert_eq!(
        table + 8 + size,
        bytes.len(),
        "the seek table ends the container"
    );
    assert_eq!((size - 9) % 8, 0);
    let count = (size - 9) / 8;
    let seek: Vec<(u32, u32)> = (0..count)
        .map(|i| {
            (
                le32(bytes, table + 8 + 8 * i),
                le32(bytes, table + 12 + 8 * i),
            )
        })
        .collect();
    assert_eq!(seek, frames, "seek table entries");
    let tail = bytes.len() - 9;
    assert_eq!(le32(bytes, tail) as usize, count);
    assert_eq!(bytes[tail + 4], 0, "descriptor");
    assert_eq!(le32(bytes, tail + 5), SEEKABLE_MAGIC);
    let mut footer = None;
    if flags & 4 != 0 {
        let last = chunks.last().expect("a footer");
        assert_eq!(last.kind, 10);
        let content = &bytes[last.inline.clone()];
        assert_eq!(content.len(), 16);
        let directory = le64(content, 0);
        let file_size = le64(content, 8);
        assert_eq!(file_size, bytes.len() as u64);
        match chunks.iter().find(|c| c.kind == 9) {
            Some(d) => assert_eq!(directory, d.offset),
            None => assert_eq!(directory, 0),
        }
        footer = Some((directory, file_size));
        check_directory(bytes, &chunks);
    } else {
        assert_eq!(resources.len(), 1, "a single-resource profile");
        assert!(chunks.iter().all(|c| matches!(c.kind, 0 | 2..=5)));
    }
    Container {
        flags,
        chunks,
        seek,
        footer,
        resources,
    }
}

/// Asserts the central directory lists every content chunk's exact header.
fn check_directory(bytes: &[u8], chunks: &[Chunk]) {
    let Some(directory) = chunks.iter().find(|c| c.kind == 9) else {
        return;
    };
    let content = directory.inline.clone();
    let mut c = content.start;
    let repeat = varint(bytes, &mut c, content.end);
    let first_repeat = chunks.iter().find(|c| c.kind == 8).map_or(0, |c| c.offset);
    assert_eq!(repeat, first_repeat);
    for chunk in chunks.iter().filter(|c| c.codec.is_some()) {
        let offset = varint(bytes, &mut c, content.end);
        let length = varint(bytes, &mut c, content.end) as usize;
        assert_eq!(offset, chunk.offset);
        assert_eq!(&bytes[c..c + length], &bytes[chunk.header.clone()]);
        c += length;
    }
    assert_eq!(c, content.end, "the directory lists only content chunks");
}

/// Decoded payload of each resource, decoded chunk by chunk with the
/// reference; `dictionary` serves chunks with references.
pub fn resources(bytes: &[u8], dictionary: Option<&[u8]>) -> Vec<Vec<u8>> {
    let container = parse(bytes);
    container
        .resources
        .iter()
        .map(|(indices, _)| {
            let mut payload = Vec::new();
            for &i in indices {
                payload.extend(content(bytes, &container.chunks[i], dictionary));
            }
            payload
        })
        .collect()
}

/// Decoded content of one chunk.
pub fn content(bytes: &[u8], chunk: &Chunk, dictionary: Option<&[u8]>) -> Vec<u8> {
    let raw = &bytes[chunk.content()];
    match chunk.codec {
        Some(0) => raw.to_vec(),
        Some(2) => oracle::decompress(raw, chunk.declared.expect("size") as usize)
            .expect("the reference decodes the chunk"),
        Some(3) => {
            let dictionary = oracle::Dictionary::auto(dictionary.expect("a dictionary"));
            oracle::decompress_with_dictionary(
                raw,
                chunk.declared.expect("size") as usize,
                &dictionary,
            )
            .expect("the reference decodes the chunk")
        }
        _ => raw.to_vec(),
    }
}

/// The concatenated visible resources, decoded by `ZSTD_decompress` of
/// the whole container.
pub fn zstd_decompress(bytes: &[u8]) -> Vec<u8> {
    let container = parse(bytes);
    let capacity: u64 = container.seek.iter().map(|(_, d)| u64::from(*d)).sum();
    oracle::decompress(bytes, capacity as usize + 1).expect("ZSTD_decompress accepts a container")
}

/// The reference parameters equivalent to `config`'s level and flags.
pub fn oracle_params(config: &EncoderConfig) -> Params {
    Params::level(config.level().get())
        .with(ZSTD_c_checksumFlag, i32::from(config.checksum()))
        .with(ZSTD_c_contentSizeFlag, i32::from(config.content_size()))
}

/// Encodes `input` through a borrowed session command by command, with
/// output slices of `width` bytes.
pub fn structured_native(
    owner: &mut FramedCompressor,
    input: FramedInput<'_>,
    width: usize,
) -> Vec<u8> {
    fn drain(
        s: &mut FramedEncoderSession<'_>,
        wire: &mut Vec<u8>,
        width: usize,
        op: FramedEncodeOperation,
    ) {
        let mut buffer = vec![0; width];
        loop {
            let p = s.process(&mut buffer, op).expect("process");
            wire.extend_from_slice(&buffer[..p.produced]);
            if p.status != FramedEncoderStatus::NeedsOutput {
                break;
            }
        }
    }
    let mut wire = Vec::new();
    let mut s = owner.start(Default::default()).expect("start");
    drain(&mut s, &mut wire, width, FramedEncodeOperation::Process);
    if let Some(codes) = input.repeat_metadata_fields {
        s.repeat_metadata_fields(codes).expect("selection");
    }
    for item in input.items {
        match *item {
            FramedItem::Metadata {
                kind,
                fields,
                options,
            } => s
                .metadata_with_options(kind, fields, options)
                .expect("metadata"),
            FramedItem::Padding { bytes } => s.padding(bytes).expect("padding"),
            FramedItem::Resource(resource) => {
                let mut r = match resource.encoding {
                    ResourceEncoding::Uncompressed => {
                        s.uncompressed_resource(resource.options).expect("resource")
                    }
                    ResourceEncoding::Zstd => s
                        .resource(resource.options, resource.stream)
                        .expect("resource"),
                    ResourceEncoding::Shared {
                        dictionary,
                        references,
                    } => s
                        .resource_with_dictionary(
                            resource.options,
                            resource.stream,
                            dictionary,
                            references,
                        )
                        .expect("resource"),
                };
                let mut consumed = 0;
                let mut output = vec![0; width];
                loop {
                    let p = r
                        .process(&resource.data[consumed..], &mut output, Operation::End)
                        .expect("resource data");
                    consumed += p.consumed;
                    wire.extend_from_slice(&output[..p.produced]);
                    if p.status == FramedEncoderStatus::Finished {
                        break;
                    }
                }
            }
        }
        drain(&mut s, &mut wire, width, FramedEncodeOperation::Process);
    }
    drain(&mut s, &mut wire, width, FramedEncodeOperation::Finish);
    wire
}
