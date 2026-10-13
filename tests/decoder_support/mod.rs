//! Corpora, oracle helpers and streaming drivers shared by the decoder's
//! integration tests.

#![allow(dead_code, reason = "each test binary uses a different subset")]

use mzstd::{DecodeError, DecodeOperation, DecoderStatus, Decompressor};
use zstd_ffi::oracle::{self, Params};

/// A small deterministic generator (xorshift64*).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A value in `0..bound`.
    pub fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

const WORDS: &[&str] = &[
    "the",
    "frame",
    "block",
    "window",
    "literal",
    "sequence",
    "offset",
    "match",
    "length",
    "huffman",
    "table",
    "state",
    "decoder",
    "stream",
    "checksum",
    "zstandard",
    "entropy",
    "of",
    "and",
    "a",
    "to",
    "in",
    "is",
    "with",
    "compressed",
    "data",
    "bits",
    "repeat",
];

/// English-like text: a skewed vocabulary with punctuation and newlines.
pub fn text(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        let index = rng.below(WORDS.len()).min(rng.below(WORDS.len()));
        out.extend_from_slice(WORDS[index].as_bytes());
        out.push(match rng.below(16) {
            0 => b'\n',
            1 => b',',
            2 => b'.',
            _ => b' ',
        });
    }
    out.truncate(len);
    out
}

/// Uniformly random bytes: incompressible.
pub fn random(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    (0..len).map(|_| rng.next_u64() as u8).collect()
}

/// Little-endian records with slowly changing fields: long matches at
/// varied offsets and numeric literals.
pub fn records(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(len + 16);
    let mut counter = 0u32;
    while out.len() < len {
        counter = counter.wrapping_add(1 + rng.below(3) as u32);
        out.extend_from_slice(&counter.to_le_bytes());
        out.extend_from_slice(&[0x5a, 0, 0, (rng.below(4)) as u8]);
        out.extend_from_slice(&(rng.next_u64() & 0xff_ffff).to_le_bytes()[..3]);
        out.push(b'|');
    }
    out.truncate(len);
    out
}

/// Runs of a few byte values: RLE blocks and literals.
pub fn runs(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let byte = [0u8, 0xff, b'a', 7][rng.below(4)];
        let longest = if rng.below(8) == 0 { 50_000 } else { 40 };
        let run = 1 + rng.below(longest);
        out.resize((out.len() + run).min(len), byte);
    }
    out
}

/// Segments of every other kind, so blocks change character.
pub fn mixed(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let segment = 1 + rng.below(20_000);
        let seed = rng.next_u64();
        let part = match rng.below(4) {
            0 => text(segment, seed),
            1 => random(segment.min(2_000), seed),
            2 => records(segment, seed),
            _ => runs(segment, seed),
        };
        out.extend_from_slice(&part);
    }
    out.truncate(len);
    out
}

/// Every corpus kind at `len` bytes.
pub fn corpora(len: usize, seed: u64) -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("text", text(len, seed)),
        ("random", random(len, seed)),
        ("records", records(len, seed)),
        ("runs", runs(len, seed)),
        ("mixed", mixed(len, seed)),
    ]
}

/// Reference parameters: a level with explicit checksum and content-size
/// flags.
pub fn params(level: i32, checksum: bool, content_size: bool) -> Params {
    Params::level(level)
        .with(zstd_ffi::ZSTD_c_checksumFlag, i32::from(checksum))
        .with(zstd_ffi::ZSTD_c_contentSizeFlag, i32::from(content_size))
}

/// Compresses with the reference.
pub fn compress(input: &[u8], params: &Params) -> Vec<u8> {
    oracle::compress2(input, params).expect("the reference compresses")
}

/// Decodes with the reference, allowing generous output.
pub fn reference_decompress(input: &[u8], capacity: usize) -> Result<Vec<u8>, String> {
    oracle::decompress(input, capacity)
}

/// Feeds `input` in pieces of the sizes `chunk` returns and drains output
/// through a buffer of `out_chunk` bytes, switching to `Finish` once the
/// last piece is offered.
pub fn decode_streaming(
    decoder: &mut Decompressor,
    input: &[u8],
    mut chunk: impl FnMut() -> usize,
    out_chunk: usize,
) -> Result<Vec<u8>, DecodeError> {
    let mut session = decoder.start();
    let mut output = Vec::new();
    let mut buffer = vec![0u8; out_chunk];
    let mut position = 0;
    let mut finishing = false;
    loop {
        let end = if finishing {
            input.len()
        } else {
            (position + chunk().max(1)).min(input.len())
        };
        finishing |= end == input.len();
        let operation = if finishing {
            DecodeOperation::Finish
        } else {
            DecodeOperation::Process
        };
        let progress = session
            .process(&input[position..end], &mut buffer, operation)
            .map_err(mzstd::DecodeFailure::into_error)?;
        position += progress.consumed;
        output.extend_from_slice(&buffer[..progress.produced]);
        match progress.status {
            DecoderStatus::Finished => return Ok(output),
            DecoderStatus::NeedsInput => assert_eq!(position, end, "NeedsInput consumed all"),
            DecoderStatus::NeedsOutput => {}
        }
    }
}

/// Streams with fixed input and output piece sizes.
pub fn decode_fixed(
    decoder: &mut Decompressor,
    input: &[u8],
    in_chunk: usize,
    out_chunk: usize,
) -> Result<Vec<u8>, DecodeError> {
    decode_streaming(decoder, input, || in_chunk, out_chunk)
}

/// Builds a skippable frame with `payload`.
pub fn skippable(variant: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = (0x184D_2A50u32 + u32::from(variant & 15))
        .to_le_bytes()
        .to_vec();
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Golden vectors shipped with the reference.
pub fn golden_dir(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("zstd-ffi/vendor/zstd/tests")
        .join(name)
}
