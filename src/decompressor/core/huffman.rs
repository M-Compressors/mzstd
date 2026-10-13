//! Huffman tree descriptions and literal stream decoding (RFC 8878
//! section 4.2).
//!
//! [`HuffmanTable::read`] parses a tree description, either direct 4-bit
//! weights or an FSE-compressed weight series, completes the implied last
//! weight and builds a single-level lookup table indexed by the next
//! `log` bits of a backward stream. [`HuffmanTable::decode_streams`] then
//! decodes one stream or four streams with a jump table.

use alloc::vec::Vec;

use super::bits::ReverseBitReader;
use super::fse::{MAX_WEIGHTS, decode_weights};
use crate::decompressor::InvalidDataKind;
use crate::shared::format::{LIT_HUF_LOG, highbit32};

/// Deepest code the reference decoder accepts.
///
/// RFC 8878 limits literal codes to [`LIT_HUF_LOG`] (11) bits, and encoders
/// never exceed it, but the reference decoder sizes its tables for one more
/// bit and accepts 12-bit trees; this decoder accepts exactly what it does.
pub(crate) const MAX_TABLE_LOG: u32 = LIT_HUF_LOG + 1;

/// One lookup row: the symbol and the length of its code.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct HuffmanEntry {
    symbol: u8,
    nb_bits: u8,
}

/// A literal decoding table of `1 << log` rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HuffmanTable {
    log: u32,
    entries: Vec<HuffmanEntry>,
}

impl HuffmanTable {
    /// Parses a tree description at the start of `src`, returning the table
    /// and the bytes the description occupies.
    pub(crate) fn read(src: &[u8]) -> Result<(Self, usize), InvalidDataKind> {
        let invalid = InvalidDataKind::HuffmanTable;
        let (&header, body) = src.split_first().ok_or(invalid)?;
        let mut weights = [0u8; MAX_WEIGHTS + 1];
        let (count, used) = if header >= 128 {
            let count = usize::from(header) - 127;
            let bytes = count.div_ceil(2);
            let packed = body.get(..bytes).ok_or(invalid)?;
            for (pair, &byte) in weights.chunks_mut(2).zip(packed) {
                pair[0] = byte >> 4;
                if let Some(second) = pair.get_mut(1) {
                    *second = byte & 0x0f;
                }
            }
            (count, 1 + bytes)
        } else {
            let size = usize::from(header);
            let compressed = body.get(..size).ok_or(invalid)?;
            (decode_weights(compressed, &mut weights)?, 1 + size)
        };
        Ok((Self::from_weights(&mut weights, count)?, used))
    }

    /// Completes the implied last weight of `weights[..count]` and builds
    /// the table.
    fn from_weights(
        weights: &mut [u8; MAX_WEIGHTS + 1],
        count: usize,
    ) -> Result<Self, InvalidDataKind> {
        let invalid = InvalidDataKind::HuffmanTable;
        let mut rank_count = [0u32; MAX_TABLE_LOG as usize + 1];
        let mut total: u32 = 0;
        for &weight in &weights[..count] {
            if u32::from(weight) > MAX_TABLE_LOG {
                return Err(invalid);
            }
            rank_count[usize::from(weight)] += 1;
            total += (1 << weight) >> 1;
        }
        if total == 0 {
            return Err(invalid);
        }
        let log = highbit32(total) + 1;
        if log > MAX_TABLE_LOG {
            return Err(invalid);
        }
        let rest = (1u32 << log) - total;
        if !rest.is_power_of_two() {
            return Err(invalid);
        }
        let last = highbit32(rest) + 1;
        weights[count] = last as u8;
        rank_count[last as usize] += 1;
        // A complete prefix code has an even number, at least two, of the
        // longest codes.
        if rank_count[1] < 2 || rank_count[1] & 1 != 0 {
            return Err(invalid);
        }
        let symbols = &weights[..=count];
        let mut entries = Vec::with_capacity(1 << log);
        // Lowest weights (longest codes) take the lowest code values, in
        // symbol order within one weight.
        for weight in 1..=log {
            for (symbol, &w) in symbols.iter().enumerate() {
                if u32::from(w) == weight {
                    let entry = HuffmanEntry {
                        symbol: symbol as u8,
                        nb_bits: (log + 1 - weight) as u8,
                    };
                    entries.resize(entries.len() + (1 << (weight - 1)), entry);
                }
            }
        }
        debug_assert_eq!(entries.len(), 1 << log);
        Ok(Self { log, entries })
    }

    /// Heap bytes the table holds.
    pub(crate) fn retained_bytes(&self) -> usize {
        self.entries.capacity() * size_of::<HuffmanEntry>()
    }

    /// Decodes `out.len()` symbols from one stream, which must be consumed
    /// exactly.
    fn decode_stream(&self, src: &[u8], out: &mut [u8]) -> Result<(), InvalidDataKind> {
        let invalid = InvalidDataKind::HuffmanStream;
        let mut bits = ReverseBitReader::new(src).ok_or(invalid)?;
        for byte in out.iter_mut() {
            let entry = self
                .entries
                .get(bits.peek(self.log) as usize)
                .copied()
                .unwrap_or_default();
            *byte = entry.symbol;
            bits.consume(u32::from(entry.nb_bits));
        }
        if bits.is_exhausted() {
            Ok(())
        } else {
            Err(invalid)
        }
    }

    /// Decodes `out.len()` literals from one stream or four streams.
    #[cfg_attr(all(feature = "hotpath", not(feature = "no_std")), hotpath::measure)]
    pub(crate) fn decode_streams(
        &self,
        src: &[u8],
        out: &mut [u8],
        four_streams: bool,
    ) -> Result<(), InvalidDataKind> {
        if !four_streams {
            return self.decode_stream(src, out);
        }
        let invalid = InvalidDataKind::HuffmanStream;
        let (jump, streams) = src.split_first_chunk::<6>().ok_or(invalid)?;
        if streams.len() < 4 || out.len() < 6 {
            return Err(invalid);
        }
        let size = |at: usize| usize::from(u16::from_le_bytes([jump[at], jump[at + 1]]));
        let (first, rest) = streams.split_at_checked(size(0)).ok_or(invalid)?;
        let (second, rest) = rest.split_at_checked(size(2)).ok_or(invalid)?;
        let (third, fourth) = rest.split_at_checked(size(4)).ok_or(invalid)?;
        let segment = out.len().div_ceil(4);
        let (out1, rest) = out.split_at_mut(segment);
        let (out2, rest) = rest.split_at_mut(segment);
        let (out3, out4) = rest.split_at_mut_checked(segment).ok_or(invalid)?;
        self.decode_stream(first, out1)?;
        self.decode_stream(second, out2)?;
        self.decode_stream(third, out3)?;
        self.decode_stream(fourth, out4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The RFC 8878 example tree: A B C D E F with weights 4 3 2 0 1 1,
    /// as symbols 0..=5, described directly.
    fn example_table() -> HuffmanTable {
        // Five listed weights (F is implied): header 127 + 5, then 4|3, 2|0, 1|_.
        let src = [132, 0x43, 0x20, 0x10];
        let (table, used) = HuffmanTable::read(&src).expect("valid tree");
        assert_eq!(used, 4);
        table
    }

    #[test]
    fn the_rfc_example_tree_assigns_canonical_codes() {
        let table = example_table();
        assert_eq!(table.log, 4);
        // E = 0000, F = 0001, C = 001x, B = 01xx, A = 1xxx.
        let row = |symbol, nb_bits| HuffmanEntry { symbol, nb_bits };
        assert_eq!(table.entries[0], row(4, 4));
        assert_eq!(table.entries[1], row(5, 4));
        assert_eq!(table.entries[2], row(2, 3));
        assert_eq!(table.entries[4], row(1, 2));
        assert_eq!(table.entries[8], row(0, 1));
    }

    #[test]
    fn the_rfc_example_stream_decodes_with_canonical_codes() {
        let table = example_table();
        let mut out = [0u8; 4];
        table
            .decode_stream(&[0b0001_0000, 0b0000_1101], &mut out)
            .expect("valid stream");
        // The RFC labels the last two codes of its stream example the other
        // way round from its own code table; with E = 0000 and F = 0001, as
        // the reference decoder assigns them, the stream reads A B F E.
        assert_eq!(out, [0, 1, 5, 4]);
    }

    #[test]
    fn streams_must_be_consumed_exactly() {
        let table = example_table();
        let stream = [0b0001_0000, 0b0000_1101];
        let invalid = Err(InvalidDataKind::HuffmanStream);
        assert_eq!(table.decode_stream(&stream, &mut [0u8; 3]), invalid);
        assert_eq!(table.decode_stream(&stream, &mut [0u8; 5]), invalid);
        assert_eq!(table.decode_stream(&[0x10, 0x00], &mut [0u8; 1]), invalid);
    }

    #[test]
    fn four_streams_split_the_output_by_the_jump_table() {
        let table = example_table();
        // Each one-byte stream holds the marker and three bits: A = 1, then
        // B = 01, read from the top.
        let mut src = alloc::vec![1, 0, 1, 0, 1, 0];
        src.extend_from_slice(&[0b0000_1101; 4]);
        let mut out = [0u8; 8];
        table.decode_streams(&src, &mut out, true).expect("valid");
        assert_eq!(out, [0, 1, 0, 1, 0, 1, 0, 1]);
    }

    #[test]
    fn malformed_jump_tables_are_rejected() {
        let table = example_table();
        let mut out = [0u8; 8];
        let invalid = Err(InvalidDataKind::HuffmanStream);
        // No room for the jump table, a first stream past the end, fewer
        // than four stream bytes, and fewer than six literals.
        assert_eq!(
            table.decode_streams(&[1, 0, 1, 0, 1], &mut out, true),
            invalid
        );
        let oversized = [9, 0, 1, 0, 1, 0, 1, 1, 1, 1];
        assert_eq!(table.decode_streams(&oversized, &mut out, true), invalid);
        let short = [1, 0, 1, 0, 1, 0, 1, 1, 1];
        assert_eq!(table.decode_streams(&short, &mut out, true), invalid);
        let streams = [1, 0, 1, 0, 1, 0, 1, 1, 1, 1];
        assert_eq!(table.decode_streams(&streams, &mut [0u8; 5], true), invalid);
    }

    #[test]
    fn invalid_weight_sets_are_rejected() {
        let invalid = Err(InvalidDataKind::HuffmanTable);
        // Empty description, truncated direct weights, truncated FSE weights.
        assert_eq!(HuffmanTable::read(&[]).map(|_| ()), invalid);
        assert_eq!(HuffmanTable::read(&[132, 0x43]).map(|_| ()), invalid);
        assert_eq!(HuffmanTable::read(&[5, 0, 0]).map(|_| ()), invalid);
        // All weights zero, and a weight above the maximum.
        assert_eq!(HuffmanTable::read(&[129, 0x00]).map(|_| ()), invalid);
        assert_eq!(HuffmanTable::read(&[129, 0xd0]).map(|_| ()), invalid);
        // Weights 2 and 2: total 4, so the implied weight is 3 and no
        // weight-1 codes exist.
        assert_eq!(HuffmanTable::read(&[129, 0x22]).map(|_| ()), invalid);
        // Weights 1, 1, 1 imply a fourth weight 1: valid. Weights 3 and 1
        // leave 3, which is not a power of two.
        assert!(HuffmanTable::read(&[130, 0x11, 0x10]).is_ok());
        assert_eq!(HuffmanTable::read(&[129, 0x31]).map(|_| ()), invalid);
        // Two weight-12 codes make a 13-bit tree.
        assert_eq!(HuffmanTable::read(&[129, 0xcc]).map(|_| ()), invalid);
    }

    #[test]
    fn twelve_bit_trees_are_accepted_like_the_reference() {
        // Weights 12, 11, ..., 1 and an implied 1: a complete 12-bit tree.
        let src = [139, 0xcb, 0xa9, 0x87, 0x65, 0x43, 0x21];
        let (table, used) = HuffmanTable::read(&src).expect("valid tree");
        assert_eq!(used, src.len());
        assert_eq!(table.log, MAX_TABLE_LOG);
        assert_eq!(table.entries.len(), 1 << 12);
    }
}
