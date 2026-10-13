//! The optimal parser: `btopt`, `btultra` and `btultra2` (`zstd_opt.c`).
//!
//! Prices are fixed-point bit costs (1/256 bit) derived from running symbol
//! statistics, exactly as the reference computes them, so the chosen parse
//! — and therefore every emitted byte — matches it.

use alloc::vec;
use alloc::vec::Vec;

use super::entropy::hist;
use super::ldm::RawSeqStore;
use super::match_state::{BlockInput, MatchState};
use super::mem::{self, hash_ptr, hash3_ptr};
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
}

impl OptState {
    /// Sizes the tables (`ZSTD_reset_matchState`'s opt space).
    pub(crate) fn allocate(&mut self) {
        self.lit_freq.resize(MAX_LIT as usize + 1, 0);
        self.lit_length_freq.resize(MAX_LL as usize + 1, 0);
        self.match_length_freq.resize(MAX_ML as usize + 1, 0);
        self.off_code_freq.resize(MAX_OFF as usize + 1, 0);
        self.match_table.resize(OPT_SIZE, Match::default());
        self.price_table.resize(OPT_SIZE, Optimal::default());
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
    }

    /// `ZSTD_rawLiteralsCost` for one literal.
    fn literal_price(&self, literal: u8, opt_level: u32) -> i32 {
        if !self.compressed_literals() {
            return ((1 << 3) * BITCOST_MULTIPLIER) as i32;
        }
        if self.price_type == PriceType::Predef {
            return (6 * BITCOST_MULTIPLIER) as i32;
        }
        let lit_price_max = self.lit_sum_base_price - BITCOST_MULTIPLIER;
        let lit_price = weight(self.lit_freq[usize::from(literal)], opt_level).min(lit_price_max);
        self.lit_sum_base_price.wrapping_sub(lit_price) as i32
    }

    /// `ZSTD_litLengthPrice`.
    fn lit_length_price(&self, lit_length: u32, opt_level: u32) -> i32 {
        if self.price_type == PriceType::Predef {
            return weight(lit_length, opt_level) as i32;
        }
        if lit_length as usize == crate::shared::format::BLOCK_SIZE_MAX {
            return BITCOST_MULTIPLIER as i32 + self.lit_length_price(lit_length - 1, opt_level);
        }
        let code = usize::from(ll_code(lit_length));
        (u32::from(LL_BITS[code]) * BITCOST_MULTIPLIER + self.lit_length_sum_base_price)
            .wrapping_sub(weight(self.lit_length_freq[code], opt_level)) as i32
    }

    /// `ZSTD_getMatchPrice`.
    fn match_price(&self, off_base: u32, match_length: u32, opt_level: u32) -> u32 {
        let off_code = highbit32(off_base);
        let ml_base = match_length - MIN_MATCH as u32;
        if self.price_type == PriceType::Predef {
            return weight(ml_base, opt_level) + (16 + off_code) * BITCOST_MULTIPLIER;
        }
        let mut price = off_code * BITCOST_MULTIPLIER
            + self
                .off_code_sum_base_price
                .wrapping_sub(weight(self.off_code_freq[off_code as usize], opt_level));
        if opt_level < 2 && off_code >= 20 {
            price += (off_code - 19) * 2 * BITCOST_MULTIPLIER;
        }
        let code = usize::from(ml_code(ml_base));
        price = price.wrapping_add(
            u32::from(ML_BITS[code]) * BITCOST_MULTIPLIER
                + self
                    .match_length_sum_base_price
                    .wrapping_sub(weight(self.match_length_freq[code], opt_level)),
        );
        price + BITCOST_MULTIPLIER / 5
    }

    /// `ZSTD_updateStats`.
    fn update_stats(&mut self, literals: &[u8], off_base: u32, match_length: u32) {
        if self.compressed_literals() {
            for &lit in literals {
                self.lit_freq[usize::from(lit)] += LITFREQ_ADD;
            }
            self.lit_sum += literals.len() as u32 * LITFREQ_ADD;
        }
        let ll = usize::from(ll_code(literals.len() as u32));
        self.lit_length_freq[ll] += 1;
        self.lit_length_sum += 1;
        let off_code = highbit32(off_base) as usize;
        self.off_code_freq[off_code] += 1;
        self.off_code_sum += 1;
        let ml = usize::from(ml_code(match_length - MIN_MATCH as u32));
        self.match_length_freq[ml] += 1;
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
const fn weight(stat: u32, opt_level: u32) -> u32 {
    if opt_level != 0 {
        frac_weight(stat)
    } else {
        bit_weight(stat)
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
#[derive(Copy, Clone, Debug)]
struct Ctx<'a> {
    window: mem::Window<'a>,
    iend: u32,
    mls: u32,
    ext: bool,
}

/// `ZSTD_readMINMATCH` comparison of `length` (3 or 4) bytes.
fn min_match_equal(window: &mem::Window<'_>, a: u32, b: u32, length: u32) -> bool {
    let x = window.read32_at(a);
    let y = window.read32_at(b);
    if length == 3 {
        (x << 8) == (y << 8)
    } else {
        x == y
    }
}

/// `ZSTD_insertAndFindFirstIndexHash3`.
fn insert_and_find_first_index_hash3(
    ms: &mut MatchState,
    ctx: &Ctx<'_>,
    next_to_update3: &mut u32,
    ip: u32,
) -> u32 {
    let hash_log3 = ms.hash_log3;
    let prefix = ctx.window.prefix;
    let mut idx = *next_to_update3;
    let target = ip;
    let hash3 = hash3_ptr(&prefix, ip, hash_log3);
    while idx < target {
        ms.hash_table3[hash3_ptr(&prefix, idx, hash_log3)] = idx;
        idx += 1;
    }
    *next_to_update3 = target;
    ms.hash_table3[hash3]
}

#[derive(Copy, Clone)]
enum Link {
    Slot(usize),
    Dummy,
}

fn set_link(bt: &mut [u32], link: Link, value: u32) {
    if let Link::Slot(slot) = link {
        bt[slot] = value;
    }
}

/// `ZSTD_insertBt1`: returns how many positions it accounted for.
fn insert_bt1(ms: &mut MatchState, ctx: &Ctx<'_>, curr: u32, target: u32) -> u32 {
    let cparams = ms.cparams;
    let prefix = ctx.window.prefix;
    let h = hash_ptr(&prefix, curr, cparams.hash_log, ctx.mls);
    let bt_mask = (1u32 << (cparams.chain_log - 1)) - 1;
    let mut match_index = ms.hash_table[h];
    let mut common_smaller = 0usize;
    let mut common_larger = 0usize;
    let bt_low = curr.saturating_sub(bt_mask);
    let mut smaller = Link::Slot(2 * (curr & bt_mask) as usize);
    let mut larger = Link::Slot(2 * (curr & bt_mask) as usize + 1);
    let window_low = ms.lowest_match_index(target, cparams.window_log);
    let mut match_end_idx = curr + 8 + 1;
    let mut best_length = 8usize;
    let mut nb_compares = 1u32 << cparams.search_log;
    let window = &ctx.window;

    ms.hash_table[h] = curr;
    let bt = &mut ms.chain_table;
    while nb_compares > 0 && match_index >= window_low {
        let next = 2 * (match_index & bt_mask) as usize;
        let mut match_length = common_smaller.min(common_larger);
        match_length += window.count_virtual(
            curr + match_length as u32,
            match_index + match_length as u32,
            ctx.iend,
        );
        if match_length > best_length {
            best_length = match_length;
            if match_length as u32 > match_end_idx - match_index {
                match_end_idx = match_index + match_length as u32;
            }
        }
        if curr as usize + match_length == ctx.iend as usize {
            break;
        }
        if window.byte_at(match_index + match_length as u32)
            < window.byte_at(curr + match_length as u32)
        {
            set_link(bt, smaller, match_index);
            common_smaller = match_length;
            if match_index <= bt_low {
                smaller = Link::Dummy;
                break;
            }
            smaller = Link::Slot(next + 1);
            match_index = bt[next + 1];
        } else {
            set_link(bt, larger, match_index);
            common_larger = match_length;
            if match_index <= bt_low {
                larger = Link::Dummy;
                break;
            }
            larger = Link::Slot(next);
            match_index = bt[next];
        }
        nb_compares -= 1;
    }
    set_link(bt, smaller, 0);
    set_link(bt, larger, 0);
    let positions = if best_length > 384 {
        192.min((best_length - 384) as u32)
    } else {
        0
    };
    positions.max(match_end_idx - (curr + 8))
}

/// `ZSTD_updateTree_internal`.
fn update_tree(ms: &mut MatchState, ctx: &Ctx<'_>, ip: u32) {
    let target = ip;
    let mut idx = ms.next_to_update;
    while idx < target {
        idx += insert_bt1(ms, ctx, idx, target);
    }
    ms.next_to_update = target;
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
fn insert_bt_and_get_all_matches(
    ms: &mut MatchState,
    ctx: &Ctx<'_>,
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
    let mls = ctx.mls;
    let min_match = if mls == 3 { 3 } else { 4 };
    let window = &ctx.window;
    let prefix = window.prefix;
    let h = hash_ptr(&prefix, ip, cparams.hash_log, mls);
    let mut match_index = ms.hash_table[h];
    let bt_mask = (1u32 << (cparams.chain_log - 1)) - 1;
    let mut common_smaller = 0usize;
    let mut common_larger = 0usize;
    let dict_limit = prefix.start();
    let bt_low = curr.saturating_sub(bt_mask);
    let window_low = ms.lowest_match_index(curr, cparams.window_log);
    let match_low = if window_low != 0 { window_low } else { 1 };
    let mut smaller = Link::Slot(2 * (curr & bt_mask) as usize);
    let mut larger = Link::Slot(2 * (curr & bt_mask) as usize + 1);
    let mut match_end_idx = curr + 8 + 1;
    let mut mnum = 0u32;
    let mut nb_compares = 1u32 << cparams.search_log;
    let mut best_length = (length_to_beat - 1) as usize;
    let i_limit = ctx.iend;

    // Repeat offsets.
    {
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
                    && min_match_equal(window, ip, ip - rep_offset, min_match)
                {
                    rep_len = (prefix.count(ip + min_match, ip + min_match - rep_offset, i_limit)
                        as u32)
                        + min_match;
                }
            } else if ctx.ext
                && rep_offset.wrapping_sub(1) < curr - window_low
                && dict_limit.wrapping_sub(1).wrapping_sub(rep_index) >= 3
                && min_match_equal(window, ip, rep_index, min_match)
            {
                rep_len = window.count_2segments(
                    ip + min_match,
                    rep_index + min_match,
                    i_limit,
                    dict_limit,
                    dict_limit,
                ) as u32
                    + min_match;
            }
            if rep_len as usize > best_length {
                best_length = rep_len as usize;
                matches[mnum as usize] = Match {
                    off: repcode_to_offbase(rep_code - ll0 + 1),
                    len: rep_len,
                };
                mnum += 1;
                if rep_len > sufficient_len || ip + rep_len == i_limit {
                    return mnum;
                }
            }
        }
    }

    // HC3 match finder.
    if mls == 3 && best_length < mls as usize {
        let match_index3 = insert_and_find_first_index_hash3(ms, ctx, next_to_update3, ip);
        if match_index3 >= match_low && curr - match_index3 < (1 << 18) {
            let mlen = if !ctx.ext || match_index3 >= dict_limit {
                prefix.count(ip, match_index3, i_limit)
            } else {
                window.count_2segments(ip, match_index3, i_limit, dict_limit, dict_limit)
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
    let bt = &mut ms.chain_table;
    while nb_compares > 0 && match_index >= match_low {
        let next = 2 * (match_index & bt_mask) as usize;
        let mut match_length = common_smaller.min(common_larger);
        match_length += window.count_virtual(
            ip + match_length as u32,
            match_index + match_length as u32,
            i_limit,
        );
        if match_length > best_length {
            if match_length as u32 > match_end_idx - match_index {
                match_end_idx = match_index + match_length as u32;
            }
            best_length = match_length;
            matches[mnum as usize] = Match {
                off: offset_to_offbase(curr - match_index),
                len: match_length as u32,
            };
            mnum += 1;
            if match_length > OPT_NUM || ip as usize + match_length == i_limit as usize {
                break;
            }
        }
        if window.byte_at(match_index + match_length as u32)
            < window.byte_at(ip + match_length as u32)
        {
            set_link(bt, smaller, match_index);
            common_smaller = match_length;
            if match_index <= bt_low {
                smaller = Link::Dummy;
                break;
            }
            smaller = Link::Slot(next + 1);
            match_index = bt[next + 1];
        } else {
            set_link(bt, larger, match_index);
            common_larger = match_length;
            if match_index <= bt_low {
                larger = Link::Dummy;
                break;
            }
            larger = Link::Slot(next);
            match_index = bt[next];
        }
        nb_compares -= 1;
    }
    set_link(bt, smaller, 0);
    set_link(bt, larger, 0);
    ms.next_to_update = match_end_idx - 8;
    mnum
}

/// `ZSTD_btGetAllMatches_internal`.
fn get_all_matches(
    ms: &mut MatchState,
    ctx: &Ctx<'_>,
    matches: &mut [Match],
    next_to_update3: &mut u32,
    query: MatchQuery<'_>,
) -> u32 {
    if query.ip < ms.next_to_update {
        return 0;
    }
    update_tree(ms, ctx, query.ip);
    insert_bt_and_get_all_matches(ms, ctx, matches, next_to_update3, query)
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
    fn process(
        &mut self,
        matches: &mut [Match],
        nb_matches: &mut u32,
        curr_pos_in_block: u32,
        remaining: u32,
        min_match: u32,
    ) {
        if self.seq_store.size() == 0 || self.seq_store.pos >= self.seq_store.size() {
            return;
        }
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

/// `ZSTD_compressBlock_opt_generic`.
fn opt_generic(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; REP_NUM],
    block: &BlockInput<'_>,
    opt_level: u32,
    ext: bool,
) -> usize {
    let prefix = block.window.prefix;
    let istart = block.start;
    let iend = block.end;
    let ilimit = i64::from(iend) - 8;
    let prefix_start = ms.window.dict_limit;
    let cparams = ms.cparams;
    let sufficient_len = cparams.target_length.min(OPT_NUM as u32 - 1);
    let min_match = if cparams.min_match == 3 { 3 } else { 4 };
    let mut next_to_update3 = ms.next_to_update;
    let ctx = Ctx {
        window: block.window,
        iend,
        mls: cparams.min_match.clamp(3, 6),
        ext,
    };

    let mut opt = core::mem::take(&mut ms.opt.price_table);
    let mut matches = core::mem::take(&mut ms.opt.match_table);
    if opt.len() < OPT_SIZE {
        opt = vec![Optimal::default(); OPT_SIZE];
        matches = vec![Match::default(); OPT_SIZE];
    }
    let mut last_stretch = Optimal::default();
    let mut opt_ldm = OptLdm {
        seq_store: ms.ldm_seq_store.clone().unwrap_or_default(),
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
        let mut nb_matches = get_all_matches(
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
        opt[0].price = os.lit_length_price(litlen, opt_level);
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
            for m in &matches[..nb_matches as usize] {
                let off_base = m.off;
                let end = m.len;
                while pos <= end {
                    let match_price = os.match_price(off_base, pos, opt_level) as i32;
                    let sequence_price = opt[0].price + match_price;
                    let slot = &mut opt[pos as usize];
                    slot.mlen = pos;
                    slot.off = off_base;
                    slot.litlen = 0;
                    slot.price = sequence_price + os.lit_length_price(0, opt_level);
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
                    let ll_price = |l: u32| os.lit_length_price(l, opt_level);
                    let ll_inc_price = |l: u32| ll_price(l) - ll_price(l - 1);
                    let litlen = opt[cur as usize - 1].litlen + 1;
                    let price = opt[cur as usize - 1].price
                        + os.literal_price(prefix.byte(ip + cur - 1), opt_level)
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
                            let next_lit = os.literal_price(prefix.byte(ip + cur), opt_level);
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
                let base_price = previous_price + ms.opt.lit_length_price(0, opt_level);
                let cur_rep = opt[cur as usize].rep;
                let mut nb_matches = get_all_matches(
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
                    let mut mlen = last_ml;
                    while mlen >= start_ml {
                        let pos = cur + mlen;
                        let price = base_price + os.match_price(offset, mlen, opt_level) as i32;
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
    (iend - anchor) as usize
}

/// `ZSTD_compressBlock_btopt`.
pub(crate) fn btopt(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    opt_generic(ms, seq_store, rep, block, 0, false)
}

/// `ZSTD_compressBlock_btopt_extDict`.
pub(crate) fn btopt_ext(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    opt_generic(ms, seq_store, rep, block, 0, true)
}

/// `ZSTD_compressBlock_btultra`.
pub(crate) fn btultra(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    opt_generic(ms, seq_store, rep, block, 2, false)
}

/// `ZSTD_compressBlock_btultra_extDict` (also btultra2 over an external
/// dictionary).
pub(crate) fn btultra_ext(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    opt_generic(ms, seq_store, rep, block, 2, true)
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
        opt_generic(ms, seq_store, &mut tmp_rep, block, 2, false);
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
        };
        return opt_generic(ms, seq_store, rep, &shifted, 2, false);
    }
    opt_generic(ms, seq_store, rep, block, 2, false)
}
