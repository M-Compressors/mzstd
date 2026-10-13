//! FSE (tANS) table descriptions, decoding tables and the two-state weight
//! decoder used by Huffman tree descriptions.
//!
//! [`read_distribution`] parses a normalized distribution (RFC 8878
//! section 4.1.1), [`FseTable::from_distribution`] spreads it into a decoding
//! table (section 4.1.1.1, Appendix A), and [`decode_weights`] runs the
//! interleaved two-state decoder over a Huffman weight stream
//! (section 4.2.1.2).

use alloc::vec::Vec;

use super::bits::{ForwardBitReader, ReverseBitReader};
use crate::decompressor::InvalidDataKind;
use crate::shared::format::{MAX_LIT, highbit32};

/// Smallest accuracy log a table description can declare.
const MIN_TABLE_LOG: u32 = 5;
/// Largest accuracy log a table description can encode.
const MAX_TABLE_LOG_DESCRIBED: u32 = 15;
/// Largest accuracy log of a Huffman weight table.
const WEIGHT_TABLE_LOG_MAX: u32 = 6;
/// Weights a compressed Huffman description can produce: every literal but
/// the implied last one.
pub(crate) const MAX_WEIGHTS: usize = MAX_LIT as usize;

/// One decoding-table row.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct FseEntry {
    /// Decoded symbol.
    pub(crate) symbol: u8,
    /// Bits to read for the next state.
    pub(crate) nb_bits: u8,
    /// Base of the next state.
    pub(crate) new_state: u16,
}

/// A normalized distribution: `-1` marks a "less than one" probability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Distribution {
    /// Probabilities of symbols `0..counts.len()`.
    pub(crate) counts: Vec<i16>,
    /// Accuracy log.
    pub(crate) table_log: u32,
}

/// An FSE decoding table of `1 << log` rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FseTable {
    log: u32,
    entries: Vec<FseEntry>,
}

/// Reads a normalized distribution describing symbols `0..=max_symbol`.
///
/// Returns the distribution and the number of bytes it occupies. As in the
/// reference decoder, the probabilities must sum exactly to the table size
/// and no symbol above `max_symbol` may appear.
pub(crate) fn read_distribution(
    src: &[u8],
    max_symbol: u32,
) -> Result<(Distribution, usize), InvalidDataKind> {
    let invalid = InvalidDataKind::FseTable;
    let symbol_limit = max_symbol as usize + 1;
    let mut bits = ForwardBitReader::new(src);
    let table_log = bits.read(4) + MIN_TABLE_LOG;
    if table_log > MAX_TABLE_LOG_DESCRIBED {
        return Err(invalid);
    }
    let mut counts = Vec::with_capacity(symbol_limit);
    let mut remaining: i32 = (1 << table_log) + 1;
    let mut threshold: i32 = 1 << table_log;
    let mut nb_bits = table_log + 1;
    let mut previous_zero = false;
    loop {
        if previous_zero {
            // Two-bit repeat flags; a value of 3 continues the run.
            loop {
                let repeat = bits.read(2) as usize;
                counts.resize(counts.len() + repeat, 0);
                if repeat != 3 || counts.len() >= symbol_limit {
                    break;
                }
            }
            if counts.len() >= symbol_limit {
                break;
            }
        }
        let max = (2 * threshold - 1) - remaining;
        let low = bits.peek(nb_bits - 1) as i32;
        let mut value = if low < max {
            bits.consume(nb_bits - 1);
            low
        } else {
            let full = bits.peek(nb_bits) as i32;
            bits.consume(nb_bits);
            if full >= threshold { full - max } else { full }
        };
        value -= 1;
        remaining -= value.abs();
        counts.push(value as i16);
        previous_zero = value == 0;
        if remaining < threshold {
            if remaining <= 1 {
                break;
            }
            nb_bits = highbit32(remaining as u32) + 1;
            threshold = 1 << (nb_bits - 1);
        }
        if counts.len() >= symbol_limit {
            break;
        }
    }
    if remaining != 1 || counts.len() > symbol_limit {
        return Err(invalid);
    }
    let used = bits.bytes_used();
    if used > src.len() {
        return Err(invalid);
    }
    Ok((Distribution { counts, table_log }, used))
}

impl FseTable {
    /// Builds the decoding table of a distribution whose probabilities sum
    /// to `1 << table_log`.
    pub(crate) fn from_distribution(counts: &[i16], table_log: u32) -> Self {
        let size = 1usize << table_log;
        let mut entries = alloc::vec![FseEntry::default(); size];
        let mut next = [0u16; 256];
        // "Less than one" symbols take single rows from the end.
        let mut high = size;
        for (symbol, &count) in counts.iter().enumerate() {
            if count == -1 {
                high -= 1;
                entries[high].symbol = symbol as u8;
                next[symbol] = 1;
            } else {
                next[symbol] = count.max(0) as u16;
            }
        }
        let step = (size >> 1) + (size >> 3) + 3;
        let mask = size - 1;
        let mut position = 0;
        for (symbol, &count) in counts.iter().enumerate() {
            for _ in 0..count.max(0) {
                entries[position].symbol = symbol as u8;
                position = (position + step) & mask;
                while position >= high {
                    position = (position + step) & mask;
                }
            }
        }
        for entry in &mut entries {
            let next_state = &mut next[usize::from(entry.symbol)];
            // Every placed symbol has a non-zero count; `max(1)` only keeps
            // the arithmetic total for a table nothing else would accept.
            let state = u32::from(*next_state).max(1);
            let nb_bits = table_log - highbit32(state);
            entry.nb_bits = nb_bits as u8;
            entry.new_state = ((state << nb_bits) - size as u32) as u16;
            *next_state += 1;
        }
        Self {
            log: table_log,
            entries,
        }
    }

    /// A one-row table that always yields `symbol` and reads no bits.
    pub(crate) fn rle(symbol: u8) -> Self {
        Self {
            log: 0,
            entries: alloc::vec![FseEntry {
                symbol,
                nb_bits: 0,
                new_state: 0,
            }],
        }
    }

    /// Heap bytes the table holds.
    pub(crate) fn retained_bytes(&self) -> usize {
        self.entries.capacity() * size_of::<FseEntry>()
    }

    /// Accuracy log: the bits an initial state takes.
    pub(crate) const fn log(&self) -> u32 {
        self.log
    }

    /// The row for `state`. States come from this table's own transitions,
    /// so they are always in range; an out-of-range state yields row zero.
    pub(crate) fn entry(&self, state: usize) -> FseEntry {
        self.entries.get(state).copied().unwrap_or_default()
    }

    /// Reads an initial state.
    pub(crate) fn initial_state(&self, bits: &mut ReverseBitReader<'_>) -> usize {
        bits.read(self.log()) as usize
    }

    /// Advances `state` past the symbol it decoded.
    pub(crate) fn next_state(&self, state: usize, bits: &mut ReverseBitReader<'_>) -> usize {
        let entry = self.entry(state);
        usize::from(entry.new_state) + bits.read(u32::from(entry.nb_bits)) as usize
    }
}

/// Decodes an FSE-compressed Huffman weight series into `weights`, returning
/// how many weights it holds.
///
/// Two states share one table and alternate; decoding stops once a state
/// update reads past the stream start, after emitting the other state's
/// symbol, exactly as the reference `FSE_decompress_wksp` does.
pub(crate) fn decode_weights(
    src: &[u8],
    weights: &mut [u8; MAX_WEIGHTS + 1],
) -> Result<usize, InvalidDataKind> {
    let invalid = InvalidDataKind::HuffmanTable;
    let (distribution, used) = read_distribution(src, MAX_LIT).map_err(|_| invalid)?;
    if distribution.table_log > WEIGHT_TABLE_LOG_MAX {
        return Err(invalid);
    }
    let table = FseTable::from_distribution(&distribution.counts, distribution.table_log);
    let stream = src.get(used..).ok_or(invalid)?;
    let mut bits = ReverseBitReader::new(stream).ok_or(invalid)?;
    let mut states = [
        table.initial_state(&mut bits),
        table.initial_state(&mut bits),
    ];
    if bits.overflowed() {
        return Err(invalid);
    }
    let capacity = MAX_WEIGHTS;
    let mut count = 0;
    let mut turn = 0;
    loop {
        if count + 2 > capacity {
            return Err(invalid);
        }
        weights[count] = table.entry(states[turn]).symbol;
        count += 1;
        states[turn] = table.next_state(states[turn], &mut bits);
        if bits.overflowed() {
            weights[count] = table.entry(states[1 - turn]).symbol;
            return Ok(count + 1);
        }
        turn = 1 - turn;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::format::{
        LL_DEFAULT_NORM, LL_DEFAULT_NORM_LOG, ML_DEFAULT_NORM, ML_DEFAULT_NORM_LOG,
        OF_DEFAULT_NORM, OF_DEFAULT_NORM_LOG,
    };

    /// Writes `(value, width)` fields forward, little-endian.
    fn pack(fields: &[(u32, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut acc = 0u64;
        let mut filled = 0;
        for &(value, width) in fields {
            acc |= u64::from(value) << filled;
            filled += width;
            while filled >= 8 {
                out.push(acc as u8);
                acc >>= 8;
                filled -= 8;
            }
        }
        if filled > 0 {
            out.push(acc as u8);
        }
        out
    }

    #[test]
    fn a_two_symbol_distribution_round_trips() {
        // Accuracy log 5: probabilities 16 and 16, written as values 17 and 17.
        // First field: remaining 33, threshold 32, nbBits 6, max = 63 - 33 = 30;
        // 17 < 30 so 5 bits. Then remaining = 17, threshold 16, nbBits 5,
        // max = 31 - 17 = 14; 17 >= 14 needs the 5-bit form 17 + 14 = 31.
        let src = pack(&[(0, 4), (17, 5), (31, 5)]);
        let (distribution, used) = read_distribution(&src, 1).expect("valid");
        assert_eq!(distribution.table_log, 5);
        assert_eq!(distribution.counts, [16, 16]);
        assert_eq!(used, 2);
    }

    #[test]
    fn zero_runs_use_repeat_flags() {
        // Log 5: symbol 0 has probability 0 (value 1, five bits), then
        // repeat flags 3 (three more zeros, continue) and 1 (one more), then
        // symbol 5 takes all 32 rows: value 33 is at least the threshold,
        // so it is stored as 33 + max (30) = 63 in six bits.
        let src = pack(&[(0, 4), (1, 5), (3, 2), (1, 2), (63, 6)]);
        let (distribution, _) = read_distribution(&src, 8).expect("valid");
        assert_eq!(distribution.counts, [0, 0, 0, 0, 0, 32]);
    }

    #[test]
    fn oversized_logs_and_overshooting_symbols_are_rejected() {
        assert_eq!(
            read_distribution(&[0x0b, 0, 0, 0], 35),
            Err(InvalidDataKind::FseTable)
        );
        // A run of zeros past the symbol limit.
        let src = pack(&[(0, 4), (1, 5), (3, 2), (3, 2), (3, 2), (0, 2)]);
        assert_eq!(read_distribution(&src, 3), Err(InvalidDataKind::FseTable));
        // Too few bytes for the declared probabilities.
        assert_eq!(
            read_distribution(&[0x00], 35),
            Err(InvalidDataKind::FseTable)
        );
    }

    /// Rows `(symbol, nb_bits, base)` of an RFC 8878 Appendix A table.
    fn appendix_a(heading: &str) -> Vec<(u8, u8, u16)> {
        let spec = include_str!("../../../zstd-ffi/vendor/zstd/doc/zstd_compression_format.md");
        let section = spec.split(heading).nth(1).expect("heading present");
        section
            .lines()
            .skip_while(|line| !line.starts_with("| ---"))
            .skip(1)
            .take_while(|line| line.starts_with('|'))
            .map(|line| {
                let cells: Vec<u16> = line
                    .split('|')
                    .filter_map(|cell| cell.trim().parse().ok())
                    .collect();
                (cells[1] as u8, cells[2] as u8, cells[3])
            })
            .collect()
    }

    #[test]
    fn predefined_tables_match_appendix_a() {
        let cases: [(&str, &[i16], u32); 3] = [
            (
                "#### Literal Length Code:",
                &LL_DEFAULT_NORM,
                LL_DEFAULT_NORM_LOG,
            ),
            (
                "#### Match Length Code:",
                &ML_DEFAULT_NORM,
                ML_DEFAULT_NORM_LOG,
            ),
            ("#### Offset Code:", &OF_DEFAULT_NORM, OF_DEFAULT_NORM_LOG),
        ];
        for (heading, norm, log) in cases {
            let table = FseTable::from_distribution(norm, log);
            let expected = appendix_a(heading);
            assert_eq!(expected.len(), 1 << log, "{heading}");
            for (state, &(symbol, nb_bits, new_state)) in expected.iter().enumerate() {
                assert_eq!(
                    table.entry(state),
                    FseEntry {
                        symbol,
                        nb_bits,
                        new_state
                    },
                    "{heading} state {state}"
                );
            }
        }
    }

    #[test]
    fn an_rle_table_reads_no_bits() {
        let table = FseTable::rle(7);
        let mut bits = ReverseBitReader::new(&[0x01]).expect("terminated");
        let state = table.initial_state(&mut bits);
        assert_eq!(table.entry(state).symbol, 7);
        assert_eq!(table.next_state(state, &mut bits), 0);
        assert!(bits.is_exhausted());
        assert_eq!(table.entry(99), FseEntry::default());
    }

    #[test]
    fn weight_streams_without_room_or_states_fail() {
        let mut weights = [0u8; MAX_WEIGHTS + 1];
        // No bitstream after the distribution.
        let distribution = pack(&[(0, 4), (17, 5), (31, 5)]);
        assert_eq!(
            decode_weights(&distribution, &mut weights),
            Err(InvalidDataKind::HuffmanTable)
        );
        // The initial states need ten bits; a one-byte stream has at most 7.
        let mut src = distribution.clone();
        src.push(0x80);
        assert_eq!(
            decode_weights(&src, &mut weights),
            Err(InvalidDataKind::HuffmanTable)
        );
        // Log 7 is too large for weights.
        let mut wide = pack(&[(2, 4), (65, 7), (127, 7)]);
        wide.extend_from_slice(&[0xff, 0xff, 0xff]);
        assert_eq!(
            decode_weights(&wide, &mut weights),
            Err(InvalidDataKind::HuffmanTable)
        );
    }

    #[test]
    fn two_states_alternate_until_the_stream_overflows() {
        let mut weights = [0u8; MAX_WEIGHTS + 1];
        // Symbols 0 and 1 with 16 rows each, so every row reads one bit.
        // The stream holds the two 5-bit initial states (both zero) and two
        // more bits: the first two updates fit, the third overflows and
        // ends the series after the other state's symbol.
        let mut src = pack(&[(0, 4), (17, 5), (31, 5)]);
        src.extend_from_slice(&[0b0000_0001, 0b0001_0000]);
        let count = decode_weights(&src, &mut weights).expect("decodes");
        assert_eq!(count, 4);
        assert!(weights[..count].iter().all(|&w| w <= 1));
    }
}
