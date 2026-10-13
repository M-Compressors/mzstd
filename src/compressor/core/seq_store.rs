//! The sequence store: what a match finder produces for one block
//! (`SeqStore_t`, `ZSTD_storeSeq` and the repeat-offset helpers).
//!
//! Offsets travel as the reference's "offBase" sum type: `1..=3` name a
//! repeat offset, anything larger is a literal offset plus 3.

use crate::shared::format::{MIN_MATCH, REP_NUM, highbit32};

/// Largest literal length code.
const MAX_LL: u8 = 35;
/// Largest match length code.
const MAX_ML: u8 = 52;

/// One sequence (`SeqDef`).
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SeqDef {
    /// Offset sum type: repeat offset `1..=3`, or offset plus 3.
    pub(crate) off_base: u32,
    /// Literal length, modulo `0x10000` (see [`LongLength`]).
    pub(crate) lit_length: u16,
    /// Match length minus [`MIN_MATCH`], modulo `0x10000`.
    pub(crate) ml_base: u16,
}

/// Which length, if any, of one sequence exceeds 16 bits
/// (`ZSTD_longLengthType_e`).
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum LongLength {
    #[default]
    None,
    /// The literal length at `pos` carries an extra `0x10000`.
    Literal(u32),
    /// The match length at `pos` carries an extra `0x10000`.
    Match(u32),
}

/// `REPCODE_TO_OFFBASE`.
pub(crate) const fn repcode_to_offbase(repcode: u32) -> u32 {
    repcode
}

/// `OFFSET_TO_OFFBASE`.
pub(crate) const fn offset_to_offbase(offset: u32) -> u32 {
    offset + REP_NUM as u32
}

/// `OFFBASE_IS_REPCODE`.
pub(crate) const fn offbase_is_repcode(off_base: u32) -> bool {
    off_base >= 1 && off_base <= REP_NUM as u32
}

/// `OFFBASE_TO_OFFSET`.
pub(crate) const fn offbase_to_offset(off_base: u32) -> u32 {
    off_base - REP_NUM as u32
}

/// Updates repeat offsets after a sequence (`ZSTD_updateRep`).
pub(crate) const fn update_rep(rep: &mut [u32; REP_NUM], off_base: u32, ll0: bool) {
    if off_base > REP_NUM as u32 {
        rep[2] = rep[1];
        rep[1] = rep[0];
        rep[0] = offbase_to_offset(off_base);
    } else {
        let rep_code = off_base - 1 + ll0 as u32;
        if rep_code > 0 {
            let current = if rep_code == REP_NUM as u32 {
                rep[0].wrapping_sub(1)
            } else {
                rep[rep_code as usize]
            };
            rep[2] = if rep_code >= 2 { rep[1] } else { rep[2] };
            rep[1] = rep[0];
            rep[0] = current;
        }
    }
}

/// Repeat offsets after a sequence, leaving `rep` unchanged (`ZSTD_newRep`).
pub(crate) const fn new_rep(rep: &[u32; REP_NUM], off_base: u32, ll0: bool) -> [u32; REP_NUM] {
    let mut next = *rep;
    update_rep(&mut next, off_base, ll0);
    next
}

/// `ZSTD_LLcode`.
pub(crate) const fn ll_code(lit_length: u32) -> u8 {
    const LL_CODE: [u8; 64] = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 16, 17, 17, 18, 18, 19, 19, 20,
        20, 20, 20, 21, 21, 21, 21, 22, 22, 22, 22, 22, 22, 22, 22, 23, 23, 23, 23, 23, 23, 23, 23,
        24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
    ];
    if lit_length > 63 {
        (highbit32(lit_length) + 19) as u8
    } else {
        LL_CODE[lit_length as usize]
    }
}

/// `ZSTD_MLcode`, from `matchLength - MINMATCH`.
pub(crate) const fn ml_code(ml_base: u32) -> u8 {
    const ML_CODE: [u8; 128] = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26, 27, 28, 29, 30, 31, 32, 32, 33, 33, 34, 34, 35, 35, 36, 36, 36, 36, 37, 37, 37, 37,
        38, 38, 38, 38, 38, 38, 38, 38, 39, 39, 39, 39, 39, 39, 39, 39, 40, 40, 40, 40, 40, 40, 40,
        40, 40, 40, 40, 40, 40, 40, 40, 40, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41, 41,
        41, 41, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42,
        42, 42, 42, 42, 42, 42, 42, 42, 42, 42, 42,
    ];
    if ml_base > 127 {
        (highbit32(ml_base) + 36) as u8
    } else {
        ML_CODE[ml_base as usize]
    }
}

/// The literal and match lengths of one sequence
/// (`ZSTD_getSequenceLength`).
#[cfg(test)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct SequenceLength {
    pub(crate) lit_length: u32,
    pub(crate) match_length: u32,
}

/// Sequences and literals of one block.
#[derive(Clone, Debug, Default)]
pub(crate) struct SeqStore {
    /// Sequences in order.
    pub(crate) sequences: alloc::vec::Vec<SeqDef>,
    /// Literals of every sequence and the block's trailing literals.
    pub(crate) literals: alloc::vec::Vec<u8>,
    /// Per-sequence codes, filled by [`SeqStore::fill_codes`].
    pub(crate) ll_code: alloc::vec::Vec<u8>,
    pub(crate) ml_code: alloc::vec::Vec<u8>,
    pub(crate) of_code: alloc::vec::Vec<u8>,
    /// The single sequence whose length exceeds 16 bits, if any.
    pub(crate) long_length: LongLength,
    /// Capacity in sequences (`maxNbSeq`).
    pub(crate) max_nb_seq: usize,
    /// Capacity in literals (`maxNbLit`).
    pub(crate) max_nb_lit: usize,
}

impl SeqStore {
    /// Sizes the store for blocks of `block_size` bytes with matches of at
    /// least `min_match` bytes (`ZSTD_maxNbSeq`).
    pub(crate) fn reserve(&mut self, block_size: usize, min_match: u32) {
        let divider = if min_match == 3 { 3 } else { 4 };
        self.max_nb_seq = block_size / divider;
        self.max_nb_lit = block_size;
        self.sequences.reserve(self.max_nb_seq);
        // Room for the 16-byte chunk `store_seq_from` writes past the end.
        self.literals.reserve(self.max_nb_lit + 16);
    }

    /// Bytes held between blocks.
    pub(crate) fn retained_bytes(&self) -> usize {
        self.sequences.capacity() * core::mem::size_of::<SeqDef>()
            + self.literals.capacity()
            + self.ll_code.capacity()
            + self.ml_code.capacity()
            + self.of_code.capacity()
    }

    /// Empties the store for a new block (`ZSTD_resetSeqStore`).
    pub(crate) fn reset(&mut self) {
        self.sequences.clear();
        self.literals.clear();
        self.long_length = LongLength::None;
    }

    /// Number of sequences.
    pub(crate) fn nb_seq(&self) -> usize {
        self.sequences.len()
    }

    /// Appends a sequence without its literals (`ZSTD_storeSeqOnly`).
    #[inline(always)]
    pub(crate) fn store_seq_only(&mut self, lit_length: usize, off_base: u32, match_length: usize) {
        debug_assert!(self.sequences.len() < self.max_nb_seq);
        debug_assert!(match_length >= MIN_MATCH);
        let pos = self.sequences.len() as u32;
        if lit_length > 0xFFFF {
            debug_assert_eq!(self.long_length, LongLength::None);
            self.long_length = LongLength::Literal(pos);
        }
        let ml_base = match_length - MIN_MATCH;
        if ml_base > 0xFFFF {
            debug_assert_eq!(self.long_length, LongLength::None);
            self.long_length = LongLength::Match(pos);
        }
        self.sequences.push(SeqDef {
            off_base,
            lit_length: lit_length as u16,
            ml_base: ml_base as u16,
        });
    }

    /// Appends a sequence and its literals (`ZSTD_storeSeq`).
    pub(crate) fn store_seq(&mut self, literals: &[u8], off_base: u32, match_length: usize) {
        debug_assert!(self.literals.len() + literals.len() <= self.max_nb_lit);
        self.literals.extend_from_slice(literals);
        self.store_seq_only(literals.len(), off_base, match_length);
    }

    /// [`Self::store_seq`] for the literals `src[anchor..anchor + lit_length]`,
    /// copied as `ZSTD_storeSeq` does: a run of at most 16 bytes is copied as
    /// one fixed 16-byte chunk (`ZSTD_copy16`) when `src` holds that many,
    /// and the excess is cut off again, so short runs cost no length-driven
    /// copy.
    #[inline(always)]
    pub(crate) fn store_seq_from(
        &mut self,
        src: &[u8],
        anchor: usize,
        lit_length: usize,
        off_base: u32,
        match_length: usize,
    ) {
        debug_assert!(self.literals.len() + lit_length <= self.max_nb_lit);
        match src.get(anchor..anchor + 16) {
            Some(chunk) if lit_length <= 16 => {
                let start = self.literals.len();
                let mut wild = [0u8; 16];
                wild.copy_from_slice(chunk);
                self.literals.extend_from_slice(&wild);
                self.literals.truncate(start + lit_length);
            }
            _ => self
                .literals
                .extend_from_slice(&src[anchor..anchor + lit_length]),
        }
        self.store_seq_only(lit_length, off_base, match_length);
    }

    /// Appends the block's trailing literals (`ZSTD_storeLastLiterals`).
    pub(crate) fn store_last_literals(&mut self, literals: &[u8]) {
        self.literals.extend_from_slice(literals);
    }

    /// Fills the code arrays (`ZSTD_seqToCodes`).
    pub(crate) fn fill_codes(&mut self) {
        let n = self.sequences.len();
        self.ll_code.resize(n, 0);
        self.ml_code.resize(n, 0);
        self.of_code.resize(n, 0);
        let codes = self
            .ll_code
            .iter_mut()
            .zip(self.of_code.iter_mut())
            .zip(self.ml_code.iter_mut());
        for (seq, ((ll, of), ml)) in self.sequences.iter().zip(codes) {
            *ll = ll_code(u32::from(seq.lit_length));
            *of = highbit32(seq.off_base) as u8;
            *ml = ml_code(u32::from(seq.ml_base));
        }
        match self.long_length {
            LongLength::Literal(pos) if (pos as usize) < n => self.ll_code[pos as usize] = MAX_LL,
            LongLength::Match(pos) if (pos as usize) < n => self.ml_code[pos as usize] = MAX_ML,
            _ => {}
        }
    }

    /// The full lengths of sequence `index` (`ZSTD_getSequenceLength`).
    #[cfg(test)]
    pub(crate) fn sequence_length(&self, index: usize) -> SequenceLength {
        let seq = self.sequences[index];
        let mut lengths = SequenceLength {
            lit_length: u32::from(seq.lit_length),
            match_length: u32::from(seq.ml_base) + MIN_MATCH as u32,
        };
        match self.long_length {
            LongLength::Literal(pos) if pos as usize == index => lengths.lit_length += 0x10000,
            LongLength::Match(pos) if pos as usize == index => lengths.match_length += 0x10000,
            _ => {}
        }
        lengths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_offsets_follow_the_format_rules() {
        let mut rep = [1, 4, 8];
        update_rep(&mut rep, offset_to_offbase(100), false);
        assert_eq!(rep, [100, 1, 4]);
        update_rep(&mut rep, repcode_to_offbase(2), false);
        assert_eq!(rep, [1, 100, 4]);
        update_rep(&mut rep, repcode_to_offbase(1), false);
        assert_eq!(rep, [1, 100, 4]);
        update_rep(&mut rep, repcode_to_offbase(3), true);
        assert_eq!(rep, [0, 1, 100]);
        assert_eq!(new_rep(&[5, 6, 7], repcode_to_offbase(1), true), [6, 5, 7]);
        assert!(offbase_is_repcode(3) && !offbase_is_repcode(4));
    }

    #[test]
    fn length_codes_match_the_format_tables() {
        assert_eq!(ll_code(0), 0);
        assert_eq!(ll_code(16), 16);
        assert_eq!(ll_code(64), 25);
        assert_eq!(ll_code(65535), 34);
        assert_eq!(ml_code(0), 0);
        assert_eq!(ml_code(127), 42);
        assert_eq!(ml_code(128), 43);
    }

    #[test]
    fn wild_literal_copies_store_what_exact_copies_store() {
        let src: alloc::vec::Vec<u8> = (0..64u8).collect();
        // Short runs far from the end take the 16-byte chunk; runs near
        // the end or longer than 16 bytes copy exactly.
        for anchor in [0usize, 10, 47, 48, 50, 60] {
            for lit_length in 0..=(src.len() - anchor).min(20) {
                let mut wild = SeqStore::default();
                wild.reserve(1 << 10, 4);
                wild.store_seq_from(&src, anchor, lit_length, offset_to_offbase(7), 5);
                wild.store_seq_from(&src, 0, 3, repcode_to_offbase(1), 4);
                let mut exact = SeqStore::default();
                exact.reserve(1 << 10, 4);
                exact.store_seq(&src[anchor..anchor + lit_length], offset_to_offbase(7), 5);
                exact.store_seq(&src[..3], repcode_to_offbase(1), 4);
                assert_eq!(wild.literals, exact.literals, "{anchor} {lit_length}");
                assert_eq!(wild.sequences, exact.sequences);
            }
        }
    }

    #[test]
    fn long_lengths_are_flagged_and_restored() {
        let mut store = SeqStore::default();
        store.reserve(1 << 17, 4);
        store.store_seq_only(0x10005, offset_to_offbase(9), 4);
        store.store_seq(b"ab", repcode_to_offbase(1), MIN_MATCH + 1);
        store.fill_codes();
        assert_eq!(store.ll_code[0], MAX_LL);
        assert_eq!(store.sequence_length(0).lit_length, 0x10005);
        let mut other = SeqStore::default();
        other.reserve(1 << 17, 4);
        other.store_seq(b"", repcode_to_offbase(1), 0x10000 + MIN_MATCH + 1);
        other.fill_codes();
        assert_eq!(other.ml_code[0], MAX_ML);
        assert_eq!(
            other.sequence_length(0).match_length,
            0x10000 + MIN_MATCH as u32 + 1
        );
        other.reset();
        assert_eq!(other.nb_seq(), 0);
    }
}
