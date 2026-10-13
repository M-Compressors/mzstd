//! The `greedy`, `lazy`, `lazy2` and `btlazy2` strategies (`zstd_lazy.c`):
//! one parser driving three searches — hash chains, a binary tree of
//! sorted suffixes (DUBT), and the row-based tag matcher.
//!
//! Like the reference, the parser is specialized on its search and depth,
//! and every search on the minimum match length (and the row finder on its
//! row size); a search picks its specialization with one `match` per call,
//! as `ZSTD_searchMax` does. The SIMD level is dispatched once per block, at
//! the entry points, and its token is passed down to the kernels: the row
//! finder's tag compare ([`row`]) and match-length counting ([`count`]).
//!
//! The reference also specializes every function on the window shape. Here
//! the parser has an extDict variant, and the candidate loops and the binary
//! tree take `EXT` as a constant chosen with one branch per search; the
//! prefix-only case reads the prefix's bytes directly. Positions are window
//! indices; an extDict candidate is read through [`mem::Window`] so a match
//! may start in the external dictionary and continue into the prefix.

mod count;
mod row;

use fearless_simd::Simd;

use super::match_state::{BlockInput, DUBT_UNSORTED_MARK, MatchState, ROW_HASH_CACHE_SIZE};
use super::mem::{self, Mem, hash_ptr};
use super::params::ROW_HASH_TAG_BITS;
use super::seq_store::{SeqStore, offbase_to_offset, offset_to_offbase, repcode_to_offbase};
use crate::shared::format::highbit32;

/// `kSearchStrength`.
const SEARCH_STRENGTH: u32 = 8;
/// `kLazySkippingStep`.
const LAZY_SKIPPING_STEP: usize = 8;
/// Offset placeholder before any match is found.
const NO_OFFBASE: u32 = 999_999_999;
const ROW_HASH_CACHE_MASK: u32 = ROW_HASH_CACHE_SIZE as u32 - 1;

/// Per-block constants of a search.
#[derive(Copy, Clone, Debug)]
struct Ctx<'a> {
    window: mem::Window<'a>,
    /// The prefix's bytes: index `i` is `src[i - base]`. They may extend
    /// past `iend`, which bounds every match.
    src: &'a [u8],
    /// Index of the prefix's first byte (`dictLimit`).
    base: u32,
    /// Index one past the block.
    iend: u32,
    mls: u32,
    row_log: u32,
    ext: bool,
}

impl<'a> Ctx<'a> {
    fn new(block: &BlockInput<'a>, mls: u32, row_log: u32, ext: bool) -> Self {
        let prefix = block.window.prefix;
        Self {
            window: block.window,
            src: prefix.slice(prefix.start(), prefix.end()),
            base: prefix.start(),
            iend: block.end,
            mls,
            row_log,
            ext,
        }
    }

    const fn dict_limit(&self) -> u32 {
        self.base
    }

    /// The little-endian `u32` at prefix index `index` (`MEM_read32`); a word
    /// past the prefix, which the reference's index invariants rule out,
    /// reads as zero.
    #[inline(always)]
    fn word(&self, index: u32) -> u32 {
        let at = index.wrapping_sub(self.base) as usize;
        self.src
            .get(at..at + 4)
            .and_then(|bytes| bytes.try_into().ok())
            .map_or(0, u32::from_le_bytes)
    }

    /// `ZSTD_count` of the prefix positions `a` and `b < a`, up to the block
    /// end.
    #[inline(always)]
    fn count<S: Simd>(&self, simd: S, a: u32, b: u32) -> usize {
        self.count_to(simd, a, b, self.iend)
    }

    /// `ZSTD_count` of the prefix positions `a` and `b`, up to `limit`.
    #[inline(always)]
    fn count_to<S: Simd>(&self, simd: S, a: u32, b: u32, limit: u32) -> usize {
        if a >= limit {
            return 0;
        }
        count::count(
            simd,
            self.src,
            (a - self.base) as usize,
            (b - self.base) as usize,
            (limit - self.base) as usize,
        )
    }

    /// `ZSTD_hashPtr` of prefix index `index` over `bits` bits for a
    /// minimum match of `MLS` (4, 5 or 6).
    ///
    /// The bytes hashed are copied into a fixed-size word first — one bounds
    /// check, and no index kept around for a panic message — and hashed by
    /// [`hash_ptr`] itself. A word past the prefix, which the reference's
    /// index invariants rule out, hashes as zeros.
    #[inline(always)]
    fn hash<const MLS: u32>(&self, index: u32, bits: u32) -> usize {
        let at = index.wrapping_sub(self.base) as usize;
        if MLS == 4 {
            let word: [u8; 4] = self
                .src
                .get(at..at + 4)
                .and_then(|bytes| bytes.try_into().ok())
                .unwrap_or_default();
            hash_ptr(&Mem::new(&word, 0), 0, bits, MLS)
        } else {
            let word: [u8; 8] = self
                .src
                .get(at..at + 8)
                .and_then(|bytes| bytes.try_into().ok())
                .unwrap_or_default();
            hash_ptr(&Mem::new(&word, 0), 0, bits, MLS)
        }
    }

    /// One step of the binary tree: the common length of the runs at
    /// window indices `a` and `b`, stopping before `limit` for `a`, and
    /// whether `b`'s run sorts below `a`'s — the byte after the common run
    /// is smaller — when that byte is before `limit` (`false` otherwise).
    ///
    /// Without an external dictionary (`!EXT`) both runs are in the prefix.
    /// A difference within the first word then comes from one load per
    /// side, and so do the two bytes that order the runs.
    #[inline(always)]
    fn tree_step<S: Simd, const EXT: bool>(
        &self,
        simd: S,
        a: u32,
        b: u32,
        limit: u32,
    ) -> (usize, bool) {
        if EXT {
            let n = self.count_virtual(simd, a, b, limit);
            let at = n as u32;
            let lower = a + at < limit && self.window.byte_at(b + at) < self.window.byte_at(a + at);
            return (n, lower);
        }
        let (a_off, b_off) = ((a - self.base) as usize, (b - self.base) as usize);
        let end = (limit - self.base) as usize;
        let mut n = 0;
        if a_off + 8 <= end
            && let (Some(x), Some(y)) = (
                self.src.get(a_off..a_off + 8),
                self.src.get(b_off..b_off + 8),
            )
        {
            let x = u64::from_le_bytes(x.try_into().unwrap_or_default());
            let y = u64::from_le_bytes(y.try_into().unwrap_or_default());
            let difference = x ^ y;
            if difference != 0 {
                let shift = difference.trailing_zeros() & !7;
                return (
                    (shift >> 3) as usize,
                    ((y >> shift) as u8) < ((x >> shift) as u8),
                );
            }
            n = 8;
        }
        n += self.count_to(simd, a + n as u32, b + n as u32, limit);
        let lower = a_off + n < end && self.src[b_off + n] < self.src[a_off + n];
        (n, lower)
    }

    /// [`mem::Window::count_virtual`] with the prefix-only case, the common
    /// one, counted by [`count`].
    #[inline(always)]
    fn count_virtual<S: Simd>(&self, simd: S, a: u32, b: u32, limit: u32) -> usize {
        if a >= self.base && b >= self.base {
            self.count_to(simd, a, b, limit)
        } else {
            self.window.count_virtual(a, b, limit)
        }
    }

    /// Length of the match of `ip` with `m` in the external dictionary,
    /// when its first four bytes match; `0` otherwise.
    #[inline(never)]
    fn ext_candidate_length(&self, ip: u32, m: u32) -> usize {
        let dict_limit = self.dict_limit();
        if self.window.ext.read32(m) == self.window.prefix.read32(ip) {
            return self
                .window
                .count_2segments(ip + 4, m + 4, self.iend, dict_limit, dict_limit)
                + 4;
        }
        0
    }
}

/// The searched position of one hash chain or row search, cut once: its
/// bytes up to the block end, and the word a candidate must share with it
/// to beat the best length so far — the reference's
/// `MEM_read32(match + ml - 3) == MEM_read32(ip + ml - 3)` test.
struct Probe<'a> {
    /// The prefix's bytes.
    src: &'a [u8],
    /// Index of `src[0]`.
    base: u32,
    /// The bytes from the searched position to the block end, which may
    /// come before the prefix's end.
    current: &'a [u8],
    /// `current[ml - 3..ml + 1]` for the best length `ml`.
    word: u32,
}

impl<'a> Probe<'a> {
    /// The probe of `ip`, aimed at the initial best length of 3.
    #[inline(always)]
    fn new(ctx: &Ctx<'a>, ip: u32) -> Self {
        // The block may end before the prefix does (long distance matching
        // runs the finder on parts of a block), and no match runs past it.
        let current = ctx
            .src
            .get((ip - ctx.base) as usize..(ctx.iend - ctx.base) as usize)
            .unwrap_or_default();
        let mut probe = Self {
            src: ctx.src,
            base: ctx.base,
            current,
            word: 0,
        };
        probe.aim(3);
        probe
    }

    /// Little-endian `u32` at `at` of `bytes`, `0` past their end.
    #[inline(always)]
    fn word(bytes: &[u8], at: usize) -> u32 {
        let at = at & u32::MAX as usize;
        bytes
            .get(at..at + 4)
            .and_then(|word| word.try_into().ok())
            .map_or(0, u32::from_le_bytes)
    }

    /// Sets the best length to beat to `ml`, which is below the window's
    /// length.
    #[inline(always)]
    fn aim(&mut self, ml: usize) {
        self.word = Self::word(self.current, ml - 3);
    }

    /// Length of the match with prefix index `m < ip` when it shares the
    /// aimed word with the searched position, `0` otherwise.
    #[inline(always)]
    fn length<S: Simd>(&self, simd: S, m: u32, ml: usize) -> usize {
        let at = (m - self.base) as usize;
        // Window positions are 32-bit; the mask states it, so the compiler
        // sees that `probe + 4` cannot overflow and checks only the end.
        let probe = (at + (ml - 3)) & u32::MAX as usize;
        let Some(word) = self.src.get(probe..probe + 4) else {
            return 0;
        };
        if u32::from_le_bytes(word.try_into().unwrap_or_default()) != self.word {
            return 0;
        }
        count::common_run(simd, self.current, self.src.get(at..).unwrap_or_default())
    }

    /// The candidate test of the hash chain and row searches for window
    /// index `m`: its match length when it may beat `ml`, `0` otherwise.
    /// `EXT` states whether `m` may lie in the external dictionary.
    #[inline(always)]
    fn candidate<S: Simd, const EXT: bool>(
        &self,
        simd: S,
        ctx: &Ctx<'_>,
        ip: u32,
        m: u32,
        ml: usize,
    ) -> usize {
        if !EXT || m >= self.base {
            self.length(simd, m, ml)
        } else {
            ctx.ext_candidate_length(ip, m)
        }
    }
}

/// The best of `candidates` for `ip` (`ZSTD_RowFindBestMatch`'s final
/// loop), its offset in `offset`; `EXT` as for [`Probe::candidate`].
#[inline(always)]
fn best_candidate<S: Simd, const EXT: bool>(
    simd: S,
    ctx: &Ctx<'_>,
    ip: u32,
    candidates: &[u32],
    offset: &mut u32,
) -> usize {
    let mut ml: usize = 4 - 1;
    let mut probe = Probe::new(ctx, ip);
    for &match_index in candidates {
        let current_ml = probe.candidate::<S, EXT>(simd, ctx, ip, match_index, ml);
        if current_ml > ml {
            ml = current_ml;
            *offset = offset_to_offbase(ip - match_index);
            if ip as usize + current_ml == ctx.iend as usize {
                break;
            }
            probe.aim(ml);
        }
    }
    ml
}

/// One walk down a hash chain (`ZSTD_HcFindBestMatch`'s loop).
struct ChainWalk<'c> {
    chain: &'c [u32],
    chain_mask: u32,
    /// The first candidate, from the hash table.
    first: u32,
    low_limit: u32,
    /// The walk stops after this index: older ones are overwritten.
    min_chain: u32,
    nb_attempts: u32,
}

/// The longest match for `ip` along `walk`, its offset in `offset`; `EXT`
/// as for [`Probe::candidate`].
#[inline(always)]
fn walk_chain<S: Simd, const EXT: bool>(
    simd: S,
    ctx: &Ctx<'_>,
    walk: &ChainWalk<'_>,
    ip: u32,
    offset: &mut u32,
) -> usize {
    let ChainWalk {
        chain,
        chain_mask,
        first: mut match_index,
        low_limit,
        min_chain,
        mut nb_attempts,
    } = *walk;
    let mut ml: usize = 4 - 1;
    let mut probe = Probe::new(ctx, ip);
    while match_index >= low_limit && nb_attempts > 0 {
        let current_ml = probe.candidate::<S, EXT>(simd, ctx, ip, match_index, ml);
        if current_ml > ml {
            ml = current_ml;
            *offset = offset_to_offbase(ip - match_index);
            if ip as usize + current_ml == ctx.iend as usize {
                break;
            }
            probe.aim(ml);
        }
        if match_index <= min_chain {
            break;
        }
        match_index = chain[(match_index & chain_mask) as usize];
        nb_attempts -= 1;
    }
    ml
}

/// A match search (`searchMethod_e`), specialized by the parser.
trait Search {
    /// Whether the search keeps the row finder's hash cache.
    const ROW: bool;

    /// `ZSTD_searchMax`: the longest match at `ip`, its offset in `offset`.
    fn find<S: Simd>(
        simd: S,
        ms: &mut MatchState,
        ctx: &Ctx<'_>,
        ip: u32,
        offset: &mut u32,
    ) -> usize;

    /// `ZSTD_row_fillHashCache`; nothing for searches without the cache, so
    /// the parser calls it unconditionally.
    fn fill_cache(_ms: &mut MatchState, _ctx: &Ctx<'_>, _idx: u32, _i_limit: i64) {}
}

/// Hash chains (`search_hashChain`).
struct HashChain;
/// The DUBT binary tree (`search_binaryTree`).
struct BinaryTree;
/// The row-based tag matcher (`search_rowHash`).
struct RowHash;

impl Search for HashChain {
    const ROW: bool = false;

    #[inline(always)]
    fn find<S: Simd>(
        simd: S,
        ms: &mut MatchState,
        ctx: &Ctx<'_>,
        ip: u32,
        offset: &mut u32,
    ) -> usize {
        match ctx.mls {
            5 => hc_find_best_match::<S, 5>(simd, ms, ctx, ip, offset),
            6 => hc_find_best_match::<S, 6>(simd, ms, ctx, ip, offset),
            _ => hc_find_best_match::<S, 4>(simd, ms, ctx, ip, offset),
        }
    }
}

impl Search for BinaryTree {
    const ROW: bool = false;

    #[inline(always)]
    fn find<S: Simd>(
        simd: S,
        ms: &mut MatchState,
        ctx: &Ctx<'_>,
        ip: u32,
        offset: &mut u32,
    ) -> usize {
        match (ctx.mls, ctx.ext) {
            (5, false) => bt_find_best_match::<S, 5, false>(simd, ms, ctx, ip, offset),
            (6, false) => bt_find_best_match::<S, 6, false>(simd, ms, ctx, ip, offset),
            (_, false) => bt_find_best_match::<S, 4, false>(simd, ms, ctx, ip, offset),
            (5, true) => bt_find_best_match::<S, 5, true>(simd, ms, ctx, ip, offset),
            (6, true) => bt_find_best_match::<S, 6, true>(simd, ms, ctx, ip, offset),
            (_, true) => bt_find_best_match::<S, 4, true>(simd, ms, ctx, ip, offset),
        }
    }
}

impl Search for RowHash {
    const ROW: bool = true;

    #[inline(always)]
    fn find<S: Simd>(
        simd: S,
        ms: &mut MatchState,
        ctx: &Ctx<'_>,
        ip: u32,
        offset: &mut u32,
    ) -> usize {
        match (ctx.mls, ctx.row_log) {
            (5, 4) => row_find_best_match::<S, 5, 16>(simd, ms, ctx, ip, offset),
            (5, 5) => row_find_best_match::<S, 5, 32>(simd, ms, ctx, ip, offset),
            (5, _) => row_find_best_match::<S, 5, 64>(simd, ms, ctx, ip, offset),
            (6, 4) => row_find_best_match::<S, 6, 16>(simd, ms, ctx, ip, offset),
            (6, 5) => row_find_best_match::<S, 6, 32>(simd, ms, ctx, ip, offset),
            (6, _) => row_find_best_match::<S, 6, 64>(simd, ms, ctx, ip, offset),
            (_, 4) => row_find_best_match::<S, 4, 16>(simd, ms, ctx, ip, offset),
            (_, 5) => row_find_best_match::<S, 4, 32>(simd, ms, ctx, ip, offset),
            (_, _) => row_find_best_match::<S, 4, 64>(simd, ms, ctx, ip, offset),
        }
    }

    fn fill_cache(ms: &mut MatchState, ctx: &Ctx<'_>, idx: u32, i_limit: i64) {
        let bits = ms.row_hash_log + ROW_HASH_TAG_BITS;
        let cache = &mut ms.hash_cache;
        let tables = (ms.tag_table.as_slice(), ms.hash_table.as_slice());
        match ctx.mls {
            5 => row_fill_hash_cache::<5>(cache, tables, ctx, bits, idx, i_limit),
            6 => row_fill_hash_cache::<6>(cache, tables, ctx, bits, idx, i_limit),
            _ => row_fill_hash_cache::<4>(cache, tables, ctx, bits, idx, i_limit),
        }
    }
}

/// `lowLimit` of the hash chain and row searches at `curr`.
const fn search_low_limit(ms: &MatchState, curr: u32) -> u32 {
    let max_distance = 1u32 << ms.cparams.window_log;
    let lowest_valid = ms.window.low_limit;
    let within_max_distance = if curr.wrapping_sub(lowest_valid) > max_distance {
        curr - max_distance
    } else {
        lowest_valid
    };
    if ms.loaded_dict_end != 0 {
        lowest_valid
    } else {
        within_max_distance
    }
}

// ---------------------------------------------------------------------------
// Hash chain.

/// `ZSTD_insertAndFindFirstIndex_internal`.
#[inline(always)]
fn insert_and_find_first_index<const MLS: u32>(ms: &mut MatchState, ctx: &Ctx<'_>, ip: u32) -> u32 {
    let hash_log = ms.cparams.hash_log;
    let chain_mask = (1u32 << ms.cparams.chain_log) - 1;
    let lazy_skipping = ms.lazy_skipping;
    let mut idx = ms.next_to_update;
    // Local slices keep the tables' pointers and lengths in registers: the
    // stores below could otherwise alias the vectors that own them.
    let hash_table = ms.hash_table.as_mut_slice();
    let chain_table = ms.chain_table.as_mut_slice();
    while idx < ip {
        let h = ctx.hash::<MLS>(idx, hash_log);
        chain_table[(idx & chain_mask) as usize] = hash_table[h];
        hash_table[h] = idx;
        idx += 1;
        if lazy_skipping {
            break;
        }
    }
    let first = hash_table[ctx.hash::<MLS>(ip, hash_log)];
    ms.next_to_update = ip;
    first
}

/// `ZSTD_HcFindBestMatch`.
#[inline(always)]
fn hc_find_best_match<S: Simd, const MLS: u32>(
    simd: S,
    ms: &mut MatchState,
    ctx: &Ctx<'_>,
    ip: u32,
    offset: &mut u32,
) -> usize {
    let cparams = ms.cparams;
    let chain_size = 1u32 << cparams.chain_log;
    let chain_mask = chain_size - 1;
    let curr = ip;
    let low_limit = search_low_limit(ms, curr);
    let min_chain = curr.saturating_sub(chain_size);
    let nb_attempts = 1u32 << cparams.search_log;

    let first = insert_and_find_first_index::<MLS>(ms, ctx, ip);
    let walk = ChainWalk {
        chain: &ms.chain_table,
        chain_mask,
        first,
        low_limit,
        min_chain,
        nb_attempts,
    };
    if ctx.ext {
        walk_chain::<S, true>(simd, ctx, &walk, ip, offset)
    } else {
        walk_chain::<S, false>(simd, ctx, &walk, ip, offset)
    }
}

// ---------------------------------------------------------------------------
// Binary tree (DUBT).

/// `ZSTD_updateDUBT`.
#[inline(always)]
fn update_dubt<const MLS: u32>(ms: &mut MatchState, ctx: &Ctx<'_>, ip: u32) {
    let hash_log = ms.cparams.hash_log;
    let bt_mask = (1u32 << (ms.cparams.chain_log - 1)) - 1;
    let mut idx = ms.next_to_update;
    while idx < ip {
        let h = ctx.hash::<MLS>(idx, hash_log);
        let match_index = ms.hash_table[h];
        let slot = 2 * (idx & bt_mask) as usize;
        ms.hash_table[h] = idx;
        ms.chain_table[slot] = match_index;
        ms.chain_table[slot + 1] = DUBT_UNSORTED_MARK;
        idx += 1;
    }
    ms.next_to_update = ip;
}

/// A tree link to write: a slot of the chain table, or nowhere.
#[derive(Copy, Clone)]
enum Link {
    Slot(usize),
    Dummy,
}

#[inline(always)]
fn set_link(bt: &mut [u32], link: Link, value: u32) {
    if let Link::Slot(slot) = link {
        bt[slot] = value;
    }
}

/// `ZSTD_insertDUBT1`: sorts one inserted but unsorted position.
#[inline(always)]
fn insert_dubt1<S: Simd, const EXT: bool>(
    simd: S,
    ms: &mut MatchState,
    ctx: &Ctx<'_>,
    curr: u32,
    mut nb_compares: u32,
    bt_low: u32,
) {
    let cparams = ms.cparams;
    let bt_mask = (1u32 << (cparams.chain_log - 1)) - 1;
    let dict_limit = ctx.dict_limit();
    let iend = if curr >= dict_limit {
        ctx.iend
    } else {
        dict_limit
    };
    let mut common_smaller = 0usize;
    let mut common_larger = 0usize;
    let mut smaller = Link::Slot(2 * (curr & bt_mask) as usize);
    let mut larger = Link::Slot(2 * (curr & bt_mask) as usize + 1);
    let window_valid = ms.window.low_limit;
    let max_distance = 1u32 << cparams.window_log;
    let window_low = if curr.wrapping_sub(window_valid) > max_distance {
        curr - max_distance
    } else {
        window_valid
    };
    let bt = &mut ms.chain_table;
    let mut match_index = bt[2 * (curr & bt_mask) as usize];

    while nb_compares > 0 && match_index > window_low {
        let next = 2 * (match_index & bt_mask) as usize;
        let mut match_length = common_smaller.min(common_larger);
        let (common, lower) = ctx.tree_step::<S, EXT>(
            simd,
            curr + match_length as u32,
            match_index + match_length as u32,
            iend,
        );
        match_length += common;
        if (curr as usize + match_length) == iend as usize {
            break;
        }
        if lower {
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
#[inline(always)]
fn dubt_find_best_match<S: Simd, const MLS: u32, const EXT: bool>(
    simd: S,
    ms: &mut MatchState,
    ctx: &Ctx<'_>,
    ip: u32,
    off_base: &mut u32,
) -> usize {
    let cparams = ms.cparams;
    let hash_log = cparams.hash_log;
    let h = ctx.hash::<MLS>(ip, hash_log);
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
        insert_dubt1::<S, EXT>(simd, ms, ctx, match_index, nb_candidates, unsort_limit);
        match_index = next_candidate_idx;
        nb_candidates += 1;
    }

    // Find the longest match.
    let mut common_smaller = 0usize;
    let mut common_larger = 0usize;
    let mut match_end_idx = curr + 8 + 1;
    let mut best_length = 0usize;

    match_index = ms.hash_table[h];
    ms.hash_table[h] = curr;
    let mut smaller = Link::Slot(2 * (curr & bt_mask) as usize);
    let mut larger = Link::Slot(2 * (curr & bt_mask) as usize + 1);
    let bt = &mut ms.chain_table;

    while nb_compares > 0 && match_index > window_low {
        let next = 2 * (match_index & bt_mask) as usize;
        let mut match_length = common_smaller.min(common_larger);
        let (common, lower) = ctx.tree_step::<S, EXT>(
            simd,
            ip + match_length as u32,
            match_index + match_length as u32,
            ctx.iend,
        );
        match_length += common;
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
        if lower {
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
#[fearless_simd_macros::simd]
fn bt_find_best_match<S: Simd, const MLS: u32, const EXT: bool>(
    simd: S,
    ms: &mut MatchState,
    ctx: &Ctx<'_>,
    ip: u32,
    off_base: &mut u32,
) -> usize {
    if ip < ms.next_to_update {
        return 0;
    }
    update_dubt::<MLS>(ms, ctx, ip);
    dubt_find_best_match::<S, MLS, EXT>(simd, ms, ctx, ip, off_base)
}

// ---------------------------------------------------------------------------
// Row-based match finder.

/// The row finder's tables cut into rows of `ROW` slots: one tag byte and
/// one position per slot, with slot 0 of a tag row holding its head.
///
/// A row number is checked once against the table; every slot index inside
/// a row is masked below `ROW`, so it needs no check of its own.
struct Rows<'t, const ROW: usize> {
    tags: &'t mut [[u8; ROW]],
    slots: &'t mut [[u32; ROW]],
}

impl<'t, const ROW: usize> Rows<'t, ROW> {
    /// Both tables cut to their common number of rows, so one check of a
    /// row number covers both.
    fn new(tag_table: &'t mut [u8], hash_table: &'t mut [u32]) -> Self {
        let tags = tag_table.as_chunks_mut::<ROW>().0;
        let slots = hash_table.as_chunks_mut::<ROW>().0;
        let rows = tags.len().min(slots.len());
        Self {
            tags: &mut tags[..rows],
            slots: &mut slots[..rows],
        }
    }

    /// [`row_prefetch`] of the rows of `hash`.
    #[inline(always)]
    fn prefetch(&self, hash: u32) {
        row_prefetch(
            self.tags.as_flattened(),
            self.slots.as_flattened(),
            hash,
            ROW.trailing_zeros(),
        );
    }

    /// Inserts position `index`, whose salted hash is `hash`
    /// (`ZSTD_row_update_internalImpl`'s loop body).
    #[inline(always)]
    fn insert(&mut self, hash: u32, index: u32) {
        let row = (hash >> ROW_HASH_TAG_BITS) as usize;
        let tag_row = &mut self.tags[row];
        let pos = row_next_index(tag_row);
        tag_row[pos] = hash as u8;
        self.slots[row][pos] = index;
    }
}

/// `PREFETCH_L1`: hints that `table[index]` will be read soon. The pointer
/// is only handed to the prefetch instruction, never dereferenced, so a hint
/// past the table is harmless; targets without one ignore it.
#[inline(always)]
fn prefetch_l1<T>(table: &[T], index: usize) {
    branches::prefetch_read_data::<T, 0>(table.as_ptr().wrapping_add(index));
}

/// `ZSTD_row_prefetch`: the hash and tag rows of `hash`, the first 64 bytes
/// of each, plus the hash row's next 64 bytes from 32-slot rows and the tag
/// row's next 32 bytes for 64-slot rows.
#[inline(always)]
fn row_prefetch(tag_table: &[u8], hash_table: &[u32], hash: u32, row_log: u32) {
    let rel_row = ((hash >> ROW_HASH_TAG_BITS) << row_log) as usize;
    prefetch_l1(hash_table, rel_row);
    if row_log >= 5 {
        prefetch_l1(hash_table, rel_row + 16);
    }
    prefetch_l1(tag_table, rel_row);
    if row_log == 6 {
        prefetch_l1(tag_table, rel_row + 32);
    }
}

/// `ZSTD_row_nextIndex`: the next slot of a row, which also becomes its head.
#[inline(always)]
fn row_next_index<const ROW: usize>(tag_row: &mut [u8; ROW]) -> usize {
    let row_mask = ROW as u32 - 1;
    let mut next = u32::from(tag_row[0]).wrapping_sub(1) & row_mask;
    if next == 0 {
        next += row_mask;
    }
    tag_row[0] = next as u8;
    next as usize & (ROW - 1)
}

/// Hash of the row match finder over `bits` bits, salted as the reference's
/// is.
#[inline(always)]
fn row_hash<const MLS: u32>(ctx: &Ctx<'_>, index: u32, bits: u32) -> u32 {
    ctx.hash::<MLS>(index, bits) as u32
}

/// `ZSTD_row_fillHashCache`, prefetching the rows of every hash it caches.
fn row_fill_hash_cache<const MLS: u32>(
    cache: &mut [u32; ROW_HASH_CACHE_SIZE],
    tables: (&[u8], &[u32]),
    ctx: &Ctx<'_>,
    bits: u32,
    mut idx: u32,
    i_limit: i64,
) {
    let max_elems = if i64::from(idx) > i_limit {
        0
    } else {
        (i_limit - i64::from(idx) + 1) as u32
    };
    let lim = idx + (ROW_HASH_CACHE_SIZE as u32).min(max_elems);
    while idx < lim {
        let hash = row_hash::<MLS>(ctx, idx, bits);
        row_prefetch(tables.0, tables.1, hash, ctx.row_log);
        cache[(idx & ROW_HASH_CACHE_MASK) as usize] = hash;
        idx += 1;
    }
}

/// `ZSTD_row_nextCachedHash`.
#[inline(always)]
fn row_next_cached_hash<const MLS: u32>(
    cache: &mut [u32; ROW_HASH_CACHE_SIZE],
    ctx: &Ctx<'_>,
    bits: u32,
    idx: u32,
) -> u32 {
    let new_hash = row_hash::<MLS>(ctx, idx + ROW_HASH_CACHE_SIZE as u32, bits);
    core::mem::replace(&mut cache[(idx & ROW_HASH_CACHE_MASK) as usize], new_hash)
}

/// `ZSTD_row_update_internalImpl` with the hash cache.
#[inline(always)]
fn row_update_impl<const MLS: u32, const ROW: usize>(
    rows: &mut Rows<'_, ROW>,
    cache: &mut [u32; ROW_HASH_CACHE_SIZE],
    ctx: &Ctx<'_>,
    bits: u32,
    start: u32,
    end: u32,
) {
    for idx in start..end {
        let hash = row_next_cached_hash::<MLS>(cache, ctx, bits, idx);
        // `ZSTD_row_nextCachedHash` prefetches the rows of the hash it
        // caches, 8 positions ahead.
        rows.prefetch(cache[(idx & ROW_HASH_CACHE_MASK) as usize]);
        rows.insert(hash, idx);
    }
}

/// `ZSTD_row_update_internal` with the hash cache.
#[inline(always)]
fn row_update<const MLS: u32, const ROW: usize>(
    rows: &mut Rows<'_, ROW>,
    cache: &mut [u32; ROW_HASH_CACHE_SIZE],
    next_to_update: &mut u32,
    ctx: &Ctx<'_>,
    bits: u32,
    ip: u32,
) {
    const SKIP_THRESHOLD: u32 = 384;
    const MAX_MATCH_START_POSITIONS_TO_UPDATE: u32 = 96;
    const MAX_MATCH_END_POSITIONS_TO_UPDATE: u32 = 32;
    let mut idx = *next_to_update;
    let target = ip;
    if target - idx > SKIP_THRESHOLD {
        let bound = idx + MAX_MATCH_START_POSITIONS_TO_UPDATE;
        row_update_impl::<MLS, ROW>(rows, cache, ctx, bits, idx, bound);
        idx = target - MAX_MATCH_END_POSITIONS_TO_UPDATE;
        let tables = (rows.tags.as_flattened(), rows.slots.as_flattened());
        row_fill_hash_cache::<MLS>(cache, tables, ctx, bits, idx, i64::from(ip) + 1);
    }
    row_update_impl::<MLS, ROW>(rows, cache, ctx, bits, idx, target);
    *next_to_update = target;
}

/// `ZSTD_RowFindBestMatch`.
#[inline(always)]
fn row_find_best_match<S: Simd, const MLS: u32, const ROW: usize>(
    simd: S,
    ms: &mut MatchState,
    ctx: &Ctx<'_>,
    ip: u32,
    offset: &mut u32,
) -> usize {
    let curr = ip;
    let low_limit = search_low_limit(ms, curr);
    let row_log = ROW.trailing_zeros();
    let capped_search_log = ms.cparams.search_log.min(row_log);
    let mut nb_attempts = 1u32 << capped_search_log;
    let bits = ms.row_hash_log + ROW_HASH_TAG_BITS;
    let lazy_skipping = ms.lazy_skipping;
    let MatchState {
        tag_table,
        hash_table,
        hash_cache,
        next_to_update,
        ..
    } = ms;
    let mut rows = Rows::<ROW>::new(tag_table, hash_table);

    let hash = if !lazy_skipping {
        row_update::<MLS, ROW>(&mut rows, hash_cache, next_to_update, ctx, bits, ip);
        let hash = row_next_cached_hash::<MLS>(hash_cache, ctx, bits, curr);
        rows.prefetch(hash_cache[(curr & ROW_HASH_CACHE_MASK) as usize]);
        hash
    } else {
        *next_to_update = curr;
        row_hash::<MLS>(ctx, ip, bits)
    };

    let row = (hash >> ROW_HASH_TAG_BITS) as usize;
    let tag = hash as u8;
    let tag_row = &mut rows.tags[row];
    let slot_row = &mut rows.slots[row];
    let head = u32::from(tag_row[0]) & (ROW as u32 - 1);
    let mut matches = row::match_mask(simd, tag_row, tag, head);
    // Candidates are collected before any is measured, as the reference
    // does: their loads then overlap instead of waiting on each comparison
    // (one fused pass measured 15-20% slower). At most `ROW - 1` are
    // collected — slot 0 is the head, never one — which the masked index
    // states, so the store needs no check.
    let mut match_buffer = [0u32; ROW];
    let mut num_matches = 0usize;
    while matches != 0 && nb_attempts > 0 {
        let match_pos = (head as usize + matches.trailing_zeros() as usize) & (ROW - 1);
        matches &= matches - 1;
        if match_pos == 0 {
            continue;
        }
        let match_index = slot_row[match_pos];
        if match_index < low_limit {
            break;
        }
        match_buffer[num_matches & (ROW - 1)] = match_index;
        num_matches += 1;
        nb_attempts -= 1;
    }
    let num_matches = num_matches.min(ROW);

    // Insert the current position too.
    {
        let pos = row_next_index(tag_row);
        tag_row[pos] = tag;
        slot_row[pos] = *next_to_update;
        *next_to_update += 1;
    }

    let candidates = &match_buffer[..num_matches];
    if ctx.ext {
        best_candidate::<S, true>(simd, ctx, ip, candidates, offset)
    } else {
        best_candidate::<S, false>(simd, ctx, ip, candidates, offset)
    }
}

// ---------------------------------------------------------------------------
// Parsers.

/// `(int)(a*m - highbit32(offBase) + k)` as the reference computes it.
#[inline(always)]
fn gain(length: usize, multiplier: i64, off_base: u32, bonus: i64) -> i32 {
    (length as i64 * multiplier - i64::from(highbit32(off_base)) + bonus) as i32
}

/// `ZSTD_compressBlock_lazy_generic` for the no-dictionary window.
#[inline(always)]
fn lazy_generic<S: Simd, M: Search, const DEPTH: u32>(
    simd: S,
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    let prefix = block.window.prefix;
    let istart = i64::from(block.start);
    let iend = i64::from(block.end);
    let ilimit = if M::ROW {
        iend - 8 - ROW_HASH_CACHE_SIZE as i64
    } else {
        iend - 8
    };
    let prefix_lowest = i64::from(ms.window.dict_limit);
    let ctx = Ctx::new(
        block,
        ms.cparams.min_match.clamp(4, 6),
        ms.cparams.search_log.clamp(4, 6),
        false,
    );
    let read32 = |p: i64| ctx.word(p as u32);
    let count = |a: i64, b: i64| ctx.count(simd, a as u32, b as u32);
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
    let next = ms.next_to_update;
    M::fill_cache(ms, &ctx, next, ilimit);

    while ip < ilimit {
        let mut match_length = 0usize;
        let mut off_base = repcode_to_offbase(1);
        let mut start = ip + 1;
        let mut store_now = false;

        if offset_1 > 0 && read32(ip + 1 - i64::from(offset_1)) == read32(ip + 1) {
            match_length = count(ip + 1 + 4, ip + 1 + 4 - i64::from(offset_1)) + 4;
            if DEPTH == 0 {
                store_now = true;
            }
        }

        if !store_now {
            let mut found = NO_OFFBASE;
            let ml2 = M::find(simd, ms, &ctx, ip as u32, &mut found);
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

            if DEPTH >= 1 {
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
                        let ml2 = M::find(simd, ms, &ctx, ip as u32, &mut candidate);
                        let gain2 = gain(ml2, 4, candidate, 0);
                        let gain1 = gain(match_length, 4, off_base, 4);
                        if ml2 >= 4 && gain2 > gain1 {
                            match_length = ml2;
                            off_base = candidate;
                            start = ip;
                            continue;
                        }
                    }
                    if DEPTH == 2 && ip < ilimit {
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
                        let ml2 = M::find(simd, ms, &ctx, ip as u32, &mut candidate);
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
            let next = ms.next_to_update;
            M::fill_cache(ms, &ctx, next, ilimit);
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
#[inline(always)]
fn lazy_ext_generic<S: Simd, M: Search, const DEPTH: u32>(
    simd: S,
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    let window = block.window;
    let prefix = window.prefix;
    let istart = i64::from(block.start);
    let iend = i64::from(block.end);
    let ilimit = if M::ROW {
        iend - 8 - ROW_HASH_CACHE_SIZE as i64
    } else {
        iend - 8
    };
    let dict_limit = ms.window.dict_limit;
    let prefix_start = i64::from(dict_limit);
    let dict_start = ms.window.low_limit;
    let window_log = ms.cparams.window_log;
    let ctx = Ctx::new(
        block,
        ms.cparams.min_match.clamp(4, 6),
        ms.cparams.search_log.clamp(4, 6),
        true,
    );
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
            Some(ctx.count_virtual(simd, curr + 4, rep_index + 4, block.end) + 4)
        } else {
            None
        }
    };

    ms.lazy_skipping = false;
    let mut ip = istart;
    let mut anchor = istart;
    ip += i64::from(ip == prefix_start);
    let next = ms.next_to_update;
    M::fill_cache(ms, &ctx, next, ilimit);

    while ip < ilimit {
        let mut match_length = 0usize;
        let mut off_base = repcode_to_offbase(1);
        let mut start = ip + 1;
        let mut curr = ip as u32;
        let mut store_now = false;

        if let Some(length) = rep_ok(ms, curr + 1, offset_1) {
            match_length = length;
            if DEPTH == 0 {
                store_now = true;
            }
        }

        if !store_now {
            let mut candidate = NO_OFFBASE;
            let ml2 = M::find(simd, ms, &ctx, ip as u32, &mut candidate);
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
            if DEPTH >= 1 {
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
                        let ml2 = M::find(simd, ms, &ctx, ip as u32, &mut candidate);
                        let gain2 = gain(ml2, 4, candidate, 0);
                        let gain1 = gain(match_length, 4, off_base, 4);
                        if ml2 >= 4 && gain2 > gain1 {
                            match_length = ml2;
                            off_base = candidate;
                            start = ip;
                            continue;
                        }
                    }
                    if DEPTH == 2 && ip < ilimit {
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
                        let ml2 = M::find(simd, ms, &ctx, ip as u32, &mut candidate);
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
            let next = ms.next_to_update;
            M::fill_cache(ms, &ctx, next, ilimit);
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

/// Defines the block compressors of one search and depth: each dispatches
/// the match state's backend once and runs the parser specialized for it.
macro_rules! block_compressors {
    ($($name:ident, $ext_name:ident => $search:ty, $depth:expr;)*) => {$(
        pub(crate) fn $name(
            ms: &mut MatchState,
            seq_store: &mut SeqStore,
            rep: &mut [u32; 3],
            block: &BlockInput<'_>,
        ) -> usize {
            let backend = ms.backend;
            fearless_simd::dispatch!(backend.0, simd => {
                lazy_generic::<_, $search, $depth>(simd, ms, seq_store, rep, block)
            })
        }

        pub(crate) fn $ext_name(
            ms: &mut MatchState,
            seq_store: &mut SeqStore,
            rep: &mut [u32; 3],
            block: &BlockInput<'_>,
        ) -> usize {
            let backend = ms.backend;
            fearless_simd::dispatch!(backend.0, simd => {
                lazy_ext_generic::<_, $search, $depth>(simd, ms, seq_store, rep, block)
            })
        }
    )*};
}

block_compressors! {
    greedy, greedy_ext => HashChain, 0;
    lazy, lazy_ext => HashChain, 1;
    lazy2, lazy2_ext => HashChain, 2;
    btlazy2, btlazy2_ext => BinaryTree, 2;
    greedy_row, greedy_row_ext => RowHash, 0;
    lazy_row, lazy_row_ext => RowHash, 1;
    lazy2_row, lazy2_row_ext => RowHash, 2;
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use super::super::advanced_tests;
    use super::super::cctx::{Engine, RequestedParams};
    use super::super::compress_once;
    use super::super::params::{ParamSwitch, Strategy};
    use super::super::stream::{EndOp, Stream};
    use crate::Backend;
    use zstd_ffi::corpus;
    use zstd_ffi::oracle::{self, Directive, Params};

    /// The reference's parameters for `requested`, row finder switch
    /// included.
    fn oracle_params(requested: &RequestedParams) -> Params {
        let row = match requested.use_row_match_finder {
            ParamSwitch::Auto => 0,
            ParamSwitch::Enable => 1,
            ParamSwitch::Disable => 2,
        };
        advanced_tests::oracle_params(requested).with(zstd_ffi::ZSTD_c_useRowMatchFinder, row)
    }

    /// Every search specialization: strategies with and without the row
    /// finder, minimum matches 4 to 6 and row sizes 16 to 64.
    fn requests() -> Vec<RequestedParams> {
        let mut all = Vec::new();
        for strategy in [
            Strategy::Greedy,
            Strategy::Lazy,
            Strategy::Lazy2,
            Strategy::BtLazy2,
        ] {
            for row in [ParamSwitch::Enable, ParamSwitch::Disable] {
                for (min_match, search_log) in [(4, 4), (5, 5), (6, 6), (7, 3)] {
                    let mut requested = RequestedParams::default();
                    requested.cparams.strategy = Some(strategy);
                    requested.cparams.min_match = min_match;
                    requested.cparams.search_log = search_log;
                    requested.use_row_match_finder = row;
                    all.push(requested);
                }
            }
        }
        all
    }

    #[test]
    fn every_backend_matches_the_reference_one_shot() {
        let inputs = [
            corpus::text(200_000, 41),
            corpus::binary(120_000, 42),
            corpus::mixed(90_000, 43),
            vec![b'a'; 70_000],
        ];
        for backend in Backend::available() {
            let mut engine = Engine::with_backend(backend);
            for level in 5..=15 {
                let requested = RequestedParams {
                    compression_level: level,
                    ..RequestedParams::default()
                };
                for (i, input) in inputs.iter().enumerate() {
                    let ours = compress_once(&mut engine, &requested, input, 1).expect("compress");
                    let theirs =
                        oracle::compress2(input, &oracle_params(&requested)).expect("oracle");
                    assert!(ours == theirs, "{backend} level {level} input {i}");
                }
            }
            for (r, requested) in requests().iter().enumerate() {
                for (i, input) in inputs.iter().enumerate() {
                    let ours = compress_once(&mut engine, requested, input, 1).expect("compress");
                    let theirs =
                        oracle::compress2(input, &oracle_params(requested)).expect("oracle");
                    assert!(ours == theirs, "{backend} request {r} input {i}");
                }
            }
        }
    }

    /// Long distance matching runs the finders on parts of a block whose end
    /// comes before the end of the prefix they see; no match may run past
    /// that end (a regression: the row finder's candidates once did).
    #[test]
    fn every_backend_matches_the_reference_with_long_distance_matching() {
        let input = corpus::text(300_000, 1);
        let mut requested = RequestedParams {
            compression_level: 10,
            ..RequestedParams::default()
        };
        requested.cparams.chain_log = 10;
        requested.cparams.hash_log = 20;
        requested.cparams.min_match = 5;
        requested.ldm.enable = ParamSwitch::Enable;
        requested.ldm.min_match_length = 23;
        for backend in Backend::available() {
            let mut engine = Engine::with_backend(backend);
            for strategy in [Strategy::Greedy, Strategy::Lazy2, Strategy::BtLazy2] {
                for row in [ParamSwitch::Enable, ParamSwitch::Disable] {
                    requested.cparams.strategy = Some(strategy);
                    requested.use_row_match_finder = row;
                    let ours = compress_once(&mut engine, &requested, &input, 1).expect("compress");
                    let theirs =
                        oracle::compress2(&input, &oracle_params(&requested)).expect("oracle");
                    assert!(ours == theirs, "{backend} {strategy:?} {row:?}");
                }
            }
        }
    }

    /// A reused engine continues its indices and keeps the previous
    /// frames' table entries, as the reference does; they lie below the new
    /// window and must never be followed, whatever finder wrote them.
    #[test]
    fn reused_engines_match_fresh_reference_frames() {
        let inputs = [
            corpus::text(90_000, 71),
            corpus::binary(60_000, 72),
            corpus::text(3_000, 73),
            corpus::mixed(120_000, 74),
        ];
        let mut sequence = requests();
        for level in [5, 9, 12, 13, 15, 3, 16, 19, 22, 13] {
            sequence.push(RequestedParams {
                compression_level: level,
                ..RequestedParams::default()
            });
        }
        for backend in Backend::available() {
            let mut engine = Engine::with_backend(backend);
            for (r, requested) in sequence.iter().enumerate() {
                let input = &inputs[r % inputs.len()];
                let ours = compress_once(&mut engine, requested, input, r as u64).expect("ours");
                let theirs = oracle::compress2(input, &oracle_params(requested)).expect("oracle");
                assert!(ours == theirs, "{backend} frame {r}");
            }
        }
    }

    fn stream(engine: &mut Engine, requested: &RequestedParams, chunks: &[&[u8]]) -> Vec<u8> {
        let mut state = Stream::new();
        let mut out = Vec::new();
        let mut buffer = vec![0u8; 1 << 17];
        for (n, &data) in chunks.iter().enumerate() {
            let op = if n + 1 == chunks.len() {
                EndOp::End
            } else {
                EndOp::Continue
            };
            let mut in_pos = 0;
            loop {
                let step = state
                    .compress(engine, requested, &mut buffer, &data[in_pos..], op)
                    .expect("stream");
                in_pos += step.consumed;
                out.extend_from_slice(&buffer[..step.produced]);
                if in_pos == data.len() && (op != EndOp::End || step.remaining == 0) {
                    break;
                }
            }
        }
        out
    }

    /// Small windows wrap the streaming buffer, so every finder runs its
    /// extDict variant.
    #[test]
    fn every_backend_matches_the_reference_on_ext_dict_windows() {
        let input = [corpus::text(60_000, 51), corpus::mixed(60_000, 52)].concat();
        let chunks: Vec<&[u8]> = input.chunks(7_001).collect();
        let steps: Vec<(&[u8], Directive)> = chunks
            .iter()
            .enumerate()
            .map(|(n, &data)| {
                let op = if n + 1 == chunks.len() {
                    Directive::End
                } else {
                    Directive::Continue
                };
                (data, op)
            })
            .collect();
        for backend in Backend::available() {
            let mut engine = Engine::with_backend(backend);
            for (r, mut requested) in requests().into_iter().enumerate() {
                requested.cparams.window_log = 12;
                let theirs =
                    oracle::compress_stream(&oracle_params(&requested), &steps).expect("oracle");
                let ours = stream(&mut engine, &requested, &chunks);
                assert!(ours == theirs, "{backend} request {r}");
            }
        }
    }
}
