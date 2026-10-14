//! The optimal parser: `btopt`, `btultra` and `btultra2` (`zstd_opt.c`).
//!
//! Prices are fixed-point bit costs (1/256 bit) derived from running symbol
//! statistics, exactly as the reference computes them, so the chosen parse
//! — and therefore every emitted byte — matches it.

use alloc::vec::Vec;

use fearless_simd::{Simd, dispatch};
use fearless_simd_macros::simd;

use super::entropy::hist;
use super::ldm::RawSeqStore;
use super::match_state::{BlockInput, MatchState};
use super::mem;
use super::opt_count;
use super::params::ParamSwitch;
use super::seq_store::{
    SeqStore, ll_code, ml_code, new_rep, offset_to_offbase, repcode_to_offbase,
};
use crate::shared::format::{
    LL_BITS, MAX_LIT, MAX_LL, MAX_ML, MAX_OFF, MIN_MATCH, ML_BITS, REP_NUM, highbit32,
};

/// `ZSTD_OPT_NUM`.
pub(crate) const OPT_NUM: usize = 1 << 12;
/// `ZSTD_OPT_SIZE`.
pub(crate) const OPT_SIZE: usize = OPT_NUM + 3;

const LITFREQ_ADD: u32 = 2;
const MAX_PRICE: i32 = 1 << 30;
const PREDEF_THRESHOLD: usize = 8;
const BITCOST_ACCURACY: u32 = 8;
const BITCOST_MULTIPLIER: u32 = 1 << BITCOST_ACCURACY;
/// `prime4bytes`, `prime5bytes` and `prime6bytes` of `ZSTD_hashPtr`.
const PRIME_4_BYTES: u32 = 2_654_435_761;
const PRIME_5_BYTES: u64 = 889_523_592_379;
const PRIME_6_BYTES: u64 = 227_718_039_650_203;

/// How prices are derived (`ZSTD_OptPrice_e`).
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum PriceType {
    #[default]
    Dynamic,
    Predef,
}

/// One position of the forward pass (`ZSTD_optimal_t`).
#[derive(Copy, Clone, Debug, Default)]
struct Optimal {
    price: i32,
    off: u32,
    mlen: u32,
    litlen: u32,
    rep: [u32; REP_NUM],
}

/// One candidate match (`ZSTD_match_t`).
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct Match {
    pub(crate) off: u32,
    pub(crate) len: u32,
}

/// Pricing statistics of the optimal parser (`optState_t`).
#[derive(Clone, Debug, Default)]
pub(crate) struct OptState {
    pub(crate) lit_freq: Vec<u32>,
    pub(crate) lit_length_freq: Vec<u32>,
    pub(crate) match_length_freq: Vec<u32>,
    pub(crate) off_code_freq: Vec<u32>,
    match_table: Vec<Match>,
    price_table: Vec<Optimal>,
    pub(crate) lit_sum: u32,
    pub(crate) lit_length_sum: u32,
    pub(crate) match_length_sum: u32,
    pub(crate) off_code_sum: u32,
    pub(crate) lit_sum_base_price: u32,
    pub(crate) lit_length_sum_base_price: u32,
    pub(crate) match_length_sum_base_price: u32,
    pub(crate) off_code_sum_base_price: u32,
    pub(crate) price_type: PriceType,
    /// The frame's literal compression switch.
    pub(crate) literal_compression: ParamSwitch,
    /// The frequencies' weights, kept current with them.
    tables: PriceTables,
}

/// The symbol statistics' `WEIGHT`s, folded with each symbol's extra bits,
/// so that a dynamic price is one table read plus the sum's base price.
///
/// [`OptState::rescale_freqs`] rebuilds them for the block's optimization
/// level and [`OptState::update_stats`] patches the entries it changes,
/// so every price equals the reference's `ZSTD_litLengthPrice`,
/// `ZSTD_getMatchPrice` and `ZSTD_rawLiteralsCost`, computed from the
/// frequencies on each call. Sums wrap as the reference's `U32` sums do.
/// Each table is a power of two longer than any code its index function
/// returns, so a masked index needs no bounds check.
#[derive(Clone, Debug)]
struct PriceTables {
    /// `WEIGHT(litFreq[byte])`.
    literal: [u32; 256],
    /// `LL_bits[code] * BITCOST_MULTIPLIER - WEIGHT(litLengthFreq[code])`.
    lit_length: [u32; 64],
    /// `ML_bits[code] * BITCOST_MULTIPLIER - WEIGHT(matchLengthFreq[code])`
    /// plus the per-sequence surcharge.
    match_length: [u32; 128],
    /// `code * BITCOST_MULTIPLIER - WEIGHT(offCodeFreq[code])` plus the
    /// long-distance handicap below level 2.
    off_code: [u32; 32],
    /// The optimization level the weights were taken at.
    opt_level: u32,
}

impl Default for PriceTables {
    fn default() -> Self {
        Self {
            literal: [0; 256],
            lit_length: [0; 64],
            match_length: [0; 128],
            off_code: [0; 32],
            opt_level: 0,
        }
    }
}

impl PriceTables {
    /// The literal length entry for `code`.
    #[inline(always)]
    fn set_lit_length(&mut self, code: usize, freq: u32) {
        self.lit_length[code & 63] = (u32::from(LL_BITS[code]) * BITCOST_MULTIPLIER)
            .wrapping_sub(weight(freq, self.opt_level));
    }

    /// The match length entry for `code`.
    #[inline(always)]
    fn set_match_length(&mut self, code: usize, freq: u32) {
        self.match_length[code & 127] = (u32::from(ML_BITS[code]) * BITCOST_MULTIPLIER)
            .wrapping_sub(weight(freq, self.opt_level))
            .wrapping_add(BITCOST_MULTIPLIER / 5);
    }

    /// The offset code entry for `code`.
    #[inline(always)]
    fn set_off_code(&mut self, code: usize, freq: u32) {
        let code32 = code as u32;
        let mut price = (code32 * BITCOST_MULTIPLIER).wrapping_sub(weight(freq, self.opt_level));
        if self.opt_level < 2 && code32 >= 20 {
            price = price.wrapping_add((code32 - 19) * 2 * BITCOST_MULTIPLIER);
        }
        self.off_code[code & 31] = price;
    }
}

impl OptState {
    /// Sizes the frequency tables (`ZSTD_reset_matchState`'s opt space).
    ///
    /// The position and match tables are left to [`Self::reserve_positions`]:
    /// the reference carves them uninitialized from its workspace, and
    /// filling all [`OPT_SIZE`] entries here dominated frames that never
    /// reach the parser (empty input) or use a few of them (tiny input).
    pub(crate) fn allocate(&mut self) {
        self.lit_freq.resize(MAX_LIT as usize + 1, 0);
        self.lit_length_freq.resize(MAX_LL as usize + 1, 0);
        self.match_length_freq.resize(MAX_ML as usize + 1, 0);
        self.off_code_freq.resize(MAX_OFF as usize + 1, 0);
    }

    /// Grows the position and match tables to cover a block of
    /// `block_len` bytes.
    ///
    /// The parser reads and writes positions up to two past the furthest
    /// match end, which lies within the block and below [`OPT_NUM`]; the
    /// match list holds one entry per distinct length plus an LDM
    /// candidate. `block_len + 3` entries, capped at [`OPT_SIZE`], cover
    /// both. A table only grows, so each entry is written once per engine.
    fn reserve_positions(&mut self, block_len: usize) {
        let len = OPT_SIZE.min(block_len + 3);
        if self.price_table.len() < len {
            self.price_table.resize(len, Optimal::default());
            self.match_table.resize(len, Match::default());
        }
    }

    /// Bytes held by the parser's position and match tables.
    pub(crate) fn table_bytes(&self) -> usize {
        self.price_table.capacity() * core::mem::size_of::<Optimal>()
            + self.match_table.capacity() * core::mem::size_of::<Match>()
    }

    fn compressed_literals(&self) -> bool {
        self.literal_compression != ParamSwitch::Disable
    }

    fn set_base_prices(&mut self, opt_level: u32) {
        if self.compressed_literals() {
            self.lit_sum_base_price = weight(self.lit_sum, opt_level);
        }
        self.lit_length_sum_base_price = weight(self.lit_length_sum, opt_level);
        self.match_length_sum_base_price = weight(self.match_length_sum, opt_level);
        self.off_code_sum_base_price = weight(self.off_code_sum, opt_level);
    }

    /// `ZSTD_rescaleFreqs` without dictionary statistics.
    fn rescale_freqs(&mut self, src: &[u8], opt_level: u32) {
        let compressed_literals = self.compressed_literals();
        self.price_type = PriceType::Dynamic;
        if self.lit_length_sum == 0 {
            if src.len() <= PREDEF_THRESHOLD {
                self.price_type = PriceType::Predef;
            }
            if compressed_literals {
                let mut lit = MAX_LIT;
                let _ = hist::count(&mut self.lit_freq, &mut lit, src, false);
                self.lit_sum = downscale_stats(&mut self.lit_freq, MAX_LIT, 8, false);
            }
            const BASE_LL_FREQS: [u32; MAX_LL as usize + 1] = [
                4, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
                1, 1, 1, 1, 1, 1, 1, 1,
            ];
            self.lit_length_freq.copy_from_slice(&BASE_LL_FREQS);
            self.lit_length_sum = BASE_LL_FREQS.iter().sum();
            self.match_length_freq.fill(1);
            self.match_length_sum = MAX_ML + 1;
            const BASE_OFC_FREQS: [u32; MAX_OFF as usize + 1] = [
                6, 2, 1, 1, 2, 3, 4, 4, 4, 3, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
                1, 1, 1, 1,
            ];
            self.off_code_freq.copy_from_slice(&BASE_OFC_FREQS);
            self.off_code_sum = BASE_OFC_FREQS.iter().sum();
        } else {
            if compressed_literals {
                self.lit_sum = scale_stats(&mut self.lit_freq, MAX_LIT, 12);
            }
            self.lit_length_sum = scale_stats(&mut self.lit_length_freq, MAX_LL, 11);
            self.match_length_sum = scale_stats(&mut self.match_length_freq, MAX_ML, 11);
            self.off_code_sum = scale_stats(&mut self.off_code_freq, MAX_OFF, 11);
        }
        self.set_base_prices(opt_level);
        self.rebuild_tables(opt_level);
    }

    /// Recomputes every [`PriceTables`] entry from the frequencies.
    ///
    /// Equal to calling each `set_*` in turn, but the weights of a whole
    /// table are taken at once ([`weights`]) and folded with straight-line
    /// loops the compiler vectorizes: a block of a few bytes pays for this
    /// rebuild on every call.
    fn rebuild_tables(&mut self, opt_level: u32) {
        let tables = &mut self.tables;
        tables.opt_level = opt_level;
        if self.literal_compression != ParamSwitch::Disable {
            weights(&self.lit_freq, opt_level, &mut tables.literal);
        }
        let mut weight_of = [0u32; 64];
        weights(&self.lit_length_freq, opt_level, &mut weight_of);
        for ((entry, &bits), &weight) in tables.lit_length.iter_mut().zip(&LL_BITS).zip(&weight_of)
        {
            *entry = (u32::from(bits) * BITCOST_MULTIPLIER).wrapping_sub(weight);
        }
        weights(&self.match_length_freq, opt_level, &mut weight_of);
        for ((entry, &bits), &weight) in
            tables.match_length.iter_mut().zip(&ML_BITS).zip(&weight_of)
        {
            *entry = (u32::from(bits) * BITCOST_MULTIPLIER)
                .wrapping_sub(weight)
                .wrapping_add(BITCOST_MULTIPLIER / 5);
        }
        weights(&self.off_code_freq, opt_level, &mut weight_of);
        for (code, (entry, &weight)) in tables.off_code.iter_mut().zip(&weight_of).enumerate() {
            let code = code as u32;
            let handicap = if opt_level < 2 && code >= 20 {
                (code - 19) * 2 * BITCOST_MULTIPLIER
            } else {
                0
            };
            *entry = (code * BITCOST_MULTIPLIER)
                .wrapping_sub(weight)
                .wrapping_add(handicap);
        }
    }

    /// `ZSTD_rawLiteralsCost` for one literal.
    #[inline(always)]
    fn literal_price(&self, literal: u8) -> i32 {
        if !self.compressed_literals() {
            return ((1 << 3) * BITCOST_MULTIPLIER) as i32;
        }
        if self.price_type == PriceType::Predef {
            return (6 * BITCOST_MULTIPLIER) as i32;
        }
        let lit_price_max = self.lit_sum_base_price - BITCOST_MULTIPLIER;
        let lit_price = self.tables.literal[usize::from(literal)].min(lit_price_max);
        self.lit_sum_base_price.wrapping_sub(lit_price) as i32
    }

    /// `ZSTD_litLengthPrice`.
    #[inline(always)]
    fn lit_length_price(&self, lit_length: u32) -> i32 {
        if self.price_type == PriceType::Predef {
            return weight(lit_length, self.tables.opt_level) as i32;
        }
        if lit_length as usize == crate::shared::format::BLOCK_SIZE_MAX {
            return BITCOST_MULTIPLIER as i32 + self.lit_length_price(lit_length - 1);
        }
        let code = usize::from(ll_code(lit_length));
        self.tables.lit_length[code & 63].wrapping_add(self.lit_length_sum_base_price) as i32
    }

    /// The offset part of `ZSTD_getMatchPrice`; [`Self::match_length_price`]
    /// adds the rest. The sum wraps as the reference's `U32` sum does.
    #[inline(always)]
    fn offset_price(&self, off_base: u32) -> u32 {
        let off_code = highbit32(off_base);
        if self.price_type == PriceType::Predef {
            return (16 + off_code) * BITCOST_MULTIPLIER;
        }
        self.tables.off_code[off_code as usize & 31].wrapping_add(self.off_code_sum_base_price)
    }

    /// The match length part of `ZSTD_getMatchPrice`, including its
    /// heuristic surcharge per sequence.
    #[inline(always)]
    fn match_length_price(&self, match_length: u32) -> u32 {
        let ml_base = match_length - MIN_MATCH as u32;
        if self.price_type == PriceType::Predef {
            return weight(ml_base, self.tables.opt_level);
        }
        let code = usize::from(ml_code(ml_base));
        self.tables.match_length[code & 127].wrapping_add(self.match_length_sum_base_price)
    }

    /// `ZSTD_getMatchPrice`.
    #[cfg(test)]
    fn match_price(&self, off_base: u32, match_length: u32) -> u32 {
        self.offset_price(off_base)
            .wrapping_add(self.match_length_price(match_length))
    }

    /// `ZSTD_updateStats`.
    fn update_stats(&mut self, literals: &[u8], off_base: u32, match_length: u32) {
        if self.compressed_literals() {
            // A byte indexes the 256 frequencies without a bounds check, and
            // the level's weight form is chosen once for the run.
            let level = self.tables.opt_level;
            let weights = &mut self.tables.literal;
            if let Some(freqs) = self.lit_freq.first_chunk_mut::<256>() {
                if level != 0 {
                    for &lit in literals {
                        let freq = &mut freqs[usize::from(lit)];
                        *freq += LITFREQ_ADD;
                        weights[usize::from(lit)] = frac_weight(*freq);
                    }
                } else {
                    for &lit in literals {
                        let freq = &mut freqs[usize::from(lit)];
                        *freq += LITFREQ_ADD;
                        weights[usize::from(lit)] = bit_weight(*freq);
                    }
                }
            } else {
                for &lit in literals {
                    let freq = &mut self.lit_freq[usize::from(lit)];
                    *freq += LITFREQ_ADD;
                    weights[usize::from(lit)] = weight(*freq, level);
                }
            }
            self.lit_sum += literals.len() as u32 * LITFREQ_ADD;
        }
        let ll = usize::from(ll_code(literals.len() as u32));
        self.lit_length_freq[ll] += 1;
        self.tables.set_lit_length(ll, self.lit_length_freq[ll]);
        self.lit_length_sum += 1;
        let off_code = highbit32(off_base) as usize;
        self.off_code_freq[off_code] += 1;
        self.tables
            .set_off_code(off_code, self.off_code_freq[off_code]);
        self.off_code_sum += 1;
        let ml = usize::from(ml_code(match_length - MIN_MATCH as u32));
        self.match_length_freq[ml] += 1;
        self.tables.set_match_length(ml, self.match_length_freq[ml]);
        self.match_length_sum += 1;
    }
}

/// `ZSTD_bitWeight`.
const fn bit_weight(stat: u32) -> u32 {
    highbit32(stat + 1) * BITCOST_MULTIPLIER
}

/// `ZSTD_fracWeight`.
const fn frac_weight(raw_stat: u32) -> u32 {
    let stat = raw_stat + 1;
    let hb = highbit32(stat);
    let b_weight = hb * BITCOST_MULTIPLIER;
    let f_weight = (stat << BITCOST_ACCURACY) >> hb;
    b_weight + f_weight
}

/// `WEIGHT(stat, optLevel)`.
#[inline(always)]
const fn weight(stat: u32, opt_level: u32) -> u32 {
    if opt_level != 0 {
        frac_weight(stat)
    } else {
        bit_weight(stat)
    }
}

/// `WEIGHT(stats[i], opt_level)` into `out[i]`, for as many entries as both
/// hold.
///
/// For `stat + 1 < 2^24`, `stat + 1` converts to `f32` exactly, its
/// exponent is `highbit32(stat + 1) + 127`, and its top eight mantissa bits
/// are `((stat + 1) << 8 >> highbit) - 256`. So `bits >> 23` gives
/// `ZSTD_bitWeight` and `bits >> 15` gives `ZSTD_fracWeight`, each up to a
/// constant, in a loop the compiler vectorizes. From `2^24` on, the
/// reference's `stat << 8` wraps, so a table holding such a statistic is
/// redone with the integer form, which wraps the same way.
fn weights(stats: &[u32], opt_level: u32, out: &mut [u32]) {
    let bits = |stat: u32| ((stat + 1) as i32 as f32).to_bits();
    let mut seen = 0u32;
    if opt_level != 0 {
        for (weight_out, &stat) in out.iter_mut().zip(stats) {
            seen |= stat + 1;
            *weight_out = (bits(stat) >> 15).wrapping_sub(126 * BITCOST_MULTIPLIER);
        }
    } else {
        for (weight_out, &stat) in out.iter_mut().zip(stats) {
            seen |= stat + 1;
            *weight_out = ((bits(stat) >> 23).wrapping_sub(127)).wrapping_mul(BITCOST_MULTIPLIER);
        }
    }
    if seen >> 24 != 0 {
        for (weight_out, &stat) in out.iter_mut().zip(stats) {
            *weight_out = weight(stat, opt_level);
        }
    }
}

/// `ZSTD_downscaleStats`.
fn downscale_stats(table: &mut [u32], last: u32, shift: u32, base1: bool) -> u32 {
    let mut sum = 0u32;
    for value in &mut table[..=last as usize] {
        let base = if base1 { 1 } else { u32::from(*value > 0) };
        let new_stat = base + (*value >> shift);
        sum += new_stat;
        *value = new_stat;
    }
    sum
}

/// `ZSTD_scaleStats`.
fn scale_stats(table: &mut [u32], last: u32, log_target: u32) -> u32 {
    let prev_sum: u32 = table[..=last as usize].iter().sum();
    let factor = prev_sum >> log_target;
    if factor <= 1 {
        return prev_sum;
    }
    downscale_stats(table, last, highbit32(factor), true)
}

// ---------------------------------------------------------------------------
// Binary tree.

/// Per-block constants of the tree searches.
///
/// The reference walks raw pointers: `base + index` in the prefix and
/// `dictBase + index` in the external dictionary. Here both segments are
/// plain slices, so an index below `dict_limit` is an offset from the end of
/// `ext` and one at or above it an offset from the start of `prefix`.
#[derive(Copy, Clone, Debug)]
struct Ctx<'a, S> {
    simd: S,
    window: mem::Window<'a>,
    /// Bytes from index `dict_limit` to the end of the block.
    prefix: &'a [u8],
    /// Bytes from index `low_limit` to `dict_limit`.
    ext: &'a [u8],
    dict_limit: u32,
    iend: u32,
    mls: u32,
}

impl<'a, S: Simd> Ctx<'a, S> {
    fn new(simd: S, block: &BlockInput<'a>, mls: u32) -> Self {
        let window = block.window;
        let prefix = window.prefix;
        let ext = window.ext;
        Self {
            simd,
            window,
            prefix: prefix.slice(prefix.start(), prefix.end()),
            ext: ext.slice(ext.start(), ext.end()),
            dict_limit: prefix.start(),
            iend: block.end,
            mls,
        }
    }

    /// Offset of prefix index `index` in [`Self::prefix`].
    #[inline(always)]
    const fn offset(&self, index: u32) -> usize {
        (index - self.dict_limit) as usize
    }

    /// Offset of external index `index` in [`Self::ext`].
    #[inline(always)]
    const fn ext_offset(&self, index: u32) -> usize {
        self.ext.len() - (self.dict_limit - index) as usize
    }

    /// The byte at window index `index`.
    #[inline(always)]
    fn byte<const EXT: bool>(&self, index: usize) -> u8 {
        let dict_limit = self.dict_limit as usize;
        if !EXT || index >= dict_limit {
            self.prefix[index - dict_limit]
        } else {
            self.ext[self.ext.len() - (dict_limit - index)]
        }
    }

    /// Little-endian `u32` at prefix index `index`.
    #[inline(always)]
    fn read32(&self, index: u32) -> u32 {
        let at = self.offset(index);
        let mut word = [0u8; 4];
        word.copy_from_slice(&self.prefix[at..at + 4]);
        u32::from_le_bytes(word)
    }

    /// `ZSTD_hashPtr(base + index, hash_log, MLS)`: the 4-byte hash for
    /// `MLS` 3 and 4, the 5- and 6-byte ones above.
    #[inline(always)]
    fn hash<const MLS: u32>(&self, index: u32, hash_log: u32) -> usize {
        if MLS <= 4 {
            return (self.read32(index).wrapping_mul(PRIME_4_BYTES) >> (32 - hash_log)) as usize;
        }
        let at = self.offset(index);
        let mut word = [0u8; 8];
        word.copy_from_slice(&self.prefix[at..at + 8]);
        let (shift, prime) = if MLS == 5 {
            (24, PRIME_5_BYTES)
        } else {
            (16, PRIME_6_BYTES)
        };
        ((u64::from_le_bytes(word) << shift).wrapping_mul(prime) >> (64 - hash_log)) as usize
    }

    /// `ZSTD_count(ip, match, iend)` inside the prefix.
    #[inline(always)]
    fn count(&self, ip: u32, candidate: u32) -> usize {
        opt_count::count(
            self.simd,
            self.prefix,
            self.offset(ip),
            self.offset(candidate),
            self.offset(self.iend),
        )
    }

    /// `ZSTD_count_2segments(ip, dictBase + candidate, iend, dictEnd,
    /// prefixStart)` for a candidate in the external dictionary.
    #[inline(always)]
    fn count_2segments(&self, ip: u32, candidate: u32) -> usize {
        opt_count::count_2segments(
            self.simd,
            self.prefix,
            self.offset(ip),
            self.ext,
            self.ext_offset(candidate),
            self.offset(self.iend),
        )
    }

    /// The bytes from prefix index `curr` to the end of the block.
    #[inline(always)]
    fn current(&self, curr: u32) -> &'a [u8] {
        &self.prefix[self.offset(curr)..self.offset(self.iend)]
    }

    /// The tree walk's comparison: `known` bytes are already equal; returns
    /// the full common length of `curr` (whose bytes to the end of the
    /// block are `current`) and `candidate`, the latter read through both
    /// segments with `EXT`, and whether the candidate's first differing
    /// byte is the smaller (`false` when the run reaches the end of the
    /// block, where the walk stops without looking).
    #[inline(always)]
    fn extend<const EXT: bool>(
        &self,
        current: &[u8],
        curr: u32,
        candidate: u32,
        known: usize,
    ) -> (usize, bool) {
        let dict_limit = self.dict_limit as usize;
        if !EXT || candidate as usize + known >= dict_limit {
            let (length, smaller) = opt_count::count_ordered(
                self.simd,
                current,
                self.prefix,
                // The candidate may start in the external segment with its
                // known prefix reaching into this one.
                candidate + known as u32 - self.dict_limit,
                known as u32,
            );
            (known + length, smaller)
        } else {
            let known32 = known as u32;
            let length = known + self.count_2segments(curr + known32, candidate + known32);
            let smaller = length < current.len()
                && self.byte::<EXT>(candidate as usize + length)
                    < self.byte::<EXT>(curr as usize + length);
            (length, smaller)
        }
    }
}

/// `ZSTD_readMINMATCH` comparison of `length` (3 or 4) bytes.
#[inline(always)]
fn min_match_equal(x: u32, y: u32, length: u32) -> bool {
    if length == 3 {
        (x << 8) == (y << 8)
    } else {
        x == y
    }
}

/// `ZSTD_insertAndFindFirstIndexHash3`.
///
/// The table holds exactly `1 << hashLog3` entries and every hash is below
/// that, so indexing a slice cut to that length through the mask changes
/// nothing but drops the bounds checks from the insertion loop.
#[inline(always)]
fn insert_and_find_first_index_hash3<S: Simd>(
    ms: &mut MatchState,
    ctx: &Ctx<'_, S>,
    next_to_update3: &mut u32,
    ip: u32,
) -> u32 {
    let hash_log3 = ms.hash_log3;
    let mask = (1usize << hash_log3) - 1;
    let table = &mut ms.hash_table3[..=mask];
    let hash3 = |index: u32| mem::hash3(ctx.read32(index), hash_log3, 0) & mask;
    let mut idx = *next_to_update3;
    let target = ip;
    while idx < target {
        table[hash3(idx)] = idx;
        idx += 1;
    }
    *next_to_update3 = target;
    table[hash3(ip)]
}

/// `ZSTD_insertBt1`: returns how many positions it accounted for.
#[inline(always)]
fn insert_bt1<S: Simd, const EXT: bool, const MLS: u32>(
    ms: &mut MatchState,
    ctx: &Ctx<'_, S>,
    curr: u32,
    target: u32,
) -> u32 {
    let cparams = ms.cparams;
    let h = ctx.hash::<MLS>(curr, cparams.hash_log);
    let match_index = ms.hash_table[h];
    ms.hash_table[h] = curr;
    let walk = TreeWalk {
        curr,
        match_index,
        window_low: ms.lowest_match_index(target, cparams.window_log),
        nb_compares: 1u32 << cparams.search_log,
        best_length: 8,
    };
    let (best_length, match_end_idx) =
        walk.run::<S, EXT>(ctx, &mut ms.chain_table, cparams.chain_log, |_, _| false);
    let positions = if best_length > 384 {
        192.min((best_length - 384) as u32)
    } else {
        0
    };
    positions.max(match_end_idx - (curr + 8))
}

/// One insertion of `curr` into the binary tree: the walk shared by
/// `ZSTD_insertBt1` and `ZSTD_insertBtAndGetAllMatches`.
#[derive(Copy, Clone, Debug)]
struct TreeWalk {
    curr: u32,
    /// The first candidate, from the hash table.
    match_index: u32,
    /// Candidates below this index are out of the window.
    window_low: u32,
    nb_compares: u32,
    /// Length a candidate must exceed to be reported.
    best_length: usize,
}

impl TreeWalk {
    /// Walks the tree from `match_index`, re-linking it so that `curr` is
    /// the new root, and calls `on_longer(index, length)` for each candidate
    /// longer than every earlier one; it returns `true` to stop the walk.
    /// Returns the longest length and the farthest match end
    /// (`matchEndIdx`).
    ///
    /// The reference parks a finished side on a dummy cell; here a side
    /// whose walk ended below `bt_low` is simply not cleared at the end.
    #[inline(always)]
    fn run<S: Simd, const EXT: bool>(
        self,
        ctx: &Ctx<'_, S>,
        chain_table: &mut [u32],
        chain_log: u32,
        mut on_longer: impl FnMut(u32, usize) -> bool,
    ) -> (usize, u32) {
        let Self {
            curr,
            mut match_index,
            window_low,
            mut nb_compares,
            mut best_length,
        } = self;
        let bt_mask = (1u32 << (chain_log - 1)) - 1;
        // Cut to the tree's size and indexed through `cells`: every cell
        // index is below that size already, so the mask changes nothing
        // but lets the compiler drop the bounds checks.
        let cells = (2 * bt_mask + 1) as usize;
        let bt = &mut chain_table[..=cells];
        let bt_low = curr.saturating_sub(bt_mask);
        let current = ctx.current(curr);
        let mut common_smaller = 0usize;
        let mut common_larger = 0usize;
        // The cells the next smaller / larger candidate is linked from.
        let mut smaller = 2 * (curr & bt_mask) as usize;
        let mut larger = smaller + 1;
        let mut match_end_idx = curr + 8 + 1;
        let (clear_smaller, clear_larger) = loop {
            if nb_compares == 0 || match_index < window_low {
                break (true, true);
            }
            let (match_length, match_smaller) = ctx.extend::<EXT>(
                current,
                curr,
                match_index,
                common_smaller.min(common_larger),
            );
            if match_length > best_length {
                best_length = match_length;
                if match_length as u32 > match_end_idx - match_index {
                    match_end_idx = match_index + match_length as u32;
                }
                if on_longer(match_index, match_length) {
                    break (true, true);
                }
            }
            if match_length == current.len() {
                // The run reaches the end of the block: no way to order it.
                break (true, true);
            }
            let next = 2 * (match_index & bt_mask) as usize;
            if match_smaller {
                bt[smaller & cells] = match_index;
                common_smaller = match_length;
                if match_index <= bt_low {
                    break (false, true);
                }
                smaller = next + 1;
                match_index = bt[(next + 1) & cells];
            } else {
                bt[larger & cells] = match_index;
                common_larger = match_length;
                if match_index <= bt_low {
                    break (true, false);
                }
                larger = next;
                match_index = bt[next & cells];
            }
            nb_compares -= 1;
        };
        if clear_smaller {
            bt[smaller & cells] = 0;
        }
        if clear_larger {
            bt[larger & cells] = 0;
        }
        (best_length, match_end_idx)
    }
}

/// Where and what `get_all_matches` searches: the position, its repeat
/// offsets, whether the literal length is zero, and the length to beat.
#[derive(Copy, Clone, Debug)]
struct MatchQuery<'r> {
    ip: u32,
    rep: &'r [u32; REP_NUM],
    ll0: bool,
    length_to_beat: u32,
}

/// `ZSTD_insertBtAndGetAllMatches`.
#[inline(always)]
fn insert_bt_and_get_all_matches<S: Simd, const EXT: bool, const MLS: u32>(
    ms: &mut MatchState,
    ctx: &Ctx<'_, S>,
    matches: &mut [Match],
    next_to_update3: &mut u32,
    query: MatchQuery<'_>,
) -> u32 {
    let MatchQuery {
        ip,
        rep,
        ll0,
        length_to_beat,
    } = query;
    let cparams = ms.cparams;
    let sufficient_len = cparams.target_length.min(OPT_NUM as u32 - 1);
    let curr = ip;
    let mls = MLS;
    let min_match = if mls == 3 { 3 } else { 4 };
    let h = ctx.hash::<MLS>(ip, cparams.hash_log);
    let match_index = ms.hash_table[h];
    let dict_limit = ctx.dict_limit;
    let window_low = ms.lowest_match_index(curr, cparams.window_log);
    let match_low = if window_low != 0 { window_low } else { 1 };
    let mut mnum = 0usize;
    let mut best_length = (length_to_beat - 1) as usize;
    let i_limit = ctx.iend;

    // Repeat offsets.
    {
        let ip_word = ctx.read32(ip);
        let ll0 = u32::from(ll0);
        let last_r = REP_NUM as u32 + ll0;
        for rep_code in ll0..last_r {
            let rep_offset = if rep_code == REP_NUM as u32 {
                rep[0].wrapping_sub(1)
            } else {
                rep[rep_code as usize]
            };
            let rep_index = curr.wrapping_sub(rep_offset);
            let mut rep_len = 0u32;
            if rep_offset.wrapping_sub(1) < curr - dict_limit {
                if rep_index >= window_low
                    && min_match_equal(ip_word, ctx.read32(ip - rep_offset), min_match)
                {
                    rep_len =
                        ctx.count(ip + min_match, ip + min_match - rep_offset) as u32 + min_match;
                }
            } else if EXT
                && rep_offset.wrapping_sub(1) < curr - window_low
                && dict_limit.wrapping_sub(1).wrapping_sub(rep_index) >= 3
                && min_match_equal(ip_word, ctx.window.ext.read32(rep_index), min_match)
            {
                rep_len =
                    ctx.count_2segments(ip + min_match, rep_index + min_match) as u32 + min_match;
            }
            if rep_len as usize > best_length {
                best_length = rep_len as usize;
                matches[mnum] = Match {
                    off: repcode_to_offbase(rep_code - ll0 + 1),
                    len: rep_len,
                };
                mnum += 1;
                if rep_len > sufficient_len || ip + rep_len == i_limit {
                    return mnum as u32;
                }
            }
        }
    }

    // HC3 match finder.
    if mls == 3 && best_length < mls as usize {
        let match_index3 = insert_and_find_first_index_hash3(ms, ctx, next_to_update3, ip);
        if match_index3 >= match_low && curr - match_index3 < (1 << 18) {
            let mlen = if !EXT || match_index3 >= dict_limit {
                ctx.count(ip, match_index3)
            } else {
                ctx.count_2segments(ip, match_index3)
            };
            if mlen >= mls as usize {
                best_length = mlen;
                matches[0] = Match {
                    off: offset_to_offbase(curr - match_index3),
                    len: mlen as u32,
                };
                mnum = 1;
                if mlen > sufficient_len as usize || ip as usize + mlen == i_limit as usize {
                    ms.next_to_update = curr + 1;
                    return 1;
                }
            }
        }
    }

    ms.hash_table[h] = curr;
    let walk = TreeWalk {
        curr,
        match_index,
        window_low: match_low,
        nb_compares: 1u32 << cparams.search_log,
        best_length,
    };
    let (_, match_end_idx) = walk.run::<S, EXT>(
        ctx,
        &mut ms.chain_table,
        cparams.chain_log,
        |match_index, match_length| {
            matches[mnum] = Match {
                off: offset_to_offbase(curr - match_index),
                len: match_length as u32,
            };
            mnum += 1;
            // A run reaching the block's end stops the walk after this.
            match_length > OPT_NUM
        },
    );
    ms.next_to_update = match_end_idx - 8;
    mnum as u32
}

/// `ZSTD_btGetAllMatches_internal`: brings the tree up to `query.ip`, then
/// collects the matches there.
///
/// The wrapper inlines into the parser, which calls the level's
/// feature-enabled entry directly; the tree update and the search inline
/// into that entry.
#[simd]
#[inline(always)]
fn get_all_matches<S: Simd, const EXT: bool, const MLS: u32>(
    simd: S,
    ms: &mut MatchState,
    ctx: &Ctx<'_, S>,
    matches: &mut [Match],
    next_to_update3: &mut u32,
    query: MatchQuery<'_>,
) -> u32 {
    let _ = simd;
    if query.ip < ms.next_to_update {
        return 0;
    }
    // ZSTD_updateTree_internal.
    let target = query.ip;
    let mut idx = ms.next_to_update;
    while idx < target {
        idx += insert_bt1::<S, EXT, MLS>(ms, ctx, idx, target);
    }
    ms.next_to_update = target;
    insert_bt_and_get_all_matches::<S, EXT, MLS>(ms, ctx, matches, next_to_update3, query)
}

/// [`get_all_matches`] for the block's minimum match length, the
/// reference's `ZSTD_selectBtGetAllMatches`; the branch resolves the same
/// way for the whole block.
#[inline(always)]
fn find_matches<S: Simd, const EXT: bool>(
    simd: S,
    ms: &mut MatchState,
    ctx: &Ctx<'_, S>,
    matches: &mut [Match],
    next_to_update3: &mut u32,
    query: MatchQuery<'_>,
) -> u32 {
    match ctx.mls {
        3 => get_all_matches::<S, EXT, 3>(simd, ms, ctx, matches, next_to_update3, query),
        4 => get_all_matches::<S, EXT, 4>(simd, ms, ctx, matches, next_to_update3, query),
        5 => get_all_matches::<S, EXT, 5>(simd, ms, ctx, matches, next_to_update3, query),
        _ => get_all_matches::<S, EXT, 6>(simd, ms, ctx, matches, next_to_update3, query),
    }
}

// ---------------------------------------------------------------------------
// LDM candidates.

/// LDM candidate tracking (`ZSTD_optLdm_t`).
#[derive(Debug)]
struct OptLdm {
    seq_store: RawSeqStore,
    start_pos_in_block: u32,
    end_pos_in_block: u32,
    offset: u32,
}

impl OptLdm {
    /// `ZSTD_opt_getNextMatchAndUpdateSeqStore`.
    fn next_match(&mut self, curr_pos_in_block: u32, block_bytes_remaining: u32) {
        if self.seq_store.size() == 0 || self.seq_store.pos >= self.seq_store.size() {
            self.start_pos_in_block = u32::MAX;
            self.end_pos_in_block = u32::MAX;
            return;
        }
        let curr = self.seq_store.seq[self.seq_store.pos];
        let curr_block_end_pos = curr_pos_in_block + block_bytes_remaining;
        let pos_in_seq = self.seq_store.pos_in_sequence as u32;
        let literals_remaining = curr.lit_length.saturating_sub(pos_in_seq);
        let match_remaining = if literals_remaining == 0 {
            curr.match_length - (pos_in_seq - curr.lit_length)
        } else {
            curr.match_length
        };
        if literals_remaining >= block_bytes_remaining {
            self.start_pos_in_block = u32::MAX;
            self.end_pos_in_block = u32::MAX;
            self.seq_store
                .skip_bytes_opt(block_bytes_remaining as usize);
            return;
        }
        self.start_pos_in_block = curr_pos_in_block + literals_remaining;
        self.end_pos_in_block = self.start_pos_in_block + match_remaining;
        self.offset = curr.offset;
        if self.end_pos_in_block > curr_block_end_pos {
            self.end_pos_in_block = curr_block_end_pos;
            self.seq_store
                .skip_bytes_opt((curr_block_end_pos - curr_pos_in_block) as usize);
        } else {
            self.seq_store
                .skip_bytes_opt((literals_remaining + match_remaining) as usize);
        }
    }

    /// `ZSTD_optLdm_maybeAddMatch`.
    fn maybe_add_match(
        &self,
        matches: &mut [Match],
        nb_matches: &mut u32,
        curr_pos_in_block: u32,
        min_match: u32,
    ) {
        let pos_diff = curr_pos_in_block.wrapping_sub(self.start_pos_in_block);
        let candidate_len = self
            .end_pos_in_block
            .wrapping_sub(self.start_pos_in_block)
            .wrapping_sub(pos_diff);
        if curr_pos_in_block < self.start_pos_in_block
            || curr_pos_in_block >= self.end_pos_in_block
            || candidate_len < min_match
        {
            return;
        }
        if *nb_matches == 0
            || (candidate_len > matches[*nb_matches as usize - 1].len
                && (*nb_matches as usize) < OPT_NUM)
        {
            matches[*nb_matches as usize] = Match {
                off: offset_to_offbase(self.offset),
                len: candidate_len,
            };
            *nb_matches += 1;
        }
    }

    /// `ZSTD_optLdm_processMatchCandidate`.
    ///
    /// Without LDM candidates this is one inline test per position; the
    /// candidate bookkeeping stays out of line.
    #[inline(always)]
    fn process(
        &mut self,
        matches: &mut [Match],
        nb_matches: &mut u32,
        curr_pos_in_block: u32,
        remaining: u32,
        min_match: u32,
    ) {
        if self.seq_store.pos >= self.seq_store.size() {
            return;
        }
        self.process_candidate(matches, nb_matches, curr_pos_in_block, remaining, min_match);
    }

    /// [`Self::process`] with candidates left.
    #[inline(never)]
    fn process_candidate(
        &mut self,
        matches: &mut [Match],
        nb_matches: &mut u32,
        curr_pos_in_block: u32,
        remaining: u32,
        min_match: u32,
    ) {
        if curr_pos_in_block >= self.end_pos_in_block {
            if curr_pos_in_block > self.end_pos_in_block {
                let overshoot = curr_pos_in_block - self.end_pos_in_block;
                self.seq_store.skip_bytes_opt(overshoot as usize);
            }
            self.next_match(curr_pos_in_block, remaining);
        }
        self.maybe_add_match(matches, nb_matches, curr_pos_in_block, min_match);
    }
}

// ---------------------------------------------------------------------------
// The parser.

/// `ZSTD_compressBlock_opt_generic`, in `simd`'s feature context.
///
/// `OPT_LEVEL` (0 for btopt, 2 for the ultra strategies) and `EXT` (an
/// external dictionary is in the window) are the reference's template
/// parameters; both are fixed per block.
#[simd]
fn opt_generic<S: Simd, const OPT_LEVEL: u32, const EXT: bool>(
    simd: S,
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; REP_NUM],
    block: &BlockInput<'_>,
) -> usize {
    let opt_level = OPT_LEVEL;
    let prefix = block.window.prefix;
    let istart = block.start;
    let iend = block.end;
    let ilimit = i64::from(iend) - 8;
    let prefix_start = ms.window.dict_limit;
    let cparams = ms.cparams;
    let sufficient_len = cparams.target_length.min(OPT_NUM as u32 - 1);
    let min_match = if cparams.min_match == 3 { 3 } else { 4 };
    let mut next_to_update3 = ms.next_to_update;
    let ctx = Ctx::new(simd, block, cparams.min_match.clamp(3, 6));

    ms.opt.reserve_positions((iend - istart) as usize);
    let mut opt = core::mem::take(&mut ms.opt.price_table);
    let mut matches = core::mem::take(&mut ms.opt.match_table);
    let mut last_stretch = Optimal::default();
    // The reference copies the store's header; here the store is moved out
    // for the block and returned with its cursor as it was.
    let lent = ms.ldm_seq_store.take();
    let lent_cursor = lent
        .as_ref()
        .map(|store| (store.pos, store.pos_in_sequence));
    let mut opt_ldm = OptLdm {
        seq_store: lent.unwrap_or_default(),
        start_pos_in_block: 0,
        end_pos_in_block: 0,
        offset: 0,
    };
    let mut ip = istart;
    let mut anchor = istart;
    opt_ldm.next_match(ip - istart, iend - ip);

    ms.opt.rescale_freqs(prefix.slice(istart, iend), opt_level);
    ip += u32::from(ip == prefix_start);

    while i64::from(ip) < ilimit {
        let mut cur: u32;
        let mut last_pos: u32;

        // Find the first match.
        let litlen = ip - anchor;
        let ll0 = litlen == 0;
        let mut nb_matches = find_matches::<S, EXT>(
            simd,
            ms,
            &ctx,
            &mut matches,
            &mut next_to_update3,
            MatchQuery {
                ip,
                rep,
                ll0,
                length_to_beat: min_match,
            },
        );
        opt_ldm.process(
            &mut matches,
            &mut nb_matches,
            ip - istart,
            iend - ip,
            min_match,
        );
        if nb_matches == 0 {
            ip += 1;
            continue;
        }

        let os = &ms.opt;
        opt[0].mlen = 0;
        opt[0].litlen = litlen;
        opt[0].price = os.lit_length_price(litlen);
        opt[0].rep = *rep;

        let max_ml = matches[nb_matches as usize - 1].len;
        let max_off_base = matches[nb_matches as usize - 1].off;
        let shortest_path_ready;
        if max_ml > sufficient_len {
            last_stretch.litlen = 0;
            last_stretch.mlen = max_ml;
            last_stretch.off = max_off_base;
            cur = 0;
            last_pos = max_ml;
            shortest_path_ready = true;
        } else {
            let mut pos = 1u32;
            while pos < min_match {
                opt[pos as usize].price = MAX_PRICE;
                opt[pos as usize].mlen = 0;
                opt[pos as usize].litlen = litlen + pos;
                pos += 1;
            }
            let ll0_price = os.lit_length_price(0);
            for m in &matches[..nb_matches as usize] {
                let off_base = m.off;
                let end = m.len;
                let off_price = os.offset_price(off_base);
                while pos <= end {
                    let match_price = off_price.wrapping_add(os.match_length_price(pos)) as i32;
                    let sequence_price = opt[0].price + match_price;
                    let slot = &mut opt[pos as usize];
                    slot.mlen = pos;
                    slot.off = off_base;
                    slot.litlen = 0;
                    slot.price = sequence_price + ll0_price;
                    pos += 1;
                }
            }
            last_pos = pos - 1;
            opt[pos as usize].price = MAX_PRICE;
            shortest_path_ready = false;
            cur = 0;
        }

        if !shortest_path_ready {
            let mut jumped = false;
            cur = 1;
            while cur <= last_pos {
                let inr = ip + cur;
                {
                    let os = &ms.opt;
                    let ll_price = |l: u32| os.lit_length_price(l);
                    let ll_inc_price = |l: u32| ll_price(l) - ll_price(l - 1);
                    let litlen = opt[cur as usize - 1].litlen + 1;
                    let price = opt[cur as usize - 1].price
                        + os.literal_price(prefix.byte(ip + cur - 1))
                        + ll_inc_price(litlen);
                    if price <= opt[cur as usize].price {
                        let prev_match = opt[cur as usize];
                        opt[cur as usize] = opt[cur as usize - 1];
                        opt[cur as usize].litlen = litlen;
                        opt[cur as usize].price = price;
                        if opt_level >= 1
                            && prev_match.litlen == 0
                            && ll_inc_price(1) < 0
                            && ip + cur < iend
                        {
                            let next_lit = os.literal_price(prefix.byte(ip + cur));
                            let with1literal = prev_match.price + next_lit + ll_inc_price(1);
                            let with_more_literals = price + next_lit + ll_inc_price(litlen + 1);
                            if with1literal < with_more_literals
                                && with1literal < opt[cur as usize + 1].price
                            {
                                let prev = cur - prev_match.mlen;
                                let new_reps = new_rep(
                                    &opt[prev as usize].rep,
                                    prev_match.off,
                                    opt[prev as usize].litlen == 0,
                                );
                                opt[cur as usize + 1] = prev_match;
                                opt[cur as usize + 1].rep = new_reps;
                                opt[cur as usize + 1].litlen = 1;
                                opt[cur as usize + 1].price = with1literal;
                                if last_pos < cur + 1 {
                                    last_pos = cur + 1;
                                }
                            }
                        }
                    }
                }

                if opt[cur as usize].litlen == 0 {
                    let prev = cur - opt[cur as usize].mlen;
                    let new_reps = new_rep(
                        &opt[prev as usize].rep,
                        opt[cur as usize].off,
                        opt[prev as usize].litlen == 0,
                    );
                    opt[cur as usize].rep = new_reps;
                }

                if i64::from(inr) > ilimit {
                    cur += 1;
                    continue;
                }
                if cur == last_pos {
                    break;
                }
                if opt_level == 0
                    && opt[cur as usize + 1].price
                        <= opt[cur as usize].price + (BITCOST_MULTIPLIER / 2) as i32
                {
                    cur += 1;
                    continue;
                }

                let ll0 = opt[cur as usize].litlen == 0;
                let previous_price = opt[cur as usize].price;
                let base_price = previous_price + ms.opt.lit_length_price(0);
                let cur_rep = opt[cur as usize].rep;
                let mut nb_matches = find_matches::<S, EXT>(
                    simd,
                    ms,
                    &ctx,
                    &mut matches,
                    &mut next_to_update3,
                    MatchQuery {
                        ip: inr,
                        rep: &cur_rep,
                        ll0,
                        length_to_beat: min_match,
                    },
                );
                opt_ldm.process(
                    &mut matches,
                    &mut nb_matches,
                    inr - istart,
                    iend - inr,
                    min_match,
                );
                if nb_matches == 0 {
                    cur += 1;
                    continue;
                }
                let longest_ml = matches[nb_matches as usize - 1].len;
                if longest_ml > sufficient_len
                    || cur + longest_ml >= OPT_NUM as u32
                    || ip + cur + longest_ml >= iend
                {
                    last_stretch.mlen = longest_ml;
                    last_stretch.off = matches[nb_matches as usize - 1].off;
                    last_stretch.litlen = 0;
                    last_pos = cur + longest_ml;
                    jumped = true;
                    break;
                }

                let os = &ms.opt;
                for match_nb in 0..nb_matches as usize {
                    let offset = matches[match_nb].off;
                    let last_ml = matches[match_nb].len;
                    let start_ml = if match_nb > 0 {
                        matches[match_nb - 1].len + 1
                    } else {
                        min_match
                    };
                    let off_price = os.offset_price(offset);
                    let mut mlen = last_ml;
                    while mlen >= start_ml {
                        let pos = cur + mlen;
                        let price =
                            base_price + off_price.wrapping_add(os.match_length_price(mlen)) as i32;
                        if pos > last_pos || price < opt[pos as usize].price {
                            while last_pos < pos {
                                last_pos += 1;
                                opt[last_pos as usize].price = MAX_PRICE;
                                opt[last_pos as usize].litlen = 1;
                            }
                            let slot = &mut opt[pos as usize];
                            slot.mlen = mlen;
                            slot.off = offset;
                            slot.litlen = 0;
                            slot.price = price;
                        } else if opt_level == 0 {
                            break;
                        }
                        mlen -= 1;
                    }
                }
                opt[last_pos as usize + 1].price = MAX_PRICE;
                cur += 1;
            }

            if !jumped {
                last_stretch = opt[last_pos as usize];
                cur = last_pos - last_stretch.mlen;
            }
        }

        // _shortestPath:
        if last_stretch.mlen == 0 {
            ip += last_pos;
            continue;
        }
        if last_stretch.litlen == 0 {
            *rep = new_rep(
                &opt[cur as usize].rep,
                last_stretch.off,
                opt[cur as usize].litlen == 0,
            );
        } else {
            *rep = last_stretch.rep;
            cur -= last_stretch.litlen;
        }

        let store_end = cur + 2;
        let mut store_start;
        let mut stretch_pos = cur;
        if last_stretch.litlen > 0 {
            opt[store_end as usize].litlen = last_stretch.litlen;
            opt[store_end as usize].mlen = 0;
            store_start = store_end - 1;
            opt[store_start as usize] = last_stretch;
        }
        opt[store_end as usize] = last_stretch;
        store_start = store_end;
        loop {
            let next_stretch = opt[stretch_pos as usize];
            opt[store_start as usize].litlen = next_stretch.litlen;
            if next_stretch.mlen == 0 {
                break;
            }
            store_start -= 1;
            opt[store_start as usize] = next_stretch;
            stretch_pos -= next_stretch.litlen + next_stretch.mlen;
        }

        for store_pos in store_start..=store_end {
            let llen = opt[store_pos as usize].litlen;
            let mlen = opt[store_pos as usize].mlen;
            let off_base = opt[store_pos as usize].off;
            let advance = llen + mlen;
            if mlen == 0 {
                ip = anchor + llen;
                continue;
            }
            let literals = prefix.slice(anchor, anchor + llen);
            ms.opt.update_stats(literals, off_base, mlen);
            seq_store.store_seq(literals, off_base, mlen as usize);
            anchor += advance;
            ip = anchor;
        }
        ms.opt.set_base_prices(opt_level);
    }

    ms.opt.price_table = opt;
    ms.opt.match_table = matches;
    if let Some((pos, pos_in_sequence)) = lent_cursor {
        let mut store = opt_ldm.seq_store;
        store.pos = pos;
        store.pos_in_sequence = pos_in_sequence;
        ms.ldm_seq_store = Some(store);
    }
    (iend - anchor) as usize
}

/// `ZSTD_compressBlock_btopt`.
pub(crate) fn btopt(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    dispatch!(block.backend.0, simd => opt_generic::<_, 0, false>(simd, ms, seq_store, rep, block))
}

/// `ZSTD_compressBlock_btopt_extDict`.
pub(crate) fn btopt_ext(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    dispatch!(block.backend.0, simd => opt_generic::<_, 0, true>(simd, ms, seq_store, rep, block))
}

/// `ZSTD_compressBlock_btultra`.
pub(crate) fn btultra(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    dispatch!(block.backend.0, simd => opt_generic::<_, 2, false>(simd, ms, seq_store, rep, block))
}

/// `ZSTD_compressBlock_btultra_extDict` (also btultra2 over an external
/// dictionary).
pub(crate) fn btultra_ext(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    dispatch!(block.backend.0, simd => opt_generic::<_, 2, true>(simd, ms, seq_store, rep, block))
}

/// `ZSTD_compressBlock_btultra2`: a first pass over the frame's first block
/// seeds the statistics, then the block is compressed for real.
pub(crate) fn btultra2(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    let curr = block.start;
    let src_size = block.end - block.start;
    // The reference's "no LDM" test only checks that no sequence was stored
    // yet; long matches offered as candidates do not disable the first pass.
    if ms.opt.lit_length_sum == 0
        && seq_store.nb_seq() == 0
        && ms.window.dict_limit == ms.window.low_limit
        && curr == ms.window.dict_limit
        && src_size as usize > PREDEF_THRESHOLD
    {
        // ZSTD_initStats_ultra.
        let mut tmp_rep = *rep;
        btultra(ms, seq_store, &mut tmp_rep, block);
        seq_store.reset();
        ms.window.base = ms.window.base.add(-i64::from(src_size));
        ms.window.dict_limit += src_size;
        ms.window.low_limit = ms.window.dict_limit;
        ms.next_to_update = ms.window.dict_limit;
        let shifted = BlockInput {
            window: mem::Window {
                prefix: block
                    .window
                    .prefix
                    .rebased(block.window.prefix.start() + src_size),
                ext: mem::Mem::new(&[], ms.window.dict_limit),
            },
            start: block.start + src_size,
            end: block.end + src_size,
            backend: block.backend,
        };
        return btultra(ms, seq_store, rep, &shifted);
    }
    btultra(ms, seq_store, rep, block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Backend;
    use crate::shared::format::BLOCK_SIZE_MAX;
    use alloc::vec;

    /// The reference's `ZSTD_litLengthPrice`, from the frequencies.
    fn reference_lit_length_price(os: &OptState, lit_length: u32, opt_level: u32) -> i32 {
        if os.price_type == PriceType::Predef {
            return weight(lit_length, opt_level) as i32;
        }
        if lit_length as usize == BLOCK_SIZE_MAX {
            return BITCOST_MULTIPLIER as i32
                + reference_lit_length_price(os, lit_length - 1, opt_level);
        }
        let code = usize::from(ll_code(lit_length));
        (u32::from(LL_BITS[code]) * BITCOST_MULTIPLIER + os.lit_length_sum_base_price)
            .wrapping_sub(weight(os.lit_length_freq[code], opt_level)) as i32
    }

    /// The reference's `ZSTD_getMatchPrice`, from the frequencies.
    fn reference_match_price(os: &OptState, off_base: u32, length: u32, opt_level: u32) -> u32 {
        let off_code = highbit32(off_base);
        let ml_base = length - MIN_MATCH as u32;
        if os.price_type == PriceType::Predef {
            return weight(ml_base, opt_level) + (16 + off_code) * BITCOST_MULTIPLIER;
        }
        let mut price = off_code * BITCOST_MULTIPLIER
            + os.off_code_sum_base_price
                .wrapping_sub(weight(os.off_code_freq[off_code as usize], opt_level));
        if opt_level < 2 && off_code >= 20 {
            price += (off_code - 19) * 2 * BITCOST_MULTIPLIER;
        }
        let code = usize::from(ml_code(ml_base));
        price = price.wrapping_add(
            u32::from(ML_BITS[code]) * BITCOST_MULTIPLIER
                + os.match_length_sum_base_price
                    .wrapping_sub(weight(os.match_length_freq[code], opt_level)),
        );
        price + BITCOST_MULTIPLIER / 5
    }

    /// The reference's `ZSTD_rawLiteralsCost` for one literal.
    fn reference_literal_price(os: &OptState, literal: u8, opt_level: u32) -> i32 {
        if !os.compressed_literals() {
            return (8 * BITCOST_MULTIPLIER) as i32;
        }
        if os.price_type == PriceType::Predef {
            return (6 * BITCOST_MULTIPLIER) as i32;
        }
        let max = os.lit_sum_base_price - BITCOST_MULTIPLIER;
        let price = weight(os.lit_freq[usize::from(literal)], opt_level).min(max);
        os.lit_sum_base_price.wrapping_sub(price) as i32
    }

    fn assert_prices_match(os: &OptState, opt_level: u32) {
        for literal in 0..=u8::MAX {
            assert_eq!(
                os.literal_price(literal),
                reference_literal_price(os, literal, opt_level),
                "literal {literal}"
            );
        }
        let lengths = (0..300u32).chain([1000, 65_535, 65_536, 131_071, BLOCK_SIZE_MAX as u32]);
        for lit_length in lengths {
            assert_eq!(
                os.lit_length_price(lit_length),
                reference_lit_length_price(os, lit_length, opt_level),
                "literal length {lit_length}"
            );
        }
        let offsets = (1..70u32).chain((4..32).map(|bit| (1u32 << bit) + 5));
        for off_base in offsets {
            for length in (3..200u32).chain([1000, 65_538, 131_074]) {
                assert_eq!(
                    os.match_price(off_base, length),
                    reference_match_price(os, off_base, length, opt_level),
                    "offset {off_base} length {length}"
                );
            }
        }
    }

    fn sample(len: usize, seed: u32) -> Vec<u8> {
        let mut state = seed;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (state >> 24) as u8 & 0x3f
            })
            .collect()
    }

    #[test]
    fn price_tables_match_the_reference_formulas() {
        for opt_level in [0, 2] {
            for literal_compression in [ParamSwitch::Auto, ParamSwitch::Disable] {
                let mut os = OptState {
                    literal_compression,
                    ..OptState::default()
                };
                os.allocate();
                let src = sample(4096, opt_level + 1);
                os.rescale_freqs(&src, opt_level);
                assert_eq!(os.price_type, PriceType::Dynamic);
                assert_prices_match(&os, opt_level);
                // Statistics updated sequence by sequence, then rescaled.
                for (round, chunk) in src.chunks(37).enumerate() {
                    let round = round as u32;
                    let off_base = if round.is_multiple_of(3) {
                        2
                    } else {
                        4 + (round << (round % 23))
                    };
                    let literals = &chunk[..(round as usize % 37).min(chunk.len())];
                    os.update_stats(literals, off_base, 3 + round * 7);
                    os.set_base_prices(opt_level);
                }
                assert_prices_match(&os, opt_level);
                os.rescale_freqs(&src, opt_level);
                assert_prices_match(&os, opt_level);
            }
        }
    }

    #[test]
    fn table_weights_match_the_integer_weights() {
        // Every power of two and its neighbours up to 2^24, then values the
        // float form cannot hold, which take the integer fallback.
        let mut stats: Vec<u32> = (0..32)
            .flat_map(|bit| {
                let p = 1u32 << bit;
                [p.wrapping_sub(2), p.wrapping_sub(1), p, p.wrapping_add(1)]
            })
            .filter(|&stat| stat < (1 << 24))
            .collect();
        stats.extend(0..5000);
        for opt_level in [0, 2] {
            let mut out = vec![0u32; stats.len()];
            weights(&stats, opt_level, &mut out);
            for (&stat, &got) in stats.iter().zip(&out) {
                assert_eq!(
                    got,
                    weight(stat, opt_level),
                    "stat {stat} level {opt_level}"
                );
            }
            let large = [1u32 << 24, (1 << 24) + 1, u32::MAX - 1, 7];
            let mut out = [0u32; 4];
            weights(&large, opt_level, &mut out);
            for (&stat, &got) in large.iter().zip(&out) {
                assert_eq!(
                    got,
                    weight(stat, opt_level),
                    "stat {stat} level {opt_level}"
                );
            }
        }
    }

    #[test]
    fn position_tables_grow_with_the_block_up_to_the_parser_window() {
        let mut os = OptState::default();
        os.allocate();
        assert_eq!(os.table_bytes(), 0);
        os.reserve_positions(44);
        assert_eq!((os.price_table.len(), os.match_table.len()), (47, 47));
        // A smaller block keeps the larger tables.
        os.reserve_positions(5);
        assert_eq!(os.price_table.len(), 47);
        os.reserve_positions(BLOCK_SIZE_MAX);
        assert_eq!(
            (os.price_table.len(), os.match_table.len()),
            (OPT_SIZE, OPT_SIZE)
        );
    }

    #[test]
    fn tiny_blocks_use_predefined_prices() {
        for opt_level in [0, 2] {
            let mut os = OptState::default();
            os.allocate();
            os.rescale_freqs(b"tiny", opt_level);
            assert_eq!(os.price_type, PriceType::Predef);
            assert_prices_match(&os, opt_level);
        }
    }

    #[test]
    fn tree_comparisons_read_both_segments_like_the_reference() {
        // Repetitive bytes so that runs cross the segment boundary.
        let ext: Vec<u8> = (0..64u32).map(|i| (i % 5) as u8).collect();
        let mut prefix: Vec<u8> = (0..96u32).map(|i| (i % 5) as u8).collect();
        prefix[70] = 9;
        let dict_limit = 1000u32;
        let window = mem::Window {
            prefix: mem::Mem::new(&prefix, dict_limit),
            ext: mem::Mem::new(&ext, dict_limit - 64),
        };
        let end = dict_limit + prefix.len() as u32;
        for backend in Backend::available() {
            let block = BlockInput {
                window,
                start: dict_limit,
                end,
                backend,
            };
            dispatch!(backend.0, simd => {
                let ctx = Ctx::new(simd, &block, 4);
                for curr in dict_limit + 10..dict_limit + 40 {
                    let current = ctx.current(curr);
                    for candidate in dict_limit - 64..curr {
                        let full = window.count_virtual(curr, candidate, end);
                        for known in [0, 1, 7, 8, 9, 30].map(|k: usize| k.min(full)) {
                            let (length, smaller) =
                                ctx.extend::<true>(current, curr, candidate, known);
                            assert_eq!(length, full, "{backend} {curr} {candidate} {known}");
                            let expected = curr + (full as u32) < end
                                && window.byte_at(candidate + full as u32)
                                    < window.byte_at(curr + full as u32);
                            assert_eq!(smaller, expected, "{backend} {curr} {candidate}");
                            if candidate >= dict_limit {
                                let plain = ctx.extend::<false>(current, curr, candidate, known);
                                assert_eq!(plain, (length, smaller));
                            }
                        }
                    }
                }
            });
        }
    }

    #[test]
    fn specialized_hashes_match_the_shared_ones() {
        let bytes = sample(64, 7);
        for backend in Backend::available() {
            let block = BlockInput {
                window: mem::Window {
                    prefix: mem::Mem::new(&bytes, 100),
                    ext: mem::Mem::new(&[], 100),
                },
                start: 100,
                end: 164,
                backend,
            };
            let prefix = &block.window.prefix;
            dispatch!(backend.0, simd => {
                let ctx = Ctx::new(simd, &block, 4);
                for index in 100..156 {
                    for hash_log in [10, 17, 24] {
                        let expected = |mls| mem::hash_ptr(prefix, index, hash_log, mls);
                        assert_eq!(ctx.hash::<3>(index, hash_log), expected(3));
                        assert_eq!(ctx.hash::<4>(index, hash_log), expected(4));
                        assert_eq!(ctx.hash::<5>(index, hash_log), expected(5));
                        assert_eq!(ctx.hash::<6>(index, hash_log), expected(6));
                    }
                }
            });
        }
    }
}
