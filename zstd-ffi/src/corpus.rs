//! Deterministic test inputs covering the shapes the codecs distinguish.

use alloc::string::String;
use alloc::vec::Vec;

extern crate std;

/// A xorshift generator, so inputs are reproducible without dependencies.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Word soup with occasional noise: compressible, text-like.
pub fn text(len: usize, seed: u64) -> Vec<u8> {
    const WORDS: [&str; 16] = [
        "the ",
        "quick ",
        "brown ",
        "fox ",
        "jumps ",
        "over ",
        "lazy ",
        "dog ",
        "zstd ",
        "frame ",
        "block ",
        "literal ",
        "sequence ",
        "offset ",
        "match ",
        "window\n",
    ];
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        let r = rng.next_u64();
        out.extend_from_slice(WORDS[(r % 16) as usize].as_bytes());
        if r.is_multiple_of(23) {
            out.push((r >> 32) as u8);
        }
    }
    out.truncate(len);
    out
}

/// Uniform random bytes: incompressible.
pub fn random(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    (0..len).map(|_| (rng.next_u64() >> 24) as u8).collect()
}

/// Structured binary records with small fields and repeats.
pub fn binary(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(len + 16);
    let mut counter = 0u32;
    while out.len() < len {
        let r = rng.next_u64();
        counter = counter.wrapping_add((r % 5) as u32);
        out.extend_from_slice(&counter.to_le_bytes());
        out.extend_from_slice(&((r >> 8) as u16 & 0x0F0F).to_le_bytes());
        out.push((r >> 40) as u8 & 3);
        if r.is_multiple_of(7) {
            let back = (r >> 20) as usize % (out.len().max(1));
            let take = ((r >> 50) as usize % 40).min(out.len() - back);
            let copy: Vec<u8> = out[back..back + take].to_vec();
            out.extend_from_slice(&copy);
        }
    }
    out.truncate(len);
    out
}

/// Blocks of different character, so statistics change mid-input.
pub fn mixed(len: usize, seed: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut part = 0u64;
    while out.len() < len {
        let chunk = 20_000 + (seed as usize % 7) * 3_000;
        let piece = match part % 4 {
            0 => text(chunk, seed + part),
            1 => random(chunk / 4, seed + part),
            2 => binary(chunk, seed + part),
            _ => alloc::vec![b'z'; chunk / 2],
        };
        out.extend_from_slice(&piece);
        part += 1;
    }
    out.truncate(len);
    out
}

/// The reference's own C sources: real text.
pub fn sources() -> Vec<u8> {
    let root = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/zstd-ffi/vendor/zstd/lib/compress/"
    );
    let mut out = Vec::new();
    for name in [
        "zstd_compress.c",
        "zstd_lazy.c",
        "zstd_opt.c",
        "huf_compress.c",
    ] {
        let path = String::from(root) + name;
        if let Ok(bytes) = std::fs::read(&path) {
            out.extend_from_slice(&bytes);
        }
    }
    out
}

/// The standard differential corpus: every shape at boundary sizes.
pub fn standard() -> Vec<(&'static str, Vec<u8>)> {
    let mut inputs = Vec::new();
    for len in [
        0usize, 1, 2, 7, 8, 9, 15, 16, 17, 63, 64, 100, 255, 256, 1000, 4096,
    ] {
        inputs.push(("text", text(len, len as u64)));
        inputs.push(("random", random(len, len as u64)));
    }
    inputs.push(("zeros", alloc::vec![0u8; 50_000]));
    inputs.push(("byte-run", alloc::vec![0xAB; 300_000]));
    for len in [
        16 << 10,
        (16 << 10) + 1,
        128 << 10,
        (128 << 10) + 1,
        256 << 10,
        (256 << 10) + 1,
    ] {
        inputs.push(("text", text(len, 7)));
        inputs.push(("binary", binary(len, 9)));
    }
    inputs.push(("random-large", random(300_000, 3)));
    inputs.push(("mixed", mixed(1 << 20, 5)));
    inputs.push(("text-large", text(1_500_000, 11)));
    inputs.push(("sources", sources()));
    inputs
}

/// `count` small JSON-like records sharing field names and value
/// vocabularies, the shape dictionaries are made for.
pub fn records(count: usize, seed: u64) -> Vec<Vec<u8>> {
    const NAMES: [&str; 8] = [
        "alice", "bob", "carol", "dave", "erin", "frank", "grace", "heidi",
    ];
    const CITIES: [&str; 6] = ["Berlin", "Lisbon", "Oslo", "Prague", "Tallinn", "Vienna"];
    const TAGS: [&str; 7] = [
        "compression",
        "dictionary",
        "frame",
        "stream",
        "window",
        "entropy",
        "sequence",
    ];
    let mut rng = Rng::new(seed);
    (0..count)
        .map(|index| {
            let mut pick = |n: usize| (rng.next_u64() % n as u64) as usize;
            let mut record = alloc::format!(
                "{{\"id\":{},\"user\":\"{}\",\"city\":\"{}\",\"score\":{},\"active\":{},\"tags\":[",
                1000 + index * 7 + pick(5),
                NAMES[pick(NAMES.len())],
                CITIES[pick(CITIES.len())],
                pick(100_000),
                pick(2) == 0,
            );
            for tag in 0..1 + pick(4) {
                if tag > 0 {
                    record.push(',');
                }
                record.push('"');
                record.push_str(TAGS[pick(TAGS.len())]);
                record.push('"');
            }
            record.push_str("],\"note\":\"");
            for _ in 0..pick(12) {
                record.push(char::from(b'a' + pick(26) as u8));
            }
            record.push_str("\"}\n");
            record.into_bytes()
        })
        .collect()
}
