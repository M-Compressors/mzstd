//! The literals section of a compressed block (RFC 8878 section 3.1.1.3.1).
//!
//! Raw literals are borrowed straight from the block; RLE and
//! Huffman-coded literals are regenerated into a reusable buffer. The
//! Huffman table of the last `Compressed_Literals_Block` is kept for
//! `Treeless_Literals_Block`s later in the same frame.

use alloc::vec::Vec;

use super::huffman::HuffmanTable;
use crate::decompressor::InvalidDataKind;
use crate::shared::format::EncodingType;

/// Smallest compressed block: a one-byte literals header and one more byte.
const MIN_COMPRESSED_BLOCK: usize = 2;
/// Fewest literals four Huffman streams can carry.
const MIN_LITERALS_FOR_4_STREAMS: usize = 6;
/// Bytes the reference decoder requires before reading a Huffman header.
const MIN_HUFFMAN_SECTION: usize = 5;

/// Maps the two-bit literals block type onto the shared encoding names.
pub(crate) const fn encoding_type(bits: u8) -> EncodingType {
    match bits & 3 {
        0 => EncodingType::Basic,
        1 => EncodingType::Rle,
        2 => EncodingType::Compressed,
        _ => EncodingType::Repeat,
    }
}

/// Literals state carried across the blocks of one frame.
#[derive(Debug, Default)]
pub(crate) struct LiteralsDecoder {
    /// Table of the last Huffman-compressed literals block.
    huffman: Option<HuffmanTable>,
    /// Regenerated RLE or Huffman literals.
    buffer: Vec<u8>,
}

/// Reads `N` little-endian header bytes into a `u32`.
fn header_value(content: &[u8], bytes: usize) -> Option<u32> {
    let header = content.get(..bytes)?;
    Some(
        header
            .iter()
            .rev()
            .fold(0u32, |value, &byte| (value << 8) | u32::from(byte)),
    )
}

impl LiteralsDecoder {
    /// Heap bytes held between blocks.
    pub(crate) fn retained_bytes(&self) -> usize {
        self.buffer.capacity()
            + self
                .huffman
                .as_ref()
                .map_or(0, HuffmanTable::retained_bytes)
    }

    /// Forgets the previous Huffman table at a frame start.
    pub(crate) fn reset(&mut self) {
        self.huffman = None;
    }

    /// Decodes the literals section at the start of a compressed block's
    /// `content`, returning the literals and the section's size.
    #[cfg_attr(all(feature = "hotpath", not(feature = "no_std")), hotpath::measure)]
    pub(crate) fn decode<'a>(
        &'a mut self,
        content: &'a [u8],
        block_max: usize,
    ) -> Result<(&'a [u8], usize), InvalidDataKind> {
        let invalid = InvalidDataKind::LiteralsSection;
        if content.len() < MIN_COMPRESSED_BLOCK {
            return Err(invalid);
        }
        let first = content[0];
        let size_format = (first >> 2) & 3;
        match encoding_type(first) {
            kind @ (EncodingType::Basic | EncodingType::Rle) => {
                let header_size = match size_format {
                    0 | 2 => 1,
                    1 => 2,
                    _ => 3,
                };
                let header = header_value(content, header_size).ok_or(invalid)?;
                let size = (header >> if header_size == 1 { 3 } else { 4 }) as usize;
                if size > block_max {
                    return Err(invalid);
                }
                if kind == EncodingType::Basic {
                    let literals = content
                        .get(header_size..header_size + size)
                        .ok_or(invalid)?;
                    return Ok((literals, header_size + size));
                }
                let &byte = content.get(header_size).ok_or(invalid)?;
                self.buffer.clear();
                self.buffer.resize(size, byte);
                Ok((&self.buffer, header_size + 1))
            }
            kind @ (EncodingType::Compressed | EncodingType::Repeat) => {
                if kind == EncodingType::Repeat && self.huffman.is_none() {
                    return Err(invalid);
                }
                if content.len() < MIN_HUFFMAN_SECTION {
                    return Err(invalid);
                }
                let (header_size, size_bits) = match size_format {
                    0 | 1 => (3, 10),
                    2 => (4, 14),
                    _ => (5, 18),
                };
                let four_streams = size_format != 0;
                let header = header_value(content, header_size.min(4)).ok_or(invalid)?;
                let mask = (1u32 << size_bits) - 1;
                let size = ((header >> 4) & mask) as usize;
                let compressed = if header_size == 5 {
                    ((header >> 22) | (u32::from(content[4]) << 10)) as usize
                } else {
                    ((header >> (4 + size_bits)) & mask) as usize
                };
                if size > block_max || (four_streams && size < MIN_LITERALS_FOR_4_STREAMS) {
                    return Err(invalid);
                }
                let section = content
                    .get(header_size..header_size + compressed)
                    .ok_or(invalid)?;
                let streams = if kind == EncodingType::Compressed {
                    let (table, used) = HuffmanTable::read(section)?;
                    self.huffman = Some(table);
                    section.get(used..).ok_or(invalid)?
                } else {
                    section
                };
                let table = self.huffman.as_ref().ok_or(invalid)?;
                self.buffer.clear();
                self.buffer.resize(size, 0);
                table.decode_streams(streams, &mut self.buffer, four_streams)?;
                Ok((&self.buffer, header_size + compressed))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(content: &[u8]) -> Result<(Vec<u8>, usize), InvalidDataKind> {
        let mut decoder = LiteralsDecoder::default();
        decoder
            .decode(content, 1 << 17)
            .map(|(literals, used)| (literals.to_vec(), used))
    }

    #[test]
    fn raw_literals_use_all_three_size_formats() {
        // One-byte header: size 3 in bits 3..8.
        assert_eq!(
            decode(&[3 << 3, b'a', b'b', b'c', 0]),
            Ok((b"abc".to_vec(), 4))
        );
        // Two-byte header, size format 01: size 2.
        assert_eq!(
            decode(&[0b0000_0100 | (2 << 4), 0, b'x', b'y']),
            Ok((b"xy".to_vec(), 4))
        );
        // Three-byte header, size format 11: size 1.
        assert_eq!(
            decode(&[0b0000_1100 | (1 << 4), 0, 0, b'z']),
            Ok((b"z".to_vec(), 4))
        );
    }

    #[test]
    fn rle_literals_repeat_one_byte() {
        let (literals, used) = decode(&[1 | (5 << 3), b'q']).expect("valid");
        assert_eq!(literals, b"qqqqq");
        assert_eq!(used, 2);
        // A two-byte RLE header needs its byte too.
        assert_eq!(
            decode(&[0b0000_0101, 0]),
            Err(InvalidDataKind::LiteralsSection)
        );
    }

    #[test]
    fn sizes_beyond_the_block_or_the_content_are_rejected() {
        let mut decoder = LiteralsDecoder::default();
        assert_eq!(
            decoder.decode(&[4 << 3, 0, 0, 0, 0], 3).map(|_| ()),
            Err(InvalidDataKind::LiteralsSection)
        );
        assert_eq!(decode(&[9 << 3, 1]), Err(InvalidDataKind::LiteralsSection));
        assert_eq!(decode(&[0]), Err(InvalidDataKind::LiteralsSection));
    }

    #[test]
    fn treeless_literals_need_a_previous_table() {
        assert_eq!(
            decode(&[3, 0, 0, 0, 0, 0]),
            Err(InvalidDataKind::LiteralsSection)
        );
    }

    #[test]
    fn compressed_literals_check_their_header_sizes() {
        // Fewer than five bytes.
        assert_eq!(decode(&[2, 0, 0, 0]), Err(InvalidDataKind::LiteralsSection));
        // Four streams with fewer than six literals: size format 01, size 5.
        let header = 2 | (1 << 2) | (5 << 4) | (10 << 14);
        let bytes = u32::to_le_bytes(header);
        assert_eq!(
            decode(&[bytes[0], bytes[1], bytes[2], 0, 0]),
            Err(InvalidDataKind::LiteralsSection)
        );
        // A compressed size past the content.
        let header = 2 | (4 << 4) | (100 << 14);
        let bytes = u32::to_le_bytes(header);
        assert_eq!(
            decode(&[bytes[0], bytes[1], bytes[2], 0, 0]),
            Err(InvalidDataKind::LiteralsSection)
        );
    }

    #[test]
    fn huffman_literals_decode_and_seed_treeless_blocks() {
        // Single stream, two literals, compressed size 5: the RFC example
        // tree (four bytes) and one stream byte decoding A then B.
        let tree = [132u8, 0x43, 0x20, 0x10];
        let stream = 0b0000_1101u8; // A then B.
        let header = 2 | (2 << 4) | (5 << 14);
        let bytes = u32::to_le_bytes(header);
        let mut content = alloc::vec![bytes[0], bytes[1], bytes[2]];
        content.extend_from_slice(&tree);
        content.push(stream);
        let mut decoder = LiteralsDecoder::default();
        let (literals, used) = decoder.decode(&content, 1 << 17).expect("valid");
        assert_eq!(literals, [0, 1]);
        assert_eq!(used, 8);
        // A treeless block reuses the table.
        let header = 3 | (2 << 4) | (1 << 14);
        let bytes = u32::to_le_bytes(header);
        let treeless = [bytes[0], bytes[1], bytes[2], stream, 0];
        let (literals, used) = decoder.decode(&treeless, 1 << 17).expect("valid");
        assert_eq!(literals, [0, 1]);
        assert_eq!(used, 4);
        // Size formats 10 and 11 read 14- and 18-bit fields. A six-byte
        // section holds only the jump table, so its streams are missing.
        let header = 3 | (2 << 2) | (6 << 4) | (6 << 18);
        let mut wide = u32::to_le_bytes(header).to_vec();
        wide.extend_from_slice(&[1, 0, 1, 0, 1, 0]);
        assert_eq!(
            decoder.decode(&wide, 1 << 17).map(|_| ()),
            Err(InvalidDataKind::HuffmanStream)
        );
        let header = 3 | (3 << 2) | (6 << 4) | (10 << 22);
        let mut widest = u32::to_le_bytes(header).to_vec();
        widest.push(0);
        widest.extend_from_slice(&[1, 0, 1, 0, 1, 0]);
        widest.extend_from_slice(&[0b0000_1101; 4]);
        // Six literals over four streams: 2, 2, 2, 0 symbols; the last
        // stream decodes nothing, so its marker byte alone is exhausted
        // only when it is 0x01.
        *widest.last_mut().expect("stream") = 0x01;
        let (literals, used) = decoder.decode(&widest, 1 << 17).expect("valid");
        assert_eq!(literals, [0, 1, 0, 1, 0, 1]);
        assert_eq!(used, 15);
    }

    #[test]
    fn encoding_types_follow_the_two_low_bits() {
        assert_eq!(encoding_type(0b100), EncodingType::Basic);
        assert_eq!(encoding_type(0b101), EncodingType::Rle);
        assert_eq!(encoding_type(0b110), EncodingType::Compressed);
        assert_eq!(encoding_type(0b111), EncodingType::Repeat);
    }
}
