//! A hand-assembling container builder and event drivers shared by the
//! framed decoder's integration tests.

use mzstd::DecodeOperation;
use mzstd::framing::*;

pub const CHUNK_MAGIC: u32 = 0x184D_2A5B;
pub const SEEK_MAGIC: u32 = 0x184D_2A5E;
pub const SEEKABLE_MAGIC: u32 = 0x8F92_EAB1;
/// An empty single-segment Zstandard frame.
pub const EMPTY_FRAME: [u8; 9] = [0x28, 0xb5, 0x2f, 0xfd, 0x20, 0, 1, 0, 0];

/// A LEB128 varint.
pub fn var(mut n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while n >= 0x80 {
        out.push((n as u8) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
    out
}

/// A complete Zstandard frame of `payload` from the reference library.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    zstd_ffi::oracle::compress(payload, 3)
}

/// Fields of a visible data chunk of `kind` whose body is `body` decoding
/// to `declared` bytes, with extra flag bits.
pub fn visible_fields(kind: u8, declared: usize, flags: u8, body: &[u8]) -> Vec<u8> {
    let mut fields = vec![kind, 2];
    fields.extend(var(declared as u64));
    fields.push(flags | 4);
    fields.extend(var(body.len() as u64));
    fields
}

/// A container assembled frame by frame, keeping the seek table entries
/// and directory records a writer would.
pub struct Builder {
    pub bytes: Vec<u8>,
    pub frames: Vec<(u32, u32)>,
    pub records: Vec<(u64, Vec<u8>)>,
    pub directory: u64,
}

impl Builder {
    /// A container header with `flags` (4 is the full profile).
    pub fn new(flags: u8) -> Self {
        let mut bytes = CHUNK_MAGIC.to_le_bytes().to_vec();
        bytes.extend(5u32.to_le_bytes());
        bytes.extend(b"MZFR");
        bytes.push(flags);
        Self {
            bytes,
            frames: vec![(13, 0)],
            records: Vec::new(),
            directory: 0,
        }
    }

    /// Appends one chunk: a header frame of `fields` and `inline` content,
    /// then `body` as a detached frame decoding to `decoded` bytes.
    pub fn chunk(&mut self, fields: &[u8], inline: &[u8], body: Option<(&[u8], u32)>) -> u64 {
        let offset = self.bytes.len() as u64;
        let mut header = CHUNK_MAGIC.to_le_bytes().to_vec();
        header.extend(((fields.len() + inline.len()) as u32).to_le_bytes());
        header.extend_from_slice(fields);
        self.bytes.extend_from_slice(&header);
        self.bytes.extend_from_slice(inline);
        self.frames.push(((header.len() + inline.len()) as u32, 0));
        if let Some((body, decoded)) = body {
            self.bytes.extend_from_slice(body);
            self.frames.push((body.len() as u32, decoded));
        }
        if (1..=8).contains(&fields.first().copied().unwrap_or(0)) {
            self.records.push((offset, header));
        }
        offset
    }

    /// A visible complete resource of `payload`.
    pub fn resource(&mut self, payload: &[u8]) -> u64 {
        let body = frame(payload);
        self.chunk(
            &visible_fields(2, payload.len(), 0, &body),
            &[],
            Some((&body, payload.len() as u32)),
        )
    }

    /// A hidden complete resource stored inline.
    pub fn hidden(&mut self, payload: &[u8]) -> u64 {
        self.chunk(&[2, 0, 1], payload, None)
    }

    /// Uncompressed metadata of chunk `kind` with serialized `fields`.
    pub fn metadata(&mut self, kind: u8, fields: &[u8]) -> u64 {
        self.chunk(&[kind, 0], fields, None)
    }

    /// Zero padding of `n` content bytes.
    pub fn padding(&mut self, n: usize) -> u64 {
        self.chunk(&[0], &vec![0; n], None)
    }

    /// A directory of every record, with a repeat pointer.
    pub fn directory(&mut self, repeat: u64) -> u64 {
        let mut content = var(repeat);
        for (offset, header) in &self.records {
            content.extend(var(*offset));
            content.extend(var(header.len() as u64));
            content.extend_from_slice(header);
        }
        let mut fields = vec![9];
        fields.extend(content);
        let offset = self.chunk(&fields, &[], None);
        self.directory = offset;
        offset
    }

    /// The footer naming the directory and the size the finished
    /// container will have.
    pub fn footer(&mut self) -> u64 {
        let size = self.bytes.len() as u64 + 25 + 8 + 8 * (self.frames.len() as u64 + 1) + 9;
        self.footer_with(self.directory, size)
    }

    /// A footer with arbitrary fields.
    pub fn footer_with(&mut self, directory: u64, size: u64) -> u64 {
        let mut fields = vec![10];
        fields.extend(directory.to_le_bytes());
        fields.extend(size.to_le_bytes());
        self.chunk(&fields, &[], None)
    }

    /// Appends a correct seek table and returns the container.
    pub fn finish(self) -> Vec<u8> {
        let frames = self.frames.clone();
        self.finish_with(&frames, 0, SEEKABLE_MAGIC)
    }

    /// Appends a seek table of `frames` with `descriptor` and `magic`.
    pub fn finish_with(mut self, frames: &[(u32, u32)], descriptor: u8, magic: u32) -> Vec<u8> {
        self.bytes.extend(SEEK_MAGIC.to_le_bytes());
        self.bytes
            .extend(((frames.len() * 8 + 9) as u32).to_le_bytes());
        for (c, d) in frames {
            self.bytes.extend(c.to_le_bytes());
            self.bytes.extend(d.to_le_bytes());
        }
        self.bytes.extend((frames.len() as u32).to_le_bytes());
        self.bytes.push(descriptor);
        self.bytes.extend(magic.to_le_bytes());
        self.bytes
    }
}

/// One full-profile container with a directory: resources "abc" and "de".
pub fn two_resources() -> Vec<u8> {
    let mut builder = Builder::new(4);
    builder.resource(b"abc");
    builder.resource(b"de");
    builder.directory(0);
    builder.footer();
    builder.finish()
}

/// The single-resource profile around one visible resource.
pub fn single(payload: &[u8]) -> Vec<u8> {
    let mut builder = Builder::new(0);
    builder.resource(payload);
    builder.finish()
}

/// Feeds `bytes` in pieces of `split` into a fresh session with output
/// slices of `capacity`, rendering events and collecting payload.
pub fn drive(
    decoder: &mut FramedDecompressor,
    bytes: &[u8],
    split: usize,
    capacity: usize,
) -> Result<(Vec<String>, Vec<u8>), FramedDecodeError> {
    let mut session = decoder.start(Default::default())?;
    let mut events = Vec::new();
    let mut payload = Vec::new();
    let mut offered = 0;
    let mut position = 0;
    let mut output = vec![0; capacity];
    loop {
        let end = (offered + split.max(1)).min(bytes.len());
        let finish = end == bytes.len();
        offered = end;
        let operation = if finish {
            DecodeOperation::Finish
        } else {
            DecodeOperation::Process
        };
        loop {
            let progress = session
                .process(&bytes[position..offered], &mut output, operation)
                .map_err(FramedDecodeFailure::into_error)?;
            position += progress.consumed;
            match progress.status {
                FramedDecoderStatus::Finished => return Ok((events, payload)),
                FramedDecoderStatus::NeedsInput => break,
                FramedDecoderStatus::NeedsOutput => {
                    if capacity == 0 {
                        output = vec![0; 1];
                    }
                }
                FramedDecoderStatus::Event(event) => {
                    if let FramedEvent::ResourceData(data) = &event {
                        payload.extend_from_slice(data.bytes);
                    }
                    events.push(render(&event));
                }
            }
        }
        if finish && position == bytes.len() {
            // `NeedsInput` with all input under `Finish` cannot happen.
            return Err(FramedDecodeError::InvalidState);
        }
    }
}

/// A stable rendering of an event that ignores fragment boundaries.
pub fn render(event: &FramedEvent<'_>) -> String {
    match event {
        FramedEvent::ResourceData(data) => format!("data {:?}", data.position.resource),
        other => format!("{other:?}"),
    }
}

/// Delivers every queued container byte of `session` into `wire`.
pub fn drain(
    session: &mut FramedEncoderSession<'_>,
    wire: &mut Vec<u8>,
    operation: FramedEncodeOperation,
) {
    let mut output = [0; 4096];
    loop {
        let progress = session.process(&mut output, operation).expect("process");
        wire.extend_from_slice(&output[..progress.produced]);
        if progress.status != FramedEncoderStatus::NeedsOutput {
            return;
        }
    }
}

/// Writes all of `data` into `resource` and ends it.
pub fn write(mut resource: FramedResourceSession<'_, '_>, data: &[u8], wire: &mut Vec<u8>) {
    let mut output = [0; 4096];
    let mut input = data;
    loop {
        let progress = resource
            .process(input, &mut output, mzstd::Operation::End)
            .expect("resource");
        input = &input[progress.consumed..];
        wire.extend_from_slice(&output[..progress.produced]);
        if progress.status == FramedEncoderStatus::Finished {
            return;
        }
    }
}

/// The identity of the external prefix in [`Dictionaries`].
pub const EXTERNAL: DictionaryId = DictionaryId([7; 32]);

/// A container whose visible resources use every kind of dictionary
/// reference, with the bytes it needs.
pub struct Dictionaries {
    /// The container.
    pub bytes: Vec<u8>,
    /// The external prefix.
    pub external: Vec<u8>,
    /// A Zstandard-format dictionary, also stored as resource 1.
    pub trained: Vec<u8>,
    /// Every resource's payload, by index.
    pub payloads: Vec<Vec<u8>>,
}

impl DictionaryResolver for Dictionaries {
    fn resolve(&self, request: ExternalDictionaryRequest) -> Option<&[u8]> {
        match (request.id, request.kind) {
            (EXTERNAL, ExternalDictionaryKind::Prefix) => Some(&self.external),
            (EXTERNAL, ExternalDictionaryKind::Serialized) => Some(&self.trained),
            _ => None,
        }
    }
}

/// Resources: 0 hidden prefix, 1 hidden trained dictionary, then visible
/// ones referencing resource 0, a metadata chunk, resource 1, the external
/// prefix, resource 0 and the external prefix together, and the external
/// trained dictionary. Chunks hold 300 bytes, so resources are partial.
pub fn dictionaries() -> Dictionaries {
    use mzstd::dictionary::{DictionaryBuilder, DictionaryContent};
    let samples: Vec<Vec<u8>> = (0..300).map(|i| zstd_ffi::corpus::text(200, i)).collect();
    let views: Vec<&[u8]> = samples.iter().map(Vec::as_slice).collect();
    let trained = zstd_ffi::oracle::train_dictionary(&views, 4096).expect("train");
    let prefix = zstd_ffi::corpus::text(1000, 900);
    let external = zstd_ffi::corpus::text(800, 901);
    let fields = [MetadataField {
        code: *b"DI",
        value: &prefix[..400],
    }];
    let mut payloads = vec![prefix.clone(), trained.clone()];
    let mut owner = FramedCompressor::new(FramedEncodeConfig::default().with_framing_config(
        FramingConfig {
            chunk_bytes: 300,
            ..Default::default()
        },
    ))
    .expect("encoder");
    let mut wire = Vec::new();
    let mut session = owner.start(Default::default()).expect("start");
    drain(&mut session, &mut wire, FramedEncodeOperation::Process);
    let hidden = ResourceOptions {
        hidden: true,
        ..Default::default()
    };
    let prefix_offset = session.next_chunk_offset();
    let resource = session.uncompressed_resource(hidden).expect("prefix");
    write(resource, &prefix, &mut wire);
    let trained_offset = session.next_chunk_offset();
    let resource = session
        .resource(hidden, Default::default())
        .expect("trained");
    write(resource, &trained, &mut wire);
    let metadata_offset = session.next_chunk_offset();
    session
        .metadata(MetadataKind::Global, &fields)
        .expect("metadata");
    drain(&mut session, &mut wire, FramedEncodeOperation::Process);
    let metadata_bytes = [b"DI".as_slice(), &[0x90, 0x03], &prefix[..400]].concat();
    let raw = |bytes: &[u8]| {
        DictionaryBuilder::new(bytes)
            .with_content(DictionaryContent::Raw)
            .build()
            .expect("raw dictionary")
    };
    let zstd = DictionaryBuilder::new(&trained)
        .with_content(DictionaryContent::Zstd)
        .build()
        .expect("zstd dictionary");
    let cases = [
        (
            raw(&prefix),
            vec![DictionaryReference::PrefixResource(prefix_offset)],
        ),
        (
            raw(&metadata_bytes),
            vec![DictionaryReference::PrefixChunk(metadata_offset)],
        ),
        (
            zstd,
            vec![DictionaryReference::SerializedResource(trained_offset)],
        ),
        (
            raw(&external),
            vec![DictionaryReference::PrefixId(EXTERNAL)],
        ),
        (
            raw(&[prefix.as_slice(), &external].concat()),
            vec![
                DictionaryReference::PrefixResource(prefix_offset),
                DictionaryReference::PrefixId(EXTERNAL),
            ],
        ),
        (
            DictionaryBuilder::new(&trained)
                .with_content(DictionaryContent::Zstd)
                .build()
                .expect("zstd dictionary"),
            vec![DictionaryReference::SerializedId(EXTERNAL)],
        ),
    ];
    for (i, (dictionary, references)) in cases.iter().enumerate() {
        let payload = zstd_ffi::corpus::text(700, 950 + i as u64);
        let resource = session
            .resource_with_dictionary(
                ResourceOptions::default(),
                Default::default(),
                dictionary,
                references,
            )
            .expect("resource");
        write(resource, &payload, &mut wire);
        payloads.push(payload);
    }
    drain(&mut session, &mut wire, FramedEncodeOperation::Finish);
    drop(session);
    Dictionaries {
        bytes: wire,
        external,
        trained,
        payloads,
    }
}
