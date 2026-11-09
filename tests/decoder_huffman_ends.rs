#![cfg(feature = "decompression")]
//! Huffman literal streams whose ends the reference decoder does not check.
//!
//! zstd 1.5.7's fast four-stream Huffman decoder validates only the
//! regenerated length: a stream may stop short of its first bit or read on
//! into the bytes before it. The frames in `testdata/huffman-stream-ends`
//! are fuzz finds (two as found, four cut down to their literals and
//! zeroed byte by byte) that `ZSTD_decompress` accepts for that reason.
//! This decoder must accept them too, produce the same bytes on every
//! backend and through every entry point, and agree with the reference on
//! every damaged variant of them.

mod decoder_support;

use decoder_support::{Rng, decode_fixed, reference_decompress};
use mzstd::{Backend, DecoderConfig, Decompressor};

/// The committed frames, by name.
fn frames() -> Vec<(String, Vec<u8>)> {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/huffman-stream-ends");
    let mut frames: Vec<_> = std::fs::read_dir(&directory)
        .expect("the fixture directory")
        .map(|entry| {
            let path = entry.expect("a fixture").path();
            let name = path
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned();
            (name, std::fs::read(&path).expect("a readable fixture"))
        })
        .collect();
    frames.sort();
    assert_eq!(frames.len(), 6, "every fixture is present");
    frames
}

#[test]
fn frames_the_reference_accepts_decode_identically_on_every_backend() {
    for (name, frame) in frames() {
        let expected = reference_decompress(&frame, 1 << 18)
            .unwrap_or_else(|error| panic!("{name}: the reference rejects it: {error}"));
        for backend in Backend::available() {
            let mut decoder = Decompressor::builder(DecoderConfig::default())
                .with_backend(backend)
                .build();
            let label = format!("{name} on {backend}");
            assert_eq!(
                decoder.decompress(&frame).expect(&label),
                expected,
                "{label}: one-shot"
            );
            let mut slice = vec![0u8; expected.len()];
            let written = decoder
                .decompress_to_slice(&frame, &mut slice)
                .expect(&label);
            assert_eq!(&slice[..written], expected, "{label}: slice");
            assert_eq!(
                decode_fixed(&mut decoder, &frame, 61, 1000).expect(&label),
                expected,
                "{label}: streaming"
            );
        }
    }
}

#[test]
fn damaged_variants_get_the_reference_verdict_and_output() {
    let mut decoder = Decompressor::default();
    let mut rng = Rng::new(0x5eed_4ee1);
    for (name, frame) in frames() {
        // Damage only the literals section, after the frame, block and
        // literals headers, so every verdict is the Huffman decoder's.
        let first = 14;
        let last = if name.starts_with("min-") {
            frame.len() - 1
        } else {
            first + 1500
        };
        for case in 0..400 {
            let mut damaged = frame.clone();
            for _ in 0..1 + rng.below(4) {
                damaged[first + rng.below(last - first)] = rng.next_u64() as u8;
            }
            let label = format!("{name} case {case}");
            let reference = reference_decompress(&damaged, 1 << 18).ok();
            let ours = decoder.decompress(&damaged).ok();
            assert_eq!(ours, reference, "{label}: verdict and output");
        }
    }
}

/// A random complete prefix code over 2 to 40 of the first 64 symbols,
/// at most `max_depth` bits deep: each symbol's weight, zero for absent
/// ones, with the last symbol present.
fn random_weights(rng: &mut Rng, max_depth: u32) -> Vec<u8> {
    // Split random leaves of a two-leaf code until it is big enough.
    let mut lengths = vec![1u32, 1];
    let leaves = 2 + rng.below(39);
    while lengths.len() < leaves {
        let at = rng.below(lengths.len());
        if lengths[at] == max_depth {
            if lengths.iter().all(|&length| length == max_depth) {
                break;
            }
            continue;
        }
        lengths[at] += 1;
        lengths.push(lengths[at]);
    }
    let depth = *lengths.iter().max().expect("leaves");
    let mut weights: Vec<u8> = lengths
        .iter()
        .map(|&length| (depth + 1 - length) as u8)
        .collect();
    // Shuffle, then spread absent symbols in; the last one stays present.
    for index in (1..weights.len()).rev() {
        weights.swap(index, rng.below(index + 1));
    }
    let last = weights.pop().expect("leaves");
    let mut spread = Vec::new();
    for weight in weights {
        while rng.below(4) == 0 && spread.len() < 60 {
            spread.push(0);
        }
        spread.push(weight);
    }
    spread.push(last);
    spread
}

/// The direct tree description of `weights`: all but the last, packed.
fn describe(weights: &[u8]) -> Vec<u8> {
    let listed = &weights[..weights.len() - 1];
    let mut description = vec![127 + listed.len() as u8];
    for pair in listed.chunks(2) {
        description.push(pair[0] << 4 | pair.get(1).copied().unwrap_or(0));
    }
    description
}

/// Each symbol's canonical code and length, as RFC 8878 assigns them:
/// by ascending weight, then symbol.
fn codes(weights: &[u8]) -> Vec<(u64, u32)> {
    let total: u32 = weights.iter().map(|&weight| (1u32 << weight) >> 1).sum();
    let depth = total.ilog2();
    let mut codes = vec![(0, 0); weights.len()];
    let mut next = 0u64;
    for weight in 1..=depth as u8 + 1 {
        for (symbol, _) in weights.iter().enumerate().filter(|&(_, &w)| w == weight) {
            let length = depth + 1 - u32::from(weight);
            codes[symbol] = (next >> (weight - 1), length);
            next += 1 << (weight - 1);
        }
    }
    codes
}

/// A Huffman stream of `symbols`, read back first to last.
fn encode(codes: &[(u64, u32)], symbols: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let (mut acc, mut filled) = (0u64, 0u32);
    let mut put = |value: u64, width: u32, bytes: &mut Vec<u8>| {
        acc |= value << filled;
        filled += width;
        while filled >= 8 {
            bytes.push(acc as u8);
            acc >>= 8;
            filled -= 8;
        }
    };
    for &symbol in symbols.iter().rev() {
        let (code, length) = codes[usize::from(symbol)];
        put(code, length, &mut bytes);
    }
    put(1, 1, &mut bytes);
    if filled > 0 {
        bytes.push(acc as u8);
    }
    bytes
}

/// Streams for `n` literals, one or four, mostly valid: each holds a few
/// symbols more or fewer than its segment, and may be replaced by random
/// bytes, cut or damaged.
fn streams(rng: &mut Rng, codes: &[(u64, u32)], n: usize, four: bool) -> Vec<u8> {
    let alphabet: Vec<u8> = (0..codes.len() as u8)
        .filter(|&symbol| codes[usize::from(symbol)].1 > 0)
        .collect();
    let segment = n.div_ceil(4);
    let counts = if four {
        vec![segment, segment, segment, n.saturating_sub(3 * segment)]
    } else {
        vec![n]
    };
    let streams: Vec<Vec<u8>> = counts
        .iter()
        .map(|&count| {
            let count = match rng.below(4) {
                0 => count + rng.below(3),
                1 => count.saturating_sub(rng.below(3)),
                _ => count,
            };
            let symbols: Vec<u8> = (0..count)
                .map(|_| alphabet[rng.below(alphabet.len()).min(rng.below(alphabet.len()))])
                .collect();
            let mut stream = match rng.below(8) {
                0 => (0..rng.below(24)).map(|_| rng.next_u64() as u8).collect(),
                _ => encode(codes, &symbols),
            };
            if rng.below(6) == 0 && !stream.is_empty() {
                let at = rng.below(stream.len());
                stream[at] ^= 1 << rng.below(8);
            }
            if rng.below(8) == 0 {
                stream.truncate(stream.len().saturating_sub(1 + rng.below(2)));
            }
            stream
        })
        .collect();
    let mut out = Vec::new();
    if four {
        for stream in &streams[..3] {
            out.extend_from_slice(&(stream.len() as u16).to_le_bytes());
        }
    }
    for stream in &streams {
        out.extend_from_slice(stream);
    }
    out
}

/// A compressed block's content: a Huffman literals section of `n`
/// literals over `payload` (tree description and streams, or streams
/// alone for a treeless one), then no sequences.
fn literals_block(treeless: bool, four: bool, n: usize, payload: &[u8]) -> Vec<u8> {
    let kind = if treeless { 3u32 } else { 2 };
    let mut block = if four && (n >= 1024 || payload.len() >= 1024) {
        let header = kind | 2 << 2 | (n as u32) << 4 | (payload.len() as u32) << 18;
        header.to_le_bytes().to_vec()
    } else {
        let header = kind | u32::from(four) << 2 | (n as u32) << 4 | (payload.len() as u32) << 14;
        header.to_le_bytes()[..3].to_vec()
    };
    block.extend_from_slice(payload);
    block.push(0);
    block
}

/// A frame of compressed `blocks` with a 128 KiB window and no content
/// size, so that no block is too large for it.
fn frame_of(blocks: &[Vec<u8>]) -> Vec<u8> {
    let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0x00, 7 << 3];
    for (index, block) in blocks.iter().enumerate() {
        let last = u32::from(index + 1 == blocks.len());
        let header = last | 2 << 1 | (block.len() as u32) << 3;
        frame.extend_from_slice(&header.to_le_bytes()[..3]);
        frame.extend_from_slice(block);
    }
    frame
}

#[test]
fn synthetic_sections_get_the_reference_verdict_and_output() {
    let mut decoder = Decompressor::default();
    let mut rng = Rng::new(0x4e57_ab1e);
    let mut accepted = [0usize; 2];
    for case in 0..3000 {
        let max_depth = if rng.below(10) == 0 {
            12
        } else {
            4 + rng.below(8) as u32
        };
        let weights = random_weights(&mut rng, max_depth);
        let codes = codes(&weights);
        let description = describe(&weights);
        let sizes = [6, 7, 9, 13, 40, 100, 333, 1000, 1023, 4000, 20_000];
        let pick = |rng: &mut Rng, four: bool| {
            let n = sizes[rng.below(sizes.len())];
            if four { n } else { n.min(1000) }
        };
        let (four, next_four) = (rng.below(4) != 0, rng.below(2) == 0);
        let (n, next) = (pick(&mut rng, four), pick(&mut rng, next_four));
        let mut first = description.clone();
        first.extend_from_slice(&streams(&mut rng, &codes, n, four));
        let mut blocks = vec![literals_block(false, four, n, &first)];
        // A treeless block after it reuses the tree, and the reference
        // the decoder type it read the tree for.
        if rng.below(2) == 0 {
            let payload = streams(&mut rng, &codes, next, next_four);
            blocks.push(literals_block(true, next_four, next, &payload));
        }
        let frame = frame_of(&blocks);
        let reference = reference_decompress(&frame, 1 << 18).ok();
        let ours = decoder.decompress(&frame).ok();
        assert_eq!(ours, reference, "case {case}: verdict and output");
        accepted[usize::from(reference.is_some())] += 1;
    }
    // Both verdicts are well represented.
    assert!(accepted.iter().all(|&count| count > 300), "{accepted:?}");
}
