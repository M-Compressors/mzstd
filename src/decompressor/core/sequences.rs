//! The sequences section of a compressed block (RFC 8878
//! section 3.1.1.3.2) and sequence execution (section 3.1.1.4).
//!
//! [`SequenceTables::read_header`] parses the sequence count and the three
//! table descriptions, updating the tables that `Repeat_Mode` reuses.
//! [`SequenceTables::execute`] then decodes the interleaved bitstream and
//! executes every sequence as soon as it is decoded: literals are appended
//! to the output, followed by a match copied from the frame's history.

use alloc::vec::Vec;

use super::bits::ReverseBitReader;
use super::fse::{FseTable, read_distribution};
use super::literals::encoding_type;
use crate::decompressor::InvalidDataKind;
use crate::shared::format::{
    EncodingType, LL_BITS, LL_DEFAULT_NORM, LL_DEFAULT_NORM_LOG, LL_FSE_LOG, LONG_NB_SEQ, MAX_LL,
    MAX_ML, MAX_OFF, MAX_SEQ, MIN_MATCH, ML_BITS, ML_DEFAULT_NORM, ML_DEFAULT_NORM_LOG, ML_FSE_LOG,
    OF_DEFAULT_NORM, OF_DEFAULT_NORM_LOG, OFF_FSE_LOG, REP_NUM,
};

/// Baselines of a length code alphabet: each code starts where the range of
/// the previous one, `1 << extra_bits` values wide, ends.
const fn baselines<const N: usize>(bits: &[u8; N], first: u32) -> [u32; N] {
    let mut base = [0u32; N];
    let mut value = first;
    let mut code = 0;
    while code < N {
        base[code] = value;
        value += 1 << bits[code];
        code += 1;
    }
    base
}

/// Literal length baselines per code.
const LL_BASE: [u32; MAX_LL as usize + 1] = baselines(&LL_BITS, 0);
/// Match length baselines per code.
const ML_BASE: [u32; MAX_ML as usize + 1] = baselines(&ML_BITS, MIN_MATCH as u32);

// Every sequence symbol fits the `u8` symbols of an FSE table.
const _: () = assert!(MAX_LL <= MAX_SEQ && MAX_ML <= MAX_SEQ && MAX_OFF <= MAX_SEQ);

/// Static description of one symbol type's tables.
#[derive(Clone, Copy, Debug)]
struct SymbolKind {
    max_symbol: u32,
    max_log: u32,
    default: &'static [i16],
    default_log: u32,
}

const LITERAL_LENGTHS: SymbolKind = SymbolKind {
    max_symbol: MAX_LL,
    max_log: LL_FSE_LOG,
    default: &LL_DEFAULT_NORM,
    default_log: LL_DEFAULT_NORM_LOG,
};
const OFFSETS: SymbolKind = SymbolKind {
    max_symbol: MAX_OFF,
    max_log: OFF_FSE_LOG,
    default: &OF_DEFAULT_NORM,
    default_log: OF_DEFAULT_NORM_LOG,
};
const MATCH_LENGTHS: SymbolKind = SymbolKind {
    max_symbol: MAX_ML,
    max_log: ML_FSE_LOG,
    default: &ML_DEFAULT_NORM,
    default_log: ML_DEFAULT_NORM_LOG,
};

/// Where the history a block may reference starts, and its limits.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BlockContext {
    /// Index in the output of the frame's first retained byte.
    pub(crate) history_start: usize,
    /// The frame's window size: the largest permitted offset.
    pub(crate) window: u64,
    /// Largest regenerated block size.
    pub(crate) block_max: usize,
}

/// The decoding tables `Repeat_Mode` refers to, per frame.
#[derive(Debug, Default)]
pub(crate) struct SequenceTables {
    literal_lengths: Option<FseTable>,
    offsets: Option<FseTable>,
    match_lengths: Option<FseTable>,
}

/// Replaces `slot` as `mode` asks, returning the description's size.
fn update_table(
    slot: &mut Option<FseTable>,
    mode: EncodingType,
    kind: SymbolKind,
    src: &[u8],
) -> Result<usize, InvalidDataKind> {
    match mode {
        EncodingType::Basic => {
            *slot = Some(FseTable::from_distribution(kind.default, kind.default_log));
            Ok(0)
        }
        EncodingType::Rle => {
            let &symbol = src.first().ok_or(InvalidDataKind::SequencesSection)?;
            if u32::from(symbol) > kind.max_symbol {
                return Err(InvalidDataKind::SequencesSection);
            }
            *slot = Some(FseTable::rle(symbol));
            Ok(1)
        }
        EncodingType::Compressed => {
            let (distribution, used) = read_distribution(src, kind.max_symbol)?;
            if distribution.table_log > kind.max_log {
                return Err(InvalidDataKind::FseTable);
            }
            *slot = Some(FseTable::from_distribution(
                &distribution.counts,
                distribution.table_log,
            ));
            Ok(used)
        }
        EncodingType::Repeat => {
            if slot.is_none() {
                return Err(InvalidDataKind::SequencesSection);
            }
            Ok(0)
        }
    }
}

/// Appends a match of `length` bytes starting `offset` bytes back; the
/// source may overlap the bytes being written.
fn copy_match(out: &mut Vec<u8>, offset: usize, length: usize) {
    let start = out.len() - offset;
    let mut copied = 0;
    // `copied` stays a multiple of `offset` until the last step, so copying
    // from `start` repeats the period correctly while the source doubles.
    while copied < length {
        let step = (length - copied).min(offset + copied);
        out.extend_from_within(start..start + step);
        copied += step;
    }
}

impl SequenceTables {
    /// Heap bytes the tables hold.
    pub(crate) fn retained_bytes(&self) -> usize {
        [&self.literal_lengths, &self.offsets, &self.match_lengths]
            .into_iter()
            .flatten()
            .map(FseTable::retained_bytes)
            .sum()
    }

    /// Forgets every table at a frame start.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Parses the sequences section header, returning the number of
    /// sequences and the header's size.
    pub(crate) fn read_header(&mut self, src: &[u8]) -> Result<(usize, usize), InvalidDataKind> {
        let invalid = InvalidDataKind::SequencesSection;
        let &first = src.first().ok_or(invalid)?;
        let (count, mut used) = match first {
            0..128 => (usize::from(first), 1),
            128..255 => {
                let &second = src.get(1).ok_or(invalid)?;
                (((usize::from(first) - 0x80) << 8) + usize::from(second), 2)
            }
            255 => {
                let bytes = src.get(1..3).ok_or(invalid)?;
                let value = usize::from(u16::from_le_bytes([bytes[0], bytes[1]]));
                (value + LONG_NB_SEQ, 3)
            }
        };
        if count == 0 {
            // Nothing may follow an empty sequences section.
            return if used == src.len() {
                Ok((0, used))
            } else {
                Err(invalid)
            };
        }
        let &modes = src.get(used).ok_or(invalid)?;
        if modes & 3 != 0 {
            return Err(invalid);
        }
        used += 1;
        let rest = |used: usize| src.get(used..).unwrap_or(&[]);
        used += update_table(
            &mut self.literal_lengths,
            encoding_type(modes >> 6),
            LITERAL_LENGTHS,
            rest(used),
        )?;
        used += update_table(
            &mut self.offsets,
            encoding_type(modes >> 4),
            OFFSETS,
            rest(used),
        )?;
        used += update_table(
            &mut self.match_lengths,
            encoding_type(modes >> 2),
            MATCH_LENGTHS,
            rest(used),
        )?;
        Ok((count, used))
    }

    /// Decodes `count` sequences from `stream` and executes them, appending
    /// the block's output to `out`.
    ///
    /// `reps` holds the repeat offsets on entry and is updated only when the
    /// whole block succeeds.
    #[cfg_attr(all(feature = "hotpath", not(feature = "no_std")), hotpath::measure)]
    pub(crate) fn execute(
        &self,
        count: usize,
        stream: &[u8],
        literals: &[u8],
        reps: &mut [u32; REP_NUM],
        out: &mut Vec<u8>,
        context: BlockContext,
    ) -> Result<(), InvalidDataKind> {
        let missing = InvalidDataKind::SequencesSection;
        let ll_table = self.literal_lengths.as_ref().ok_or(missing)?;
        let of_table = self.offsets.as_ref().ok_or(missing)?;
        let ml_table = self.match_lengths.as_ref().ok_or(missing)?;
        let mut bits = ReverseBitReader::new(stream).ok_or(InvalidDataKind::SequenceStream)?;
        let mut ll_state = ll_table.initial_state(&mut bits);
        let mut of_state = of_table.initial_state(&mut bits);
        let mut ml_state = ml_table.initial_state(&mut bits);
        let mut repeat = *reps;
        let block_start = out.len();
        let mut remaining = literals;
        for index in 0..count {
            let ll_code = usize::from(ll_table.entry(ll_state).symbol);
            let of_code = u32::from(of_table.entry(of_state).symbol);
            let ml_code = usize::from(ml_table.entry(ml_state).symbol);
            let ll_zero = ll_code == 0;
            let offset = if of_code > 1 {
                let offset = (1u32 << of_code) - 3 + bits.read(of_code) as u32;
                repeat = [offset, repeat[0], repeat[1]];
                offset
            } else if of_code == 0 {
                if ll_zero {
                    repeat.swap(0, 1);
                }
                repeat[0]
            } else {
                let index = 1 + usize::from(ll_zero) + bits.read(1) as usize;
                let offset = if index == REP_NUM {
                    repeat[0] - 1
                } else {
                    repeat[index]
                };
                if offset == 0 {
                    return Err(InvalidDataKind::Offset);
                }
                if index != 1 {
                    repeat[2] = repeat[1];
                }
                repeat[1] = repeat[0];
                repeat[0] = offset;
                offset
            };
            let ml_base = ML_BASE.get(ml_code).ok_or(missing)?;
            let ml_bits = ML_BITS.get(ml_code).ok_or(missing)?;
            let match_length = (*ml_base + bits.read(u32::from(*ml_bits)) as u32) as usize;
            let ll_base = LL_BASE.get(ll_code).ok_or(missing)?;
            let ll_bits = LL_BITS.get(ll_code).ok_or(missing)?;
            let literal_length = (*ll_base + bits.read(u32::from(*ll_bits)) as u32) as usize;
            if index + 1 < count {
                ll_state = ll_table.next_state(ll_state, &mut bits);
                ml_state = ml_table.next_state(ml_state, &mut bits);
                of_state = of_table.next_state(of_state, &mut bits);
            }

            let (copy, rest) = remaining
                .split_at_checked(literal_length)
                .ok_or(InvalidDataKind::LiteralsOverrun)?;
            if out.len() - block_start + literal_length + match_length > context.block_max {
                return Err(InvalidDataKind::BlockSize);
            }
            out.extend_from_slice(copy);
            remaining = rest;
            let history = out.len() - context.history_start;
            let offset = offset as usize;
            if offset > history || offset as u64 > context.window {
                return Err(InvalidDataKind::Offset);
            }
            copy_match(out, offset, match_length);
        }
        if !bits.is_exhausted() {
            return Err(InvalidDataKind::SequenceStream);
        }
        if out.len() - block_start + remaining.len() > context.block_max {
            return Err(InvalidDataKind::BlockSize);
        }
        out.extend_from_slice(remaining);
        *reps = repeat;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::format::REP_START;

    const CONTEXT: BlockContext = BlockContext {
        history_start: 0,
        window: 1 << 20,
        block_max: 1 << 17,
    };

    #[test]
    fn baselines_match_the_rfc_tables() {
        // The tables are computed at compile time; recompute them at run
        // time too.
        assert_eq!(baselines(&LL_BITS, 0), LL_BASE);
        assert_eq!(baselines(&ML_BITS, 3), ML_BASE);
        assert_eq!(
            &LL_BASE[..16],
            &::core::array::from_fn::<u32, 16, _>(|i| i as u32)
        );
        assert_eq!(
            &LL_BASE[16..],
            &[
                16, 18, 20, 22, 24, 28, 32, 40, 48, 64, 128, 256, 512, 1024, 2048, 4096, 8192,
                16384, 32768, 65536
            ]
        );
        assert_eq!(ML_BASE[0], 3);
        assert_eq!(ML_BASE[31], 34);
        assert_eq!(
            &ML_BASE[32..],
            &[
                35, 37, 39, 41, 43, 47, 51, 59, 67, 83, 99, 131, 259, 515, 1027, 2051, 4099, 8195,
                16387, 32771, 65539
            ]
        );
    }

    #[test]
    fn overlapping_matches_repeat_their_period() {
        let mut out = b"abc".to_vec();
        copy_match(&mut out, 3, 8);
        assert_eq!(out, b"abcabcabcab");
        let mut out = b"x".to_vec();
        copy_match(&mut out, 1, 5);
        assert_eq!(out, b"xxxxxx");
        let mut out = b"hello".to_vec();
        copy_match(&mut out, 5, 2);
        assert_eq!(out, b"hellohe");
    }

    #[test]
    fn sequence_counts_use_one_two_or_three_bytes() {
        let mut tables = SequenceTables::default();
        assert_eq!(tables.read_header(&[0]), Ok((0, 1)));
        assert_eq!(
            tables.read_header(&[0, 0]),
            Err(InvalidDataKind::SequencesSection)
        );
        // 0x81 0x02 → 0x102 sequences, predefined modes.
        assert_eq!(tables.read_header(&[0x81, 0x02, 0x00]), Ok((0x102, 3)));
        assert_eq!(
            tables.read_header(&[0xff, 0x01, 0x00, 0x00]),
            Ok((LONG_NB_SEQ + 1, 4))
        );
        assert_eq!(
            tables.read_header(&[0xff, 0x01]),
            Err(InvalidDataKind::SequencesSection)
        );
        assert_eq!(
            tables.read_header(&[0x81]),
            Err(InvalidDataKind::SequencesSection)
        );
        assert_eq!(
            tables.read_header(&[1]),
            Err(InvalidDataKind::SequencesSection)
        );
    }

    #[test]
    fn reserved_mode_bits_and_bad_tables_are_rejected() {
        let mut tables = SequenceTables::default();
        assert_eq!(
            tables.read_header(&[1, 0x01]),
            Err(InvalidDataKind::SequencesSection)
        );
        // Repeat mode before any table.
        assert_eq!(
            tables.read_header(&[1, 0xc0]),
            Err(InvalidDataKind::SequencesSection)
        );
        // RLE literal length symbol above 35, and a missing RLE byte.
        assert_eq!(
            tables.read_header(&[1, 0x40, 36]),
            Err(InvalidDataKind::SequencesSection)
        );
        assert_eq!(
            tables.read_header(&[1, 0x40]),
            Err(InvalidDataKind::SequencesSection)
        );
        // An offset table with accuracy log 9 (above 8): low nibble 4.
        assert_eq!(
            tables.read_header(&[1, 0x20, 0x04, 0, 0, 0]),
            Err(InvalidDataKind::FseTable)
        );
        // Once a block set the tables, repeat mode reuses them.
        assert_eq!(tables.read_header(&[1, 0x54, 1, 2, 3]), Ok((1, 5)));
        assert_eq!(tables.read_header(&[1, 0xfc]), Ok((1, 2)));
    }

    /// Writes a sequence bitstream: fields are listed in the order the
    /// decoder reads them, and the marker bit goes on top.
    fn backward_stream(fields: &[(u64, u32)]) -> Vec<u8> {
        let total: u32 = fields.iter().map(|&(_, width)| width).sum();
        let mut value: u128 = 1;
        for &(field, width) in fields {
            value = (value << width) | u128::from(field);
        }
        let bytes = (total as usize + 1).div_ceil(8);
        (0..bytes).map(|i| (value >> (8 * i)) as u8).collect()
    }

    /// RLE tables: literal length code `ll`, offset code `of`, match length
    /// code `ml`.
    fn rle_tables(ll: u8, of: u8, ml: u8) -> SequenceTables {
        let mut tables = SequenceTables::default();
        tables
            .read_header(&[1, 0x54, ll, of, ml])
            .expect("valid RLE tables");
        tables
    }

    #[test]
    fn a_literal_run_and_an_overlapping_match_execute() {
        // Literal length 3, offset code 2 with extra bits 2 (offset value
        // 4 + 2 = 6, offset 3), match length code 2 (5 bytes).
        let tables = rle_tables(3, 2, 2);
        let stream = backward_stream(&[(2, 2)]);
        let mut reps = REP_START;
        let mut out = Vec::new();
        tables
            .execute(1, &stream, b"abcXY", &mut reps, &mut out, CONTEXT)
            .expect("valid");
        assert_eq!(out, b"abcabcabXY");
        assert_eq!(reps, [3, 1, 4]);
    }

    #[test]
    fn repeat_offsets_follow_the_update_rules() {
        // Offset code 1 reads one bit: index 1 + bit, or 2 + bit when the
        // literal length is zero.
        let mut out = b"0123456789".to_vec();
        let context = BlockContext {
            history_start: 0,
            ..CONTEXT
        };
        // Literal length 1, offset code 1 with bit 1 → Repeated_Offset3 = 8.
        let tables = rle_tables(1, 1, 0);
        let mut reps = REP_START;
        tables
            .execute(
                1,
                &backward_stream(&[(1, 1)]),
                b"z",
                &mut reps,
                &mut out,
                context,
            )
            .expect("valid");
        assert_eq!(reps, [8, 1, 4]);
        // Literal length 0, offset code 0 → Repeated_Offset2.
        let tables = rle_tables(0, 0, 0);
        tables
            .execute(1, &[0x01], b"", &mut reps, &mut out, context)
            .expect("valid");
        assert_eq!(reps, [1, 8, 4]);
        // Literal length 0, offset code 1 with bit 1 → Repeated_Offset1 - 1
        // = 0, which is corrupt.
        let tables = rle_tables(0, 1, 0);
        assert_eq!(
            tables.execute(
                1,
                &backward_stream(&[(1, 1)]),
                b"",
                &mut reps,
                &mut out,
                context
            ),
            Err(InvalidDataKind::Offset)
        );
        // Literal length 0, offset code 1 with bit 0 → Repeated_Offset3.
        tables
            .execute(
                1,
                &backward_stream(&[(0, 1)]),
                b"",
                &mut reps,
                &mut out,
                context,
            )
            .expect("valid");
        assert_eq!(reps, [4, 1, 8]);
        // Offset code 0 with literals keeps the order.
        let tables = rle_tables(1, 0, 0);
        tables
            .execute(1, &[0x01], b"q", &mut reps, &mut out, context)
            .expect("valid");
        assert_eq!(reps, [4, 1, 8]);
    }

    #[test]
    fn invalid_sequences_are_rejected() {
        let mut reps = REP_START;
        let mut out = Vec::new();
        // Offset 3 with no history.
        let tables = rle_tables(0, 2, 0);
        assert_eq!(
            tables.execute(
                1,
                &backward_stream(&[(2, 2)]),
                b"",
                &mut reps,
                &mut out,
                CONTEXT
            ),
            Err(InvalidDataKind::Offset)
        );
        // More literals than the section holds.
        let tables = rle_tables(3, 0, 0);
        assert_eq!(
            tables.execute(1, &[0x01], b"ab", &mut reps, &mut out, CONTEXT),
            Err(InvalidDataKind::LiteralsOverrun)
        );
        // Leftover bits, and a missing end marker.
        assert_eq!(
            tables.execute(1, &[0x03], b"abc", &mut reps, &mut out, CONTEXT),
            Err(InvalidDataKind::SequenceStream)
        );
        assert_eq!(
            tables.execute(1, &[0x00], b"abc", &mut reps, &mut out, CONTEXT),
            Err(InvalidDataKind::SequenceStream)
        );
        // A block larger than its maximum.
        let small = BlockContext {
            block_max: 2,
            ..CONTEXT
        };
        assert_eq!(
            tables.execute(1, &[0x01], b"abc", &mut reps, &mut out, small),
            Err(InvalidDataKind::BlockSize)
        );
        let tables = rle_tables(0, 0, 0);
        assert_eq!(
            tables.execute(1, &[0x01], b"abc", &mut reps, &mut b"x".to_vec(), small),
            Err(InvalidDataKind::BlockSize)
        );
        // Offsets beyond the window.
        let narrow = BlockContext {
            window: 0,
            ..CONTEXT
        };
        assert_eq!(
            tables.execute(1, &[0x01], b"", &mut reps, &mut b"abcdef".to_vec(), narrow),
            Err(InvalidDataKind::Offset)
        );
        assert_eq!(reps, REP_START);
        // Tables are required.
        assert_eq!(
            SequenceTables::default().execute(1, &[0x01], b"", &mut reps, &mut out, CONTEXT),
            Err(InvalidDataKind::SequencesSection)
        );
    }

    #[test]
    fn reset_forgets_repeat_tables() {
        let mut tables = rle_tables(0, 0, 0);
        tables.reset();
        assert_eq!(
            tables.read_header(&[1, 0xfc]),
            Err(InvalidDataKind::SequencesSection)
        );
    }
}
