//! The `greedy`, `lazy`, `lazy2` and `btlazy2` strategies (`zstd_lazy.c`):
//! one parser driving three searches — hash chains, a binary tree of
//! sorted suffixes (DUBT), and the row-based tag matcher.
//!
//! The reference specializes every function on the window shape; here
//! `ext` selects the external-dictionary variant at run time. Positions are
//! window indices, read through [`mem::Window`] so a match may start in the
//! external dictionary and continue into the prefix.

use super::match_state::{BlockInput, DUBT_UNSORTED_MARK, MatchState, ROW_HASH_CACHE_SIZE};
use super::mem::{self, hash_ptr, hash_salted};
use super::params::ROW_HASH_TAG_BITS;
use super::seq_store::{SeqStore, offbase_to_offset, offset_to_offbase, repcode_to_offbase};
use crate::shared::format::highbit32;

/// `kSearchStrength`.
const SEARCH_STRENGTH: u32 = 8;
/// `kLazySkippingStep`.
const LAZY_SKIPPING_STEP: usize = 8;
/// Offset placeholder before any match is found.
const NO_OFFBASE: u32 = 999_999_999;
const ROW_HASH_TAG_MASK: u32 = (1 << ROW_HASH_TAG_BITS) - 1;
const ROW_HASH_CACHE_MASK: u32 = ROW_HASH_CACHE_SIZE as u32 - 1;

/// How candidates are found (`searchMethod_e`).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum SearchMethod {
    HashChain,
    BinaryTree,
    RowHash,
}

/// Per-block constants of a search.
#[derive(Copy, Clone, Debug)]
struct Ctx<'a> {
    window: mem::Window<'a>,
    /// Index one past the block.
    iend: u32,
    mls: u32,
    row_log: u32,
    ext: bool,
}

impl Ctx<'_> {
    fn dict_limit(&self) -> u32 {
        self.window.prefix.start()
    }
}

// ---------------------------------------------------------------------------
// Hash chain.

/// `ZSTD_insertAndFindFirstIndex_internal`.
fn insert_and_find_first_index(ms: &mut MatchState, ctx: &Ctx<'_>, ip: u32) -> u32 {
    let hash_log = ms.cparams.hash_log;
    let chain_mask = (1u32 << ms.cparams.chain_log) - 1;
    let prefix = ctx.window.prefix;
    let target = ip;
    let mut idx = ms.next_to_update;
    while idx < target {
        let h = hash_ptr(&prefix, idx, hash_log, ctx.mls);
        ms.chain_table[(idx & chain_mask) as usize] = ms.hash_table[h];
        ms.hash_table[h] = idx;
        idx += 1;
        if ms.lazy_skipping {
            break;
        }
    }
    ms.next_to_update = target;
    ms.hash_table[hash_ptr(&prefix, ip, hash_log, ctx.mls)]
}

/// `ZSTD_HcFindBestMatch`.
fn hc_find_best_match(ms: &mut MatchState, ctx: &Ctx<'_>, ip: u32, offset: &mut u32) -> usize {
    let cparams = ms.cparams;
    let chain_size = 1u32 << cparams.chain_log;
    let chain_mask = chain_size - 1;
    let dict_limit = ctx.dict_limit();
    let curr = ip;
    let max_distance = 1u32 << cparams.window_log;
    let lowest_valid = ms.window.low_limit;
    let within_max_distance = if curr.wrapping_sub(lowest_valid) > max_distance {
        curr - max_distance
    } else {
        lowest_valid
    };
    let low_limit = if ms.loaded_dict_end != 0 {
        lowest_valid
    } else {
        within_max_distance
    };
    let min_chain = curr.saturating_sub(chain_size);
    let mut nb_attempts = 1u32 << cparams.search_log;
    let mut ml: usize = 4 - 1;
    let window = &ctx.window;
    let prefix = window.prefix;

    let mut match_index = insert_and_find_first_index(ms, ctx, ip);
    while match_index >= low_limit && nb_attempts > 0 {
        let mut current_ml = 0usize;
        if !ctx.ext || match_index >= dict_limit {
            let back = ml as u32 - 3;
            if prefix.read32(match_index + back) == prefix.read32(ip + back) {
                current_ml = prefix.count(ip, match_index, ctx.iend);
            }
        } else if window.ext.read32(match_index) == prefix.read32(ip) {
            current_ml =
                window.count_2segments(ip + 4, match_index + 4, ctx.iend, dict_limit, dict_limit)
                    + 4;
        }
        if current_ml > ml {
            ml = current_ml;
            *offset = offset_to_offbase(curr - match_index);
            if ip as usize + current_ml == ctx.iend as usize {
                break;
            }
        }
        if match_index <= min_chain {
            break;
        }
        match_index = ms.chain_table[(match_index & chain_mask) as usize];
        nb_attempts -= 1;
    }
    ml
}

// ---------------------------------------------------------------------------
// Binary tree (DUBT).

/// `ZSTD_updateDUBT`.
fn update_dubt(ms: &mut MatchState, ctx: &Ctx<'_>, ip: u32) {
    let hash_log = ms.cparams.hash_log;
    let bt_mask = (1u32 << (ms.cparams.chain_log - 1)) - 1;
    let prefix = ctx.window.prefix;
    let target = ip;
    let mut idx = ms.next_to_update;
    while idx < target {
        let h = hash_ptr(&prefix, idx, hash_log, ctx.mls);
        let match_index = ms.hash_table[h];
        let slot = 2 * (idx & bt_mask) as usize;
        ms.hash_table[h] = idx;
        ms.chain_table[slot] = match_index;
        ms.chain_table[slot + 1] = DUBT_UNSORTED_MARK;
        idx += 1;
    }
    ms.next_to_update = target;
}

/// A tree link to write: a slot of the chain table, or nowhere.
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

/// `ZSTD_insertDUBT1`: sorts one inserted but unsorted position.
fn insert_dubt1(ms: &mut MatchState, ctx: &Ctx<'_>, curr: u32, mut nb_compares: u32, bt_low: u32) {
    let cparams = ms.cparams;
    let bt_mask = (1u32 << (cparams.chain_log - 1)) - 1;
    let dict_limit = ctx.dict_limit();
    let window = &ctx.window;
    let iend = if curr >= dict_limit {
        ctx.iend
    } else {
        dict_limit
    };
    let mut common_smaller = 0usize;
    let mut common_larger = 0usize;
    let mut smaller = Link::Slot(2 * (curr & bt_mask) as usize);
    let mut larger = Link::Slot(2 * (curr & bt_mask) as usize + 1);
    let bt = &mut ms.chain_table;
    let mut match_index = bt[2 * (curr & bt_mask) as usize];
    let window_valid = ms.window.low_limit;
    let max_distance = 1u32 << cparams.window_log;
    let window_low = if curr.wrapping_sub(window_valid) > max_distance {
        curr - max_distance
    } else {
        window_valid
    };

    while nb_compares > 0 && match_index > window_low {
        let next = 2 * (match_index & bt_mask) as usize;
        let mut match_length = common_smaller.min(common_larger);
        match_length += window.count_virtual(
            curr + match_length as u32,
            match_index + match_length as u32,
            iend,
        );
        if (curr as usize + match_length) == iend as usize {
            break;
        }
        let m = window.byte_at(match_index + match_length as u32);
        let c = window.byte_at(curr + match_length as u32);
        if m < c {
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
}

/// `ZSTD_DUBT_findBestMatch`.
fn dubt_find_best_match(ms: &mut MatchState, ctx: &Ctx<'_>, ip: u32, off_base: &mut u32) -> usize {
    let cparams = ms.cparams;
    let hash_log = cparams.hash_log;
    let prefix = ctx.window.prefix;
    let h = hash_ptr(&prefix, ip, hash_log, ctx.mls);
    let mut match_index = ms.hash_table[h];
    let curr = ip;
    let window_low = ms.lowest_match_index(curr, cparams.window_log);
    let bt_mask = (1u32 << (cparams.chain_log - 1)) - 1;
    let bt_low = curr.saturating_sub(bt_mask);
    let unsort_limit = bt_low.max(window_low);
    let mut nb_compares = 1u32 << cparams.search_log;
    let mut nb_candidates = nb_compares;
    let mut previous_candidate = 0u32;

    // Reach the end of the unsorted candidates list.
    {
        let bt = &mut ms.chain_table;
        let mut next_candidate = 2 * (match_index & bt_mask) as usize;
        while match_index > unsort_limit
            && bt[next_candidate + 1] == DUBT_UNSORTED_MARK
            && nb_candidates > 1
        {
            bt[next_candidate + 1] = previous_candidate;
            previous_candidate = match_index;
            match_index = bt[next_candidate];
            next_candidate = 2 * (match_index & bt_mask) as usize;
            nb_candidates -= 1;
        }
        // Nullify the last candidate if it is still unsorted.
        if match_index > unsort_limit && bt[next_candidate + 1] == DUBT_UNSORTED_MARK {
            bt[next_candidate] = 0;
            bt[next_candidate + 1] = 0;
        }
    }

    // Batch sort the stacked candidates.
    match_index = previous_candidate;
    while match_index != 0 {
        let next_candidate_idx = ms.chain_table[2 * (match_index & bt_mask) as usize + 1];
        insert_dubt1(ms, ctx, match_index, nb_candidates, unsort_limit);
        match_index = next_candidate_idx;
        nb_candidates += 1;
    }

    // Find the longest match.
    let window = &ctx.window;
    let mut common_smaller = 0usize;
    let mut common_larger = 0usize;
    let mut smaller = Link::Slot(2 * (curr & bt_mask) as usize);
    let mut larger = Link::Slot(2 * (curr & bt_mask) as usize + 1);
    let mut match_end_idx = curr + 8 + 1;
    let mut best_length = 0usize;

    match_index = ms.hash_table[h];
    ms.hash_table[h] = curr;
    let bt = &mut ms.chain_table;

    while nb_compares > 0 && match_index > window_low {
        let next = 2 * (match_index & bt_mask) as usize;
        let mut match_length = common_smaller.min(common_larger);
        match_length += window.count_virtual(
            ip + match_length as u32,
            match_index + match_length as u32,
            ctx.iend,
        );
        if match_length > best_length {
            if match_length as u32 > match_end_idx - match_index {
                match_end_idx = match_index + match_length as u32;
            }
            let gain = 4 * (match_length - best_length) as i32;
            let cost = (highbit32(curr - match_index + 1) as i32)
                .wrapping_sub(highbit32(*off_base) as i32);
            if gain > cost {
                best_length = match_length;
                *off_base = offset_to_offbase(curr - match_index);
            }
            if ip as usize + match_length == ctx.iend as usize {
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
    best_length
}

/// `ZSTD_BtFindBestMatch`.
fn bt_find_best_match(ms: &mut MatchState, ctx: &Ctx<'_>, ip: u32, off_base: &mut u32) -> usize {
    if ip < ms.next_to_update {
        return 0;
    }
    update_dubt(ms, ctx, ip);
    dubt_find_best_match(ms, ctx, ip, off_base)
}

// ---------------------------------------------------------------------------
// Row-based match finder.

/// `ZSTD_row_nextIndex`: the next slot of a row, which also becomes its head.
fn row_next_index(tag_table: &mut [u8], rel_row: usize, row_mask: u32) -> u32 {
    let mut next = u32::from(tag_table[rel_row]).wrapping_sub(1) & row_mask;
    if next == 0 {
        next += row_mask;
    }
    tag_table[rel_row] = next as u8;
    next
}

/// Hash of the row match finder, salted as the reference's is.
fn row_hash(ms: &MatchState, ctx: &Ctx<'_>, index: u32) -> u32 {
    hash_salted(
        &ctx.window.prefix,
        index,
        ms.row_hash_log + ROW_HASH_TAG_BITS,
        ctx.mls,
        0,
    ) as u32
}

/// `ZSTD_row_fillHashCache`.
fn row_fill_hash_cache(ms: &mut MatchState, ctx: &Ctx<'_>, mut idx: u32, i_limit: i64) {
    let max_elems = if i64::from(idx) > i_limit {
        0
    } else {
        (i_limit - i64::from(idx) + 1) as u32
    };
    let lim = idx + (ROW_HASH_CACHE_SIZE as u32).min(max_elems);
    while idx < lim {
        let hash = row_hash(ms, ctx, idx);
        ms.hash_cache[(idx & ROW_HASH_CACHE_MASK) as usize] = hash;
        idx += 1;
    }
}

/// `ZSTD_row_nextCachedHash`.
fn row_next_cached_hash(ms: &mut MatchState, ctx: &Ctx<'_>, idx: u32) -> u32 {
    let new_hash = row_hash(ms, ctx, idx + ROW_HASH_CACHE_SIZE as u32);
    let slot = (idx & ROW_HASH_CACHE_MASK) as usize;
    let hash = ms.hash_cache[slot];
    ms.hash_cache[slot] = new_hash;
    hash
}

/// `ZSTD_row_update_internalImpl` with the hash cache.
fn row_update_impl(ms: &mut MatchState, ctx: &Ctx<'_>, mut start: u32, end: u32, row_mask: u32) {
    while start < end {
        let hash = row_next_cached_hash(ms, ctx, start);
        let rel_row = ((hash >> ROW_HASH_TAG_BITS) << ctx.row_log) as usize;
        let pos = row_next_index(&mut ms.tag_table, rel_row, row_mask) as usize;
        ms.tag_table[rel_row + pos] = (hash & ROW_HASH_TAG_MASK) as u8;
        ms.hash_table[rel_row + pos] = start;
        start += 1;
    }
}

/// `ZSTD_row_update_internal` with the hash cache.
fn row_update(ms: &mut MatchState, ctx: &Ctx<'_>, ip: u32, row_mask: u32) {
    const SKIP_THRESHOLD: u32 = 384;
    const MAX_MATCH_START_POSITIONS_TO_UPDATE: u32 = 96;
    const MAX_MATCH_END_POSITIONS_TO_UPDATE: u32 = 32;
    let mut idx = ms.next_to_update;
    let target = ip;
    if target - idx > SKIP_THRESHOLD {
        let bound = idx + MAX_MATCH_START_POSITIONS_TO_UPDATE;
        row_update_impl(ms, ctx, idx, bound, row_mask);
        idx = target - MAX_MATCH_END_POSITIONS_TO_UPDATE;
        row_fill_hash_cache(ms, ctx, idx, i64::from(ip) + 1);
    }
    row_update_impl(ms, ctx, idx, target, row_mask);
    ms.next_to_update = target;
}

/// `ZSTD_RowFindBestMatch`.
fn row_find_best_match(ms: &mut MatchState, ctx: &Ctx<'_>, ip: u32, offset: &mut u32) -> usize {
    let cparams = ms.cparams;
    let dict_limit = ctx.dict_limit();
    let window = &ctx.window;
    let prefix = window.prefix;
    let curr = ip;
    let max_distance = 1u32 << cparams.window_log;
    let lowest_valid = ms.window.low_limit;
    let within_max_distance = if curr.wrapping_sub(lowest_valid) > max_distance {
        curr - max_distance
    } else {
        lowest_valid
    };
    let low_limit = if ms.loaded_dict_end != 0 {
        lowest_valid
    } else {
        within_max_distance
    };
    let row_log = ctx.row_log;
    let row_entries = 1u32 << row_log;
    let row_mask = row_entries - 1;
    let capped_search_log = cparams.search_log.min(row_log);
    let mut nb_attempts = 1u32 << capped_search_log;
    let mut ml: usize = 4 - 1;

    let hash = if !ms.lazy_skipping {
        row_update(ms, ctx, ip, row_mask);
        row_next_cached_hash(ms, ctx, curr)
    } else {
        ms.next_to_update = curr;
        row_hash(ms, ctx, ip)
    };

    let rel_row = ((hash >> ROW_HASH_TAG_BITS) << row_log) as usize;
    let tag = (hash & ROW_HASH_TAG_MASK) as u8;
    let head = u32::from(ms.tag_table[rel_row]) & row_mask;
    let mut match_buffer = [0u32; 64];
    let mut num_matches = 0usize;
    for k in 0..row_entries {
        if nb_attempts == 0 {
            break;
        }
        let match_pos = (head + k) & row_mask;
        if ms.tag_table[rel_row + match_pos as usize] != tag {
            continue;
        }
        if match_pos == 0 {
            continue;
        }
        let match_index = ms.hash_table[rel_row + match_pos as usize];
        if match_index < low_limit {
            break;
        }
        match_buffer[num_matches] = match_index;
        num_matches += 1;
        nb_attempts -= 1;
    }

    // Insert the current position too.
    {
        let pos = row_next_index(&mut ms.tag_table, rel_row, row_mask) as usize;
        ms.tag_table[rel_row + pos] = tag;
        ms.hash_table[rel_row + pos] = ms.next_to_update;
        ms.next_to_update += 1;
    }

    for &match_index in &match_buffer[..num_matches] {
        let mut current_ml = 0usize;
        if !ctx.ext || match_index >= dict_limit {
            let back = ml as u32 - 3;
            if prefix.read32(match_index + back) == prefix.read32(ip + back) {
                current_ml = prefix.count(ip, match_index, ctx.iend);
            }
        } else if window.ext.read32(match_index) == prefix.read32(ip) {
            current_ml =
                window.count_2segments(ip + 4, match_index + 4, ctx.iend, dict_limit, dict_limit)
                    + 4;
        }
        if current_ml > ml {
            ml = current_ml;
            *offset = offset_to_offbase(curr - match_index);
            if ip as usize + current_ml == ctx.iend as usize {
                break;
            }
        }
    }
    ml
}

/// `ZSTD_searchMax`.
fn search_max(
    ms: &mut MatchState,
    ctx: &Ctx<'_>,
    method: SearchMethod,
    ip: u32,
    offset: &mut u32,
) -> usize {
    match method {
        SearchMethod::HashChain => hc_find_best_match(ms, ctx, ip, offset),
        SearchMethod::BinaryTree => bt_find_best_match(ms, ctx, ip, offset),
        SearchMethod::RowHash => row_find_best_match(ms, ctx, ip, offset),
    }
}

// ---------------------------------------------------------------------------
// Parsers.

/// `(int)(a*m - highbit32(offBase) + k)` as the reference computes it.
fn gain(length: usize, multiplier: i64, off_base: u32, bonus: i64) -> i32 {
    (length as i64 * multiplier - i64::from(highbit32(off_base)) + bonus) as i32
}

/// `ZSTD_compressBlock_lazy_generic` for the no-dictionary window.
fn lazy_generic(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
    method: SearchMethod,
    depth: u32,
) -> usize {
    let prefix = block.window.prefix;
    let istart = i64::from(block.start);
    let iend = i64::from(block.end);
    let ilimit = if method == SearchMethod::RowHash {
        iend - 8 - ROW_HASH_CACHE_SIZE as i64
    } else {
        iend - 8
    };
    let prefix_lowest = i64::from(ms.window.dict_limit);
    let ctx = Ctx {
        window: block.window,
        iend: block.end,
        mls: ms.cparams.min_match.clamp(4, 6),
        row_log: ms.cparams.search_log.clamp(4, 6),
        ext: false,
    };
    let read32 = |p: i64| prefix.read32(p as u32);
    let count = |a: i64, b: i64| prefix.count(a as u32, b as u32, block.end);
    let mut offset_1 = rep[0];
    let mut offset_2 = rep[1];
    let mut offset_saved1 = 0u32;
    let mut offset_saved2 = 0u32;

    let mut ip = istart;
    let mut anchor = istart;
    ip += i64::from(ip == prefix_lowest);
    {
        let curr = ip as u32;
        let window_low = ms.lowest_prefix_index(curr, ms.cparams.window_log);
        let max_rep = curr - window_low;
        if offset_2 > max_rep {
            offset_saved2 = offset_2;
            offset_2 = 0;
        }
        if offset_1 > max_rep {
            offset_saved1 = offset_1;
            offset_1 = 0;
        }
    }
    ms.lazy_skipping = false;
    if method == SearchMethod::RowHash {
        let next = ms.next_to_update;
        row_fill_hash_cache(ms, &ctx, next, ilimit);
    }

    while ip < ilimit {
        let mut match_length = 0usize;
        let mut off_base = repcode_to_offbase(1);
        let mut start = ip + 1;
        let mut store_now = false;

        if offset_1 > 0 && read32(ip + 1 - i64::from(offset_1)) == read32(ip + 1) {
            match_length = count(ip + 1 + 4, ip + 1 + 4 - i64::from(offset_1)) + 4;
            if depth == 0 {
                store_now = true;
            }
        }

        if !store_now {
            let mut found = NO_OFFBASE;
            let ml2 = search_max(ms, &ctx, method, ip as u32, &mut found);
            if ml2 > match_length {
                match_length = ml2;
                start = ip;
                off_base = found;
            }

            if match_length < 4 {
                let step = ((ip - anchor) >> SEARCH_STRENGTH) as usize + 1;
                ip += step as i64;
                ms.lazy_skipping = step > LAZY_SKIPPING_STEP;
                continue;
            }

            if depth >= 1 {
                while ip < ilimit {
                    ip += 1;
                    if off_base != 0
                        && offset_1 > 0
                        && read32(ip) == read32(ip - i64::from(offset_1))
                    {
                        let ml_rep = count(ip + 4, ip + 4 - i64::from(offset_1)) + 4;
                        let gain2 = (ml_rep * 3) as i32;
                        let gain1 = gain(match_length, 3, off_base, 1);
                        if ml_rep >= 4 && gain2 > gain1 {
                            match_length = ml_rep;
                            off_base = repcode_to_offbase(1);
                            start = ip;
                        }
                    }
                    {
                        let mut candidate = NO_OFFBASE;
                        let ml2 = search_max(ms, &ctx, method, ip as u32, &mut candidate);
                        let gain2 = gain(ml2, 4, candidate, 0);
                        let gain1 = gain(match_length, 4, off_base, 4);
                        if ml2 >= 4 && gain2 > gain1 {
                            match_length = ml2;
                            off_base = candidate;
                            start = ip;
                            continue;
                        }
                    }
                    if depth == 2 && ip < ilimit {
                        ip += 1;
                        if off_base != 0
                            && offset_1 > 0
                            && read32(ip) == read32(ip - i64::from(offset_1))
                        {
                            let ml_rep = count(ip + 4, ip + 4 - i64::from(offset_1)) + 4;
                            let gain2 = (ml_rep * 4) as i32;
                            let gain1 = gain(match_length, 4, off_base, 1);
                            if ml_rep >= 4 && gain2 > gain1 {
                                match_length = ml_rep;
                                off_base = repcode_to_offbase(1);
                                start = ip;
                            }
                        }
                        let mut candidate = NO_OFFBASE;
                        let ml2 = search_max(ms, &ctx, method, ip as u32, &mut candidate);
                        let gain2 = gain(ml2, 4, candidate, 0);
                        let gain1 = gain(match_length, 4, off_base, 7);
                        if ml2 >= 4 && gain2 > gain1 {
                            match_length = ml2;
                            off_base = candidate;
                            start = ip;
                            continue;
                        }
                    }
                    break;
                }
            }

            // Catch up.
            if off_base > 3 {
                let offset = i64::from(offbase_to_offset(off_base));
                while start > anchor
                    && start - offset > prefix_lowest
                    && prefix.byte((start - 1) as u32) == prefix.byte((start - offset - 1) as u32)
                {
                    start -= 1;
                    match_length += 1;
                }
                offset_2 = offset_1;
                offset_1 = offbase_to_offset(off_base);
            }
        }

        // _storeSequence:
        seq_store.store_seq(
            prefix.slice(anchor as u32, start as u32),
            off_base,
            match_length,
        );
        ip = start + match_length as i64;
        anchor = ip;
        if ms.lazy_skipping {
            if method == SearchMethod::RowHash {
                let next = ms.next_to_update;
                row_fill_hash_cache(ms, &ctx, next, ilimit);
            }
            ms.lazy_skipping = false;
        }

        while ip <= ilimit && offset_2 > 0 && read32(ip) == read32(ip - i64::from(offset_2)) {
            let ml = count(ip + 4, ip + 4 - i64::from(offset_2)) + 4;
            core::mem::swap(&mut offset_1, &mut offset_2);
            seq_store.store_seq(&[], repcode_to_offbase(1), ml);
            ip += ml as i64;
            anchor = ip;
        }
    }

    offset_saved2 = if offset_saved1 != 0 && offset_1 != 0 {
        offset_saved1
    } else {
        offset_saved2
    };
    rep[0] = if offset_1 != 0 {
        offset_1
    } else {
        offset_saved1
    };
    rep[1] = if offset_2 != 0 {
        offset_2
    } else {
        offset_saved2
    };
    (iend - anchor) as usize
}

/// `ZSTD_compressBlock_lazy_extDict_generic`.
fn lazy_ext_generic(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
    method: SearchMethod,
    depth: u32,
) -> usize {
    let window = block.window;
    let prefix = window.prefix;
    let istart = i64::from(block.start);
    let iend = i64::from(block.end);
    let ilimit = if method == SearchMethod::RowHash {
        iend - 8 - ROW_HASH_CACHE_SIZE as i64
    } else {
        iend - 8
    };
    let dict_limit = ms.window.dict_limit;
    let prefix_start = i64::from(dict_limit);
    let dict_start = ms.window.low_limit;
    let window_log = ms.cparams.window_log;
    let ctx = Ctx {
        window,
        iend: block.end,
        mls: ms.cparams.min_match.clamp(4, 6),
        row_log: ms.cparams.search_log.clamp(4, 6),
        ext: true,
    };
    let mut offset_1 = rep[0];
    let mut offset_2 = rep[1];
    // A repeat-offset candidate at `rep_index` for position `curr`.
    let rep_ok = |ms: &MatchState, curr: u32, offset: u32| {
        let window_low = ms.lowest_match_index(curr, window_log);
        let rep_index = curr.wrapping_sub(offset);
        let overlap_ok = dict_limit.wrapping_sub(1).wrapping_sub(rep_index) >= 3;
        if overlap_ok
            && offset <= curr - window_low
            && window.read32_at(rep_index) == prefix.read32(curr)
        {
            Some(window.count_virtual(curr + 4, rep_index + 4, block.end) + 4)
        } else {
            None
        }
    };

    ms.lazy_skipping = false;
    let mut ip = istart;
    let mut anchor = istart;
    ip += i64::from(ip == prefix_start);
    if method == SearchMethod::RowHash {
        let next = ms.next_to_update;
        row_fill_hash_cache(ms, &ctx, next, ilimit);
    }

    while ip < ilimit {
        let mut match_length = 0usize;
        let mut off_base = repcode_to_offbase(1);
        let mut start = ip + 1;
        let mut curr = ip as u32;
        let mut store_now = false;

        if let Some(length) = rep_ok(ms, curr + 1, offset_1) {
            match_length = length;
            if depth == 0 {
                store_now = true;
            }
        }

        if !store_now {
            let mut candidate = NO_OFFBASE;
            let ml2 = search_max(ms, &ctx, method, ip as u32, &mut candidate);
            if ml2 > match_length {
                match_length = ml2;
                start = ip;
                off_base = candidate;
            }
            if match_length < 4 {
                let step = ((ip - anchor) >> SEARCH_STRENGTH) as usize;
                ip += step as i64 + 1;
                ms.lazy_skipping = step > LAZY_SKIPPING_STEP;
                continue;
            }
            if depth >= 1 {
                while ip < ilimit {
                    ip += 1;
                    curr += 1;
                    if off_base != 0
                        && let Some(rep_length) = rep_ok(ms, curr, offset_1)
                    {
                        let gain2 = (rep_length * 3) as i32;
                        let gain1 = gain(match_length, 3, off_base, 1);
                        if rep_length >= 4 && gain2 > gain1 {
                            match_length = rep_length;
                            off_base = repcode_to_offbase(1);
                            start = ip;
                        }
                    }
                    {
                        let mut candidate = NO_OFFBASE;
                        let ml2 = search_max(ms, &ctx, method, ip as u32, &mut candidate);
                        let gain2 = gain(ml2, 4, candidate, 0);
                        let gain1 = gain(match_length, 4, off_base, 4);
                        if ml2 >= 4 && gain2 > gain1 {
                            match_length = ml2;
                            off_base = candidate;
                            start = ip;
                            continue;
                        }
                    }
                    if depth == 2 && ip < ilimit {
                        ip += 1;
                        curr += 1;
                        if off_base != 0
                            && let Some(rep_length) = rep_ok(ms, curr, offset_1)
                        {
                            let gain2 = (rep_length * 4) as i32;
                            let gain1 = gain(match_length, 4, off_base, 1);
                            if rep_length >= 4 && gain2 > gain1 {
                                match_length = rep_length;
                                off_base = repcode_to_offbase(1);
                                start = ip;
                            }
                        }
                        let mut candidate = NO_OFFBASE;
                        let ml2 = search_max(ms, &ctx, method, ip as u32, &mut candidate);
                        let gain2 = gain(ml2, 4, candidate, 0);
                        let gain1 = gain(match_length, 4, off_base, 7);
                        if ml2 >= 4 && gain2 > gain1 {
                            match_length = ml2;
                            off_base = candidate;
                            start = ip;
                            continue;
                        }
                    }
                    break;
                }
            }

            if off_base > 3 {
                let match_index = (start as u32).wrapping_sub(offbase_to_offset(off_base));
                let mut m = match_index;
                let m_start = if match_index < dict_limit {
                    dict_start
                } else {
                    dict_limit
                };
                while start > anchor
                    && m > m_start
                    && prefix.byte((start - 1) as u32) == window.byte_at(m - 1)
                {
                    start -= 1;
                    m -= 1;
                    match_length += 1;
                }
                offset_2 = offset_1;
                offset_1 = offbase_to_offset(off_base);
            }
        }

        seq_store.store_seq(
            prefix.slice(anchor as u32, start as u32),
            off_base,
            match_length,
        );
        ip = start + match_length as i64;
        anchor = ip;
        if ms.lazy_skipping {
            if method == SearchMethod::RowHash {
                let next = ms.next_to_update;
                row_fill_hash_cache(ms, &ctx, next, ilimit);
            }
            ms.lazy_skipping = false;
        }

        while ip <= ilimit {
            let Some(ml) = rep_ok(ms, ip as u32, offset_2) else {
                break;
            };
            core::mem::swap(&mut offset_1, &mut offset_2);
            seq_store.store_seq(&[], repcode_to_offbase(1), ml);
            ip += ml as i64;
            anchor = ip;
        }
    }
    rep[0] = offset_1;
    rep[1] = offset_2;
    (iend - anchor) as usize
}

macro_rules! block_compressors {
    ($($name:ident, $ext_name:ident => $method:expr, $depth:expr;)*) => {$(
        pub(crate) fn $name(
            ms: &mut MatchState,
            seq_store: &mut SeqStore,
            rep: &mut [u32; 3],
            block: &BlockInput<'_>,
        ) -> usize {
            lazy_generic(ms, seq_store, rep, block, $method, $depth)
        }

        pub(crate) fn $ext_name(
            ms: &mut MatchState,
            seq_store: &mut SeqStore,
            rep: &mut [u32; 3],
            block: &BlockInput<'_>,
        ) -> usize {
            lazy_ext_generic(ms, seq_store, rep, block, $method, $depth)
        }
    )*};
}

block_compressors! {
    greedy, greedy_ext => SearchMethod::HashChain, 0;
    lazy, lazy_ext => SearchMethod::HashChain, 1;
    lazy2, lazy2_ext => SearchMethod::HashChain, 2;
    btlazy2, btlazy2_ext => SearchMethod::BinaryTree, 2;
    greedy_row, greedy_row_ext => SearchMethod::RowHash, 0;
    lazy_row, lazy_row_ext => SearchMethod::RowHash, 1;
    lazy2_row, lazy2_row_ext => SearchMethod::RowHash, 2;
}
