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

use super::match_state::{
    BlockInput, DUBT_UNSORTED_MARK, MatchState, ROW_HASH_CACHE_SIZE, ROW_TABLE_SLACK,
};
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

    /// Whether the four bytes at prefix index `p` repeat `offset` bytes
    /// earlier (`MEM_read32(ip - offset) == MEM_read32(ip)`), for an offset
    /// above zero. One slice spanning both words is cut, so one check covers
    /// both loads; a span that leaves the prefix, which the parser's offset
    /// bounds rule out, does not match.
    #[inline(always)]
    fn repeats(&self, p: u32, offset: u32) -> bool {
        let at = p.wrapping_sub(self.base) as usize;
        let Some(from) = at.checked_sub(offset as usize) else {
            return false;
        };
        let Some(span) = self.src.get(from..at + 4) else {
            return false;
        };
        match (span.first_chunk::<4>(), span.last_chunk::<4>()) {
            (Some(earlier), Some(word)) => earlier == word,
            _ => false,
        }
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

    /// One step of the binary tree with an external dictionary: the common
    /// length of the runs at window indices `a` and `b`, stopping before
    /// `limit` for `a`, and whether `b`'s run sorts below `a`'s — the byte
    /// after the common run is smaller — when that byte is before `limit`
    /// (`false` otherwise). [`TreeRun::step`] takes the prefix-only case.
    #[inline(always)]
    fn ext_tree_step<S: Simd>(&self, simd: S, a: u32, b: u32, limit: u32) -> (usize, bool) {
        let n = self.count_virtual(simd, a, b, limit);
        let at = n as u32;
        let lower = a + at < limit && self.window.byte_at(b + at) < self.window.byte_at(a + at);
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

    /// The search's view of the match state for one block: the tables it
    /// reads and writes, and the constants it derives from the parameters.
    type State<'t>;

    /// The state of `ms` for one block of `ctx`.
    fn state<'t>(ms: &'t mut MatchState, ctx: &Ctx<'_>) -> Self::State<'t>;

    /// `ZSTD_searchMax`: the longest match at `ip`, its offset in `offset`.
    /// `lazy_skipping` is the parser's `ms->lazySkipping`.
    fn find<S: Simd>(
        simd: S,
        state: &mut Self::State<'_>,
        ctx: &Ctx<'_>,
        ip: u32,
        lazy_skipping: bool,
        offset: &mut u32,
    ) -> usize;

    /// `ZSTD_row_fillHashCache` from the next position to insert; nothing
    /// for searches without the cache, so the parser calls it
    /// unconditionally.
    fn fill_cache(_state: &mut Self::State<'_>, _ctx: &Ctx<'_>, _i_limit: i64) {}
}

/// Hash chains (`search_hashChain`).
struct HashChain;
/// The DUBT binary tree (`search_binaryTree`).
struct BinaryTree;
/// The row-based tag matcher (`search_rowHash`).
struct RowHash;

impl Search for HashChain {
    const ROW: bool = false;
    type State<'t> = &'t mut MatchState;

    fn state<'t>(ms: &'t mut MatchState, _ctx: &Ctx<'_>) -> Self::State<'t> {
        ms
    }

    #[inline(always)]
    fn find<S: Simd>(
        simd: S,
        ms: &mut &mut MatchState,
        ctx: &Ctx<'_>,
        ip: u32,
        lazy_skipping: bool,
        offset: &mut u32,
    ) -> usize {
        match ctx.mls {
            5 => hc_find_best_match::<S, 5>(simd, ms, ctx, ip, lazy_skipping, offset),
            6 => hc_find_best_match::<S, 6>(simd, ms, ctx, ip, lazy_skipping, offset),
            _ => hc_find_best_match::<S, 4>(simd, ms, ctx, ip, lazy_skipping, offset),
        }
    }
}

impl Search for BinaryTree {
    const ROW: bool = false;
    type State<'t> = &'t mut MatchState;

    fn state<'t>(ms: &'t mut MatchState, _ctx: &Ctx<'_>) -> Self::State<'t> {
        ms
    }

    #[inline(always)]
    fn find<S: Simd>(
        simd: S,
        ms: &mut &mut MatchState,
        ctx: &Ctx<'_>,
        ip: u32,
        _lazy_skipping: bool,
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
    type State<'t> = RowState<'t>;

    fn state<'t>(ms: &'t mut MatchState, ctx: &Ctx<'_>) -> Self::State<'t> {
        RowState::new(ms, ctx)
    }

    #[inline(always)]
    fn find<S: Simd>(
        simd: S,
        state: &mut RowState<'_>,
        ctx: &Ctx<'_>,
        ip: u32,
        lazy_skipping: bool,
        offset: &mut u32,
    ) -> usize {
        let (s, l) = (state, lazy_skipping);
        match (ctx.mls, ctx.row_log) {
            (5, 4) => row_find_best_match::<S, 5, 16>(simd, s, ctx, ip, l, offset),
            (5, 5) => row_find_best_match::<S, 5, 32>(simd, s, ctx, ip, l, offset),
            (5, _) => row_find_best_match::<S, 5, 64>(simd, s, ctx, ip, l, offset),
            (6, 4) => row_find_best_match::<S, 6, 16>(simd, s, ctx, ip, l, offset),
            (6, 5) => row_find_best_match::<S, 6, 32>(simd, s, ctx, ip, l, offset),
            (6, _) => row_find_best_match::<S, 6, 64>(simd, s, ctx, ip, l, offset),
            (_, 4) => row_find_best_match::<S, 4, 16>(simd, s, ctx, ip, l, offset),
            (_, 5) => row_find_best_match::<S, 4, 32>(simd, s, ctx, ip, l, offset),
            (_, _) => row_find_best_match::<S, 4, 64>(simd, s, ctx, ip, l, offset),
        }
    }

    fn fill_cache(state: &mut RowState<'_>, ctx: &Ctx<'_>, i_limit: i64) {
        let idx = state.next_to_update;
        let bits = state.bits;
        let cache = &mut *state.cache;
        let tables = (&*state.tags, &*state.slots);
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
fn insert_and_find_first_index<const MLS: u32>(
    ms: &mut MatchState,
    ctx: &Ctx<'_>,
    ip: u32,
    lazy_skipping: bool,
) -> u32 {
    let hash_log = ms.cparams.hash_log;
    let chain_mask = (1u32 << ms.cparams.chain_log) - 1;
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
    lazy_skipping: bool,
    offset: &mut u32,
) -> usize {
    let cparams = ms.cparams;
    let chain_size = 1u32 << cparams.chain_log;
    let chain_mask = chain_size - 1;
    let curr = ip;
    let low_limit = search_low_limit(ms, curr);
    let min_chain = curr.saturating_sub(chain_size);
    let nb_attempts = 1u32 << cparams.search_log;

    let first = insert_and_find_first_index::<MLS>(ms, ctx, ip, lazy_skipping);
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
    let start = ms.next_to_update;
    ms.next_to_update = ip;
    if start >= ip {
        return;
    }
    let hash_log = ms.cparams.hash_log;
    let bt_mask = (1u32 << (ms.cparams.chain_log - 1)) - 1;
    // The words hashed, cut once (see [`row_update_impl`]); the
    // reference's index invariants keep them in the prefix.
    let from = start.wrapping_sub(ctx.base) as usize;
    let Some(words) = ctx.src.get(from..from + (ip - start) as usize + 7) else {
        return update_dubt_checked::<MLS>(ms, ctx, start, ip);
    };
    let hash_table = ms.hash_table.as_mut_slice();
    // A node's two cells: the next candidate in the unsorted list, and the
    // mark that the node is not sorted yet.
    let nodes = ms.chain_table.as_chunks_mut::<2>().0;
    let mut idx = start;
    for word in words.windows(8) {
        let word = u64::from_le_bytes(word.try_into().unwrap_or_default());
        let h = mem::hash_word::<MLS>(word, hash_log);
        let match_index = core::mem::replace(&mut hash_table[h], idx);
        nodes[(idx & bt_mask) as usize] = [match_index, DUBT_UNSORTED_MARK];
        idx = idx.wrapping_add(1);
    }
}

/// [`update_dubt`] hashing through [`Ctx::hash`], for a range whose
/// words leave the prefix.
#[cold]
#[inline(never)]
fn update_dubt_checked<const MLS: u32>(ms: &mut MatchState, ctx: &Ctx<'_>, start: u32, ip: u32) {
    let hash_log = ms.cparams.hash_log;
    let bt_mask = (1u32 << (ms.cparams.chain_log - 1)) - 1;
    for idx in start..ip {
        let h = ctx.hash::<MLS>(idx, hash_log);
        let match_index = ms.hash_table[h];
        let slot = 2 * (idx & bt_mask) as usize;
        ms.hash_table[h] = idx;
        ms.chain_table[slot] = match_index;
        ms.chain_table[slot + 1] = DUBT_UNSORTED_MARK;
    }
}

/// A tree link not to write (`dummy32`): past every chain table, so the
/// check that guards a write also skips it.
const NO_LINK: usize = usize::MAX;

/// Writes `value` to the tree link `link`: a cell of the chain table, or
/// [`NO_LINK`].
#[inline(always)]
fn set_link(bt: &mut [u32], link: usize, value: u32) {
    if let Some(cell) = bt.get_mut(link) {
        *cell = value;
    }
}

/// The open links of a tree walk (`smallerPtr`, `largerPtr`) and the
/// lengths its run is known to share with each side.
struct TreeLinks {
    smaller: usize,
    larger: usize,
    common_smaller: usize,
    common_larger: usize,
}

impl TreeLinks {
    /// The links of the node at `slot` (its pair of cells).
    #[inline(always)]
    const fn new(slot: usize) -> Self {
        Self {
            smaller: slot,
            larger: slot + 1,
            common_smaller: 0,
            common_larger: 0,
        }
    }

    /// The length every candidate below the open links shares with the run.
    #[inline(always)]
    fn common(&self) -> usize {
        self.common_smaller.min(self.common_larger)
    }

    /// Attaches candidate `m` (whose cells start at `next`, sharing
    /// `match_length` bytes with the run) to the side it sorts on, and
    /// returns that side's child to visit next, or `None` when `m` is at
    /// or below `bt_low` and the walk ends.
    ///
    /// Which side a candidate takes is as likely one way as the other, so
    /// the side is selected without a branch; the reference's two arms do
    /// the same writes.
    #[inline(always)]
    fn descend(
        &mut self,
        bt: &mut [u32],
        m: u32,
        next: usize,
        match_length: usize,
        lower: bool,
        bt_low: u32,
    ) -> Option<u32> {
        use core::hint::select_unpredictable as select;
        set_link(bt, select(lower, self.smaller, self.larger), m);
        self.common_smaller = select(lower, match_length, self.common_smaller);
        self.common_larger = select(lower, self.common_larger, match_length);
        let ends = m <= bt_low;
        let child = if ends {
            NO_LINK
        } else {
            next + usize::from(lower)
        };
        self.smaller = select(lower, child, self.smaller);
        self.larger = select(lower, self.larger, child);
        if ends {
            return None;
        }
        bt.get(child).copied()
    }

    /// Closes both open links (`*smallerPtr = *largerPtr = 0`).
    #[inline(always)]
    fn close(&self, bt: &mut [u32]) {
        set_link(bt, self.smaller, 0);
        set_link(bt, self.larger, 0);
    }
}

/// The run a tree walk sorts candidates against: the bytes from position
/// `curr` to the walk's limit. Without an external dictionary (`!EXT`)
/// they are cut from the prefix once, so a step costs one check per side.
#[derive(Copy, Clone)]
struct TreeRun<'a> {
    curr: u32,
    limit: u32,
    /// The run's bytes (prefix only).
    current: &'a [u8],
}

impl<'a> TreeRun<'a> {
    #[inline(always)]
    fn new<const EXT: bool>(ctx: &Ctx<'a>, curr: u32, limit: u32) -> Self {
        let current = if EXT {
            &[]
        } else {
            ctx.src
                .get(curr.wrapping_sub(ctx.base) as usize..limit.wrapping_sub(ctx.base) as usize)
                .unwrap_or_default()
        };
        Self {
            curr,
            limit,
            current,
        }
    }

    /// The run's length.
    #[inline(always)]
    const fn len(&self) -> usize {
        (self.limit - self.curr) as usize
    }

    /// One step of a tree walk: the common length of
    /// the run and candidate `m`, which agree on their first `from` bytes,
    /// and whether the candidate sorts below the run.
    #[inline(always)]
    fn step<S: Simd, const EXT: bool>(
        &self,
        simd: S,
        ctx: &Ctx<'_>,
        m: u32,
        from: usize,
    ) -> (usize, bool) {
        if EXT {
            let at = from as u32;
            let (n, lower) = ctx.ext_tree_step(simd, self.curr + at, m + at, self.limit);
            return (from + n, lower);
        }
        let (current, src) = (self.current, ctx.src);
        // Lengths and window offsets fit in 32 bits; saying so lets the
        // compiler drop the overflow checks of the ranges below.
        let from = from & u32::MAX as usize;
        let at = m.wrapping_sub(ctx.base) as usize;
        // `ZSTD_count` inline, a word at a time, for the first
        // `TREE_INLINE_WORDS` words: a call here would make the walk keep its
        // state in memory across every step. Longer runs (run-length input
        // matches whole blocks) continue in the vector scan.
        let mut n = from;
        let mut words = 0;
        while let (Some(x), Some(y)) = (current.get(n..n + 8), src.get(at + n..at + n + 8)) {
            let x = u64::from_le_bytes(x.try_into().unwrap_or_default());
            let y = u64::from_le_bytes(y.try_into().unwrap_or_default());
            let difference = x ^ y;
            if difference != 0 {
                // The words read big-endian order as their bytes do, so the
                // first difference orders them.
                let lower = y.swap_bytes() < x.swap_bytes();
                return (n + (difference.trailing_zeros() >> 3) as usize, lower);
            }
            n += 8;
            words += 1;
            if words == TREE_INLINE_WORDS {
                return long_tree_step(simd, current, src.get(at..).unwrap_or_default(), n);
            }
        }
        while let (Some(x), Some(y)) = (current.get(n), src.get(at + n))
            && x == y
        {
            n += 1;
        }
        let lower = match (current.get(n), src.get(at + n)) {
            (Some(&x), Some(&y)) => y < x,
            _ => false,
        };
        (n, lower)
    }
}

/// Words a tree step compares inline before handing a run to
/// [`long_tree_step`].
const TREE_INLINE_WORDS: usize = 4;

/// The rest of a long tree step, past the `matched` bytes compared inline:
/// the common length of `current` and `candidate` by the vector scan, and
/// whether the candidate sorts below the run. Out of line and cold, so the
/// walk keeps its registers.
#[cold]
#[inline(never)]
fn long_tree_step<S: Simd>(
    simd: S,
    current: &[u8],
    candidate: &[u8],
    matched: usize,
) -> (usize, bool) {
    let (current_rest, candidate_rest) = (
        current.get(matched..).unwrap_or_default(),
        candidate.get(matched..).unwrap_or_default(),
    );
    let n = matched + mem::count_slices_simd(simd, current_rest, candidate_rest);
    let lower = match (current.get(n), candidate.get(n)) {
        (Some(&x), Some(&y)) => y < x,
        _ => false,
    };
    (n, lower)
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
    let run = TreeRun::new::<EXT>(ctx, curr, iend);
    let window_valid = ms.window.low_limit;
    let max_distance = 1u32 << cparams.window_log;
    let window_low = if curr.wrapping_sub(window_valid) > max_distance {
        curr - max_distance
    } else {
        window_valid
    };
    let bt = ms.chain_table.as_mut_slice();
    let mut links = TreeLinks::new(2 * (curr & bt_mask) as usize);
    let mut match_index = bt[links.smaller];

    while nb_compares > 0 && match_index > window_low {
        let next = 2 * (match_index & bt_mask) as usize;
        let (match_length, lower) = run.step::<S, EXT>(simd, ctx, match_index, links.common());
        if match_length == run.len() {
            break;
        }
        match links.descend(bt, match_index, next, match_length, lower, bt_low) {
            Some(child) => match_index = child,
            None => break,
        }
        nb_compares -= 1;
    }
    links.close(bt);
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
    let run = TreeRun::new::<EXT>(ctx, ip, ctx.iend);
    let mut match_end_idx = curr + 8 + 1;
    let mut best_length = 0usize;

    match_index = ms.hash_table[h];
    ms.hash_table[h] = curr;
    let mut links = TreeLinks::new(2 * (curr & bt_mask) as usize);
    let bt = ms.chain_table.as_mut_slice();

    while nb_compares > 0 && match_index > window_low {
        let next = 2 * (match_index & bt_mask) as usize;
        let (match_length, lower) = run.step::<S, EXT>(simd, ctx, match_index, links.common());
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
            if match_length == run.len() {
                break;
            }
        }
        match links.descend(bt, match_index, next, match_length, lower, bt_low) {
            Some(child) => match_index = child,
            None => break,
        }
        nb_compares -= 1;
    }
    links.close(bt);
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
    #[inline(always)]
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

/// `PREFETCH_L1`: hints that the bytes at `at` will be read soon. The
/// pointer is only handed to the prefetch instruction, never dereferenced, so
/// a hint past a table is harmless; targets without one ignore it.
#[inline(always)]
fn prefetch_l1(at: *const u8) {
    branches::prefetch_read_data::<u8, 0>(at);
}

/// `ZSTD_row_prefetch`: the hash and tag rows of `hash`, the first 64 bytes
/// of each, plus the hash row's next 64 bytes from 32-slot rows and the tag
/// row's next 32 bytes for 64-slot rows.
///
/// Each row's address is formed once, and the second line of a row is a
/// constant step from it.
#[inline(always)]
fn row_prefetch(tag_table: &[u8], hash_table: &[u32], hash: u32, row_log: u32) {
    let rel_row = ((hash >> ROW_HASH_TAG_BITS) << row_log) as usize;
    let slots = hash_table.as_ptr().wrapping_add(rel_row).cast::<u8>();
    prefetch_l1(slots);
    if row_log >= 5 {
        prefetch_l1(slots.wrapping_add(64));
    }
    let tags = tag_table.as_ptr().wrapping_add(rel_row);
    prefetch_l1(tags);
    if row_log == 6 {
        prefetch_l1(tags.wrapping_add(32));
    }
}

/// `ZSTD_row_nextIndex` of every head byte for rows of `ROW` slots: the
/// head steps down from the last slot to slot 1, skipping slot 0, which
/// holds it. A head left by a different row size is masked first, as the
/// reference masks it.
const fn next_index_table<const ROW: usize>() -> [u8; 256] {
    let row_mask = ROW - 1;
    let mut table = [0u8; 256];
    let mut head = 0usize;
    while head < 256 {
        let next = head.wrapping_sub(1) & row_mask;
        table[head] = if next == 0 { row_mask } else { next } as u8;
        head += 1;
    }
    table
}

/// The next head of a row, by row size.
struct NextIndex<const ROW: usize>;

impl<const ROW: usize> NextIndex<ROW> {
    const TABLE: [u8; 256] = next_index_table::<ROW>();
}

/// `ZSTD_row_nextIndex`: the next slot of a row, which also becomes its head.
///
/// One load from a 256-byte table replaces the reference's decrement, mask
/// and conditional move, whose wrap the compiler would otherwise branch on.
#[inline(always)]
fn row_next_index<const ROW: usize>(tag_row: &mut [u8; ROW]) -> usize {
    let next = NextIndex::<ROW>::TABLE[usize::from(tag_row[0])];
    tag_row[0] = next;
    usize::from(next) & (ROW - 1)
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
    state: &mut RowState<'_>,
    ctx: &Ctx<'_>,
    start: u32,
    end: u32,
) {
    if start >= end {
        return;
    }
    // The words hashed, `ROW_HASH_CACHE_SIZE` positions ahead of each
    // inserted one, cut once: the reference's index invariants keep them in
    // the prefix, and each window of the cut is then a word without a check
    // of its own.
    let from = start.wrapping_sub(ctx.base) as usize + ROW_HASH_CACHE_SIZE;
    let words = ctx.src.get(from..from + (end - start) as usize + 7);
    let Some(words) = words else {
        return row_update_checked::<MLS, ROW>(state, ctx, start, end);
    };
    let bits = state.bits;
    let cache = &mut *state.cache;
    let mut rows = Rows::<ROW>::new(state.tags, state.slots);
    let mut idx = start;
    for word in words.windows(8) {
        let word = u64::from_le_bytes(word.try_into().unwrap_or_default());
        let new_hash = mem::hash_word::<MLS>(word, bits) as u32;
        // `ZSTD_row_nextCachedHash` prefetches the rows of the hash it
        // caches, 8 positions ahead.
        rows.prefetch(new_hash);
        let hash = core::mem::replace(&mut cache[(idx & ROW_HASH_CACHE_MASK) as usize], new_hash);
        rows.insert(hash, idx);
        idx = idx.wrapping_add(1);
    }
}

/// [`row_update_impl`] reading each word through [`Ctx::hash`], for a
/// range whose words leave the prefix.
#[cold]
#[inline(never)]
fn row_update_checked<const MLS: u32, const ROW: usize>(
    state: &mut RowState<'_>,
    ctx: &Ctx<'_>,
    start: u32,
    end: u32,
) {
    let bits = state.bits;
    let cache = &mut *state.cache;
    let mut rows = Rows::<ROW>::new(state.tags, state.slots);
    for idx in start..end {
        let hash = row_next_cached_hash::<MLS>(cache, ctx, bits, idx);
        rows.prefetch(cache[(idx & ROW_HASH_CACHE_MASK) as usize]);
        rows.insert(hash, idx);
    }
}

/// `ZSTD_row_update_internal` with the hash cache.
#[inline(always)]
fn row_update<const MLS: u32, const ROW: usize>(state: &mut RowState<'_>, ctx: &Ctx<'_>, ip: u32) {
    const SKIP_THRESHOLD: u32 = 384;
    const MAX_MATCH_START_POSITIONS_TO_UPDATE: u32 = 96;
    const MAX_MATCH_END_POSITIONS_TO_UPDATE: u32 = 32;
    let mut idx = state.next_to_update;
    let target = ip;
    if target - idx > SKIP_THRESHOLD {
        let bound = idx + MAX_MATCH_START_POSITIONS_TO_UPDATE;
        row_update_impl::<MLS, ROW>(state, ctx, idx, bound);
        idx = target - MAX_MATCH_END_POSITIONS_TO_UPDATE;
        let tables = (&*state.tags, &*state.slots);
        let i_limit = i64::from(ip) + 1;
        row_fill_hash_cache::<MLS>(state.cache, tables, ctx, state.bits, idx, i_limit);
    }
    row_update_impl::<MLS, ROW>(state, ctx, idx, target);
    state.next_to_update = target;
}

/// The row finder's view of the match state for one block: its tables, and
/// the constants each search would otherwise derive from the parameters.
struct RowState<'t> {
    tags: &'t mut [u8],
    slots: &'t mut [u32],
    cache: &'t mut [u32; ROW_HASH_CACHE_SIZE],
    /// `nextToUpdate`, kept here and stored back when the block ends.
    next_to_update: u32,
    /// The match state's `nextToUpdate`.
    ms_next_to_update: &'t mut u32,
    /// `rowHashLog + ZSTD_ROW_HASH_TAG_BITS`.
    bits: u32,
    /// The window's lowest valid index.
    lowest_valid: u32,
    /// The longest distance searched: the window size, or no limit once a
    /// dictionary is loaded (`lowLimit` is then the lowest valid index).
    max_distance: u32,
    /// Candidates collected per search (`1 << cappedSearchLog`), at most a
    /// row.
    nb_attempts: usize,
    /// `matchBuffer`, kept across searches so it is never cleared.
    candidates: [u32; 64],
}

impl<'t> RowState<'t> {
    fn new(ms: &'t mut MatchState, ctx: &Ctx<'_>) -> Self {
        let max_distance = if ms.loaded_dict_end != 0 {
            u32::MAX
        } else {
            1u32 << ms.cparams.window_log
        };
        let nb_attempts = 1usize << ms.cparams.search_log.min(ctx.row_log);
        let bits = ms.row_hash_log + ROW_HASH_TAG_BITS;
        let lowest_valid = ms.window.low_limit;
        let rows_len = 1usize << ms.cparams.hash_log;
        let MatchState {
            tag_table,
            hash_table,
            hash_cache,
            next_to_update,
            ..
        } = ms;
        Self {
            tags: aligned_rows(tag_table, rows_len),
            slots: aligned_rows(hash_table, rows_len),
            cache: hash_cache,
            next_to_update: *next_to_update,
            ms_next_to_update: next_to_update,
            bits,
            lowest_valid,
            max_distance,
            nb_attempts,
            candidates: [0; 64],
        }
    }

    /// `lowLimit` of a search at `curr` ([`search_low_limit`]).
    #[inline(always)]
    const fn low_limit(&self, curr: u32) -> u32 {
        if curr.wrapping_sub(self.lowest_valid) > self.max_distance {
            curr - self.max_distance
        } else {
            self.lowest_valid
        }
    }
}

/// The first `len` entries of `table` from its first 64-byte boundary, as
/// the reference places its tables (`ZSTD_cwksp_reserve_aligned64`): a row
/// of tags or positions then never straddles a cache line, and its vector
/// load never splits. A table without [`ROW_TABLE_SLACK`] entries to spare
/// is used from its start.
///
/// The view's position in the allocation does not change the frames: a
/// search reads only the entries the finder wrote since the window's
/// indices were last invalidated, and every older entry, wherever it lies,
/// is below `lowLimit` and ends the search as a zero would.
fn aligned_rows<T>(table: &mut [T], len: usize) -> &mut [T] {
    const ALIGN: usize = 64;
    let skip = table.as_ptr().addr().wrapping_neg() % ALIGN / size_of::<T>().max(1);
    if skip < ROW_TABLE_SLACK && skip + len <= table.len() {
        &mut table[skip..skip + len]
    } else {
        let len = len.min(table.len());
        &mut table[..len]
    }
}

impl Drop for RowState<'_> {
    fn drop(&mut self) {
        *self.ms_next_to_update = self.next_to_update;
    }
}

/// `ZSTD_RowFindBestMatch`.
#[inline(always)]
fn row_find_best_match<S: Simd, const MLS: u32, const ROW: usize>(
    simd: S,
    state: &mut RowState<'_>,
    ctx: &Ctx<'_>,
    ip: u32,
    lazy_skipping: bool,
    offset: &mut u32,
) -> usize {
    let curr = ip;
    let low_limit = state.low_limit(curr);
    let bits = state.bits;
    if !lazy_skipping {
        row_update::<MLS, ROW>(state, ctx, ip);
    }
    let rows = Rows::<ROW>::new(state.tags, state.slots);
    let cache = &mut *state.cache;
    let hash = if !lazy_skipping {
        let hash = row_next_cached_hash::<MLS>(cache, ctx, bits, curr);
        rows.prefetch(cache[(curr & ROW_HASH_CACHE_MASK) as usize]);
        hash
    } else {
        state.next_to_update = curr;
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
    // (one fused pass measured 15-20% slower).
    let mut num_matches = 0usize;
    let buffer = &mut state.candidates;
    // Window index `i` of the prefix is at `window + i` (`base`); the
    // pointer only feeds prefetches.
    let window = ctx.src.as_ptr().wrapping_sub(ctx.base as usize);
    if state.nb_attempts >= ROW {
        // At most `ROW - 1` slots match (slot 0 never does), so the
        // attempts cannot run out and the loop counts only the candidates,
        // which also stay below the buffer's 64 slots.
        while matches != 0 {
            let match_pos = (head as usize + matches.trailing_zeros() as usize) & (ROW - 1);
            matches &= matches - 1;
            let match_index = slot_row[match_pos];
            if match_index < low_limit {
                break;
            }
            // `PREFETCH_L1(base + matchIndex)`: the candidates' bytes load
            // while the rest are collected.
            prefetch_l1(window.wrapping_add(match_index as usize));
            buffer[num_matches & 63] = match_index;
            num_matches += 1;
        }
    } else {
        // Fewer attempts than a row has slots, all within the buffer.
        let nb_attempts = state.nb_attempts;
        while matches != 0 && num_matches < nb_attempts {
            let match_pos = (head as usize + matches.trailing_zeros() as usize) & (ROW - 1);
            matches &= matches - 1;
            let match_index = slot_row[match_pos];
            if match_index < low_limit {
                break;
            }
            prefetch_l1(window.wrapping_add(match_index as usize));
            buffer[num_matches & 63] = match_index;
            num_matches += 1;
        }
    }

    // Insert the current position too.
    {
        let pos = row_next_index(tag_row);
        tag_row[pos] = tag;
        slot_row[pos] = state.next_to_update;
        state.next_to_update += 1;
    }

    let candidates = &state.candidates[..num_matches.min(64)];
    if ctx.ext {
        best_candidate::<S, true>(simd, ctx, ip, candidates, offset)
    } else {
        best_prefix_candidate(simd, ctx, ip, candidates, offset)
    }
}

/// [`best_candidate`] without an external dictionary: every candidate is a
/// prefix index below `ip`.
///
/// The best length's probe word is compared against `hay`, the prefix
/// shifted by that length, so a candidate costs one load and one check; a
/// candidate that shares it is measured with its first word inline.
#[inline(always)]
fn best_prefix_candidate<S: Simd>(
    simd: S,
    ctx: &Ctx<'_>,
    ip: u32,
    candidates: &[u32],
    offset: &mut u32,
) -> usize {
    let src = ctx.src;
    let base = ctx.base;
    // The block may end before the prefix does (long distance matching
    // runs the finder on parts of a block), and no match runs past it.
    let current = src
        .get((ip - base) as usize..(ctx.iend - base) as usize)
        .unwrap_or_default();
    // The searched position is at least 16 bytes before the block's end
    // (`ilimit`); a shorter tail takes the general loop.
    let Some(first) = current.first_chunk::<8>() else {
        return best_candidate::<S, false>(simd, ctx, ip, candidates, offset);
    };
    let first = u64::from_le_bytes(*first);
    let mut ml: usize = 4 - 1;
    // `current[ml - 3..ml + 1]`, and `hay[i]` is `src[i + ml - 3]`.
    let mut word = first as u32;
    let mut hay = src;
    for &match_index in candidates {
        let at = match_index.wrapping_sub(base) as usize;
        match hay.get(at..at + 4) {
            Some(bytes) if u32::from_le_bytes(bytes.try_into().unwrap_or_default()) == word => {}
            _ => continue,
        }
        let current_ml = match src.get(at..at + 8) {
            Some(y) => match first ^ u64::from_le_bytes(y.try_into().unwrap_or_default()) {
                0 => count::common_run(simd, current, &src[at..]),
                difference => (difference.trailing_zeros() >> 3) as usize,
            },
            None => count::common_run(simd, current, &src[at..]),
        };
        if current_ml > ml {
            ml = current_ml;
            *offset = offset_to_offbase(ip - match_index);
            // A match below the tail's length leaves the bytes of the next
            // probe word in it.
            let (Some(next), Some(next_hay)) = (current.get(ml - 3..ml + 1), src.get(ml - 3..))
            else {
                break;
            };
            word = u32::from_le_bytes(next.try_into().unwrap_or_default());
            hay = next_hay;
        }
    }
    ml
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
    let mut lazy_skipping = false;
    let mut state = M::state(ms, &ctx);
    M::fill_cache(&mut state, &ctx, ilimit);

    while ip < ilimit {
        let mut match_length = 0usize;
        let mut off_base = repcode_to_offbase(1);
        let mut start = ip + 1;
        let mut store_now = false;

        if offset_1 > 0 && ctx.repeats((ip + 1) as u32, offset_1) {
            match_length = count(ip + 1 + 4, ip + 1 + 4 - i64::from(offset_1)) + 4;
            if DEPTH == 0 {
                store_now = true;
            }
        }

        if !store_now {
            let mut found = NO_OFFBASE;
            let ml2 = M::find(simd, &mut state, &ctx, ip as u32, lazy_skipping, &mut found);
            if ml2 > match_length {
                match_length = ml2;
                start = ip;
                off_base = found;
            }

            if match_length < 4 {
                let step = ((ip - anchor) >> SEARCH_STRENGTH) as usize + 1;
                ip += step as i64;
                lazy_skipping = step > LAZY_SKIPPING_STEP;
                continue;
            }

            if DEPTH >= 1 {
                while ip < ilimit {
                    ip += 1;
                    if off_base != 0 && offset_1 > 0 && ctx.repeats(ip as u32, offset_1) {
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
                        let ml2 = M::find(
                            simd,
                            &mut state,
                            &ctx,
                            ip as u32,
                            lazy_skipping,
                            &mut candidate,
                        );
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
                        if off_base != 0 && offset_1 > 0 && ctx.repeats(ip as u32, offset_1) {
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
                        let ml2 = M::find(
                            simd,
                            &mut state,
                            &ctx,
                            ip as u32,
                            lazy_skipping,
                            &mut candidate,
                        );
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
        if lazy_skipping {
            M::fill_cache(&mut state, &ctx, ilimit);
            lazy_skipping = false;
        }

        while ip <= ilimit && offset_2 > 0 && ctx.repeats(ip as u32, offset_2) {
            let ml = count(ip + 4, ip + 4 - i64::from(offset_2)) + 4;
            core::mem::swap(&mut offset_1, &mut offset_2);
            seq_store.store_seq_only(0, repcode_to_offbase(1), ml);
            ip += ml as i64;
            anchor = ip;
        }
    }

    drop(state);
    ms.lazy_skipping = lazy_skipping;
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
    let (lowest_valid, dict_loaded) = (ms.window.low_limit, ms.loaded_dict_end != 0);
    // `ZSTD_getLowestMatchIndex`.
    let lowest_match_index = move |curr: u32| {
        let max_distance = 1u32 << window_log;
        if dict_loaded || curr.wrapping_sub(lowest_valid) <= max_distance {
            lowest_valid
        } else {
            curr - max_distance
        }
    };
    let rep_ok = |curr: u32, offset: u32| {
        let window_low = lowest_match_index(curr);
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

    let mut lazy_skipping = false;
    let mut ip = istart;
    let mut anchor = istart;
    ip += i64::from(ip == prefix_start);
    let mut state = M::state(ms, &ctx);
    M::fill_cache(&mut state, &ctx, ilimit);

    while ip < ilimit {
        let mut match_length = 0usize;
        let mut off_base = repcode_to_offbase(1);
        let mut start = ip + 1;
        let mut curr = ip as u32;
        let mut store_now = false;

        if let Some(length) = rep_ok(curr + 1, offset_1) {
            match_length = length;
            if DEPTH == 0 {
                store_now = true;
            }
        }

        if !store_now {
            let mut candidate = NO_OFFBASE;
            let ml2 = M::find(
                simd,
                &mut state,
                &ctx,
                ip as u32,
                lazy_skipping,
                &mut candidate,
            );
            if ml2 > match_length {
                match_length = ml2;
                start = ip;
                off_base = candidate;
            }
            if match_length < 4 {
                let step = ((ip - anchor) >> SEARCH_STRENGTH) as usize;
                ip += step as i64 + 1;
                lazy_skipping = step > LAZY_SKIPPING_STEP;
                continue;
            }
            if DEPTH >= 1 {
                while ip < ilimit {
                    ip += 1;
                    curr += 1;
                    if off_base != 0
                        && let Some(rep_length) = rep_ok(curr, offset_1)
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
                        let ml2 = M::find(
                            simd,
                            &mut state,
                            &ctx,
                            ip as u32,
                            lazy_skipping,
                            &mut candidate,
                        );
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
                            && let Some(rep_length) = rep_ok(curr, offset_1)
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
                        let ml2 = M::find(
                            simd,
                            &mut state,
                            &ctx,
                            ip as u32,
                            lazy_skipping,
                            &mut candidate,
                        );
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
        if lazy_skipping {
            M::fill_cache(&mut state, &ctx, ilimit);
            lazy_skipping = false;
        }

        while ip <= ilimit {
            let Some(ml) = rep_ok(ip as u32, offset_2) else {
                break;
            };
            core::mem::swap(&mut offset_1, &mut offset_2);
            seq_store.store_seq_only(0, repcode_to_offbase(1), ml);
            ip += ml as i64;
            anchor = ip;
        }
    }
    drop(state);
    ms.lazy_skipping = lazy_skipping;
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
    use super::super::params::{ParamSwitch, Strategy, level_cparams};
    use super::super::stream::{EndOp, Stream};
    use super::*;
    use crate::Backend;
    use zstd_ffi::corpus;
    use zstd_ffi::oracle::{self, Directive, Params};

    /// A prefix-only block over `bytes`, starting at window index 2.
    fn prefix_block(bytes: &[u8]) -> BlockInput<'_> {
        BlockInput {
            window: mem::Window {
                prefix: Mem::new(bytes, 2),
                ext: Mem::new(&[], 2),
            },
            start: 2,
            end: 2 + bytes.len() as u32,
            backend: Backend::default(),
        }
    }

    #[test]
    fn next_index_tables_follow_the_reference() {
        fn check<const ROW: usize>() {
            let mask = ROW as u32 - 1;
            for head in 0..=255u8 {
                let mut row = [0u8; ROW];
                row[0] = head;
                let next = u32::from(head).wrapping_sub(1) & mask;
                let expected = if next == 0 { mask } else { next };
                assert_eq!(row_next_index(&mut row), expected as usize, "{ROW} {head}");
                assert_eq!(u32::from(row[0]), expected);
            }
            // Built at run time too, not only into the constant.
            assert_eq!(next_index_table::<ROW>(), NextIndex::<ROW>::TABLE);
        }
        check::<16>();
        check::<32>();
        check::<64>();
    }

    #[test]
    fn row_tables_start_at_a_cache_line() {
        let mut slots = vec![0u32; 1024 + ROW_TABLE_SLACK];
        let view = aligned_rows(&mut slots, 1024);
        assert_eq!(view.len(), 1024);
        assert_eq!(view.as_ptr().addr() % 64, 0);
        let mut tags = vec![0u8; 512 + ROW_TABLE_SLACK];
        let view = aligned_rows(&mut tags, 512);
        assert_eq!(view.len(), 512);
        assert_eq!(view.as_ptr().addr() % 64, 0);
        // Without room to move, a table is used from its start.
        let mut short = vec![0u8; 100];
        let start = short.as_ptr();
        assert_eq!(aligned_rows(&mut short, 200).as_ptr(), start);
        assert_eq!(aligned_rows(&mut short, 200).len(), 100);
    }

    /// The per-index fallback of a row update inserts exactly what the
    /// word loop inserts.
    #[test]
    fn checked_row_updates_insert_like_the_word_loop() {
        fn check<const MLS: u32, const ROW: usize>(bytes: &[u8]) {
            let block = prefix_block(bytes);
            let row_log = ROW.trailing_zeros();
            let mut cparams = level_cparams(6, u64::MAX);
            cparams.search_log = row_log;
            cparams.min_match = MLS;
            let ctx = Ctx::new(&block, MLS, row_log, false);
            let mut fast = MatchState::new(cparams, ParamSwitch::Enable);
            let mut checked = MatchState::new(cparams, ParamSwitch::Enable);
            for (ms, by_index) in [(&mut fast, false), (&mut checked, true)] {
                let mut state = RowState::new(ms, &ctx);
                for (start, end) in [(12, 300), (300, 301), (301, 2000)] {
                    if by_index {
                        row_update_checked::<MLS, ROW>(&mut state, &ctx, start, end);
                    } else {
                        row_update_impl::<MLS, ROW>(&mut state, &ctx, start, end);
                    }
                }
            }
            // Each state's rows start where its own allocation aligns.
            let rows = |ms: &mut MatchState| {
                let len = 1usize << ms.cparams.hash_log;
                let tags = aligned_rows(&mut ms.tag_table, len).to_vec();
                (tags, aligned_rows(&mut ms.hash_table, len).to_vec())
            };
            assert!(rows(&mut fast) == rows(&mut checked), "{MLS} {ROW}");
            assert_eq!(fast.hash_cache, checked.hash_cache, "{MLS} {ROW}");
        }
        let bytes = corpus::text(4096, 61);
        check::<4, 16>(&bytes);
        check::<5, 32>(&bytes);
        check::<6, 64>(&bytes);
    }

    /// The per-index fallback of a tree update inserts exactly what the
    /// word loop inserts.
    #[test]
    fn checked_tree_updates_insert_like_the_word_loop() {
        let bytes = corpus::text(4096, 62);
        let block = prefix_block(&bytes);
        let cparams = level_cparams(13, u64::MAX);
        let ctx = Ctx::new(&block, 5, 4, false);
        let mut fast = MatchState::new(cparams, ParamSwitch::Disable);
        let mut checked = MatchState::new(cparams, ParamSwitch::Disable);
        fast.next_to_update = 12;
        update_dubt::<5>(&mut fast, &ctx, 3000);
        update_dubt::<5>(&mut fast, &ctx, 3000);
        update_dubt_checked::<5>(&mut checked, &ctx, 12, 3000);
        assert_eq!(fast.next_to_update, 3000);
        assert!(fast.hash_table == checked.hash_table);
        assert!(fast.chain_table == checked.chain_table);
    }

    /// A tree step in the prefix measures and orders runs as the
    /// external-dictionary step does, from any known common length.
    #[test]
    fn prefix_tree_steps_match_the_general_step() {
        let mut bytes = corpus::text(600, 63);
        bytes.extend_from_within(..300);
        let block = prefix_block(&bytes);
        let end = block.end;
        for backend in Backend::available() {
            fearless_simd::dispatch!(backend.0, simd => {
                let ctx = Ctx::new(&block, 4, 4, false);
                for (curr, limit) in [(602, end), (650, end), (700, 720)] {
                    let prefix = TreeRun::new::<false>(&ctx, curr, limit);
                    let general = TreeRun::new::<true>(&ctx, curr, limit);
                    for m in 2..curr {
                        let (full, _) = general.step::<_, true>(simd, &ctx, m, 0);
                        for known in [0usize, 1, 7, 8, 9, 30].map(|k| k.min(full)) {
                            let expected = general.step::<_, true>(simd, &ctx, m, known);
                            let got = prefix.step::<_, false>(simd, &ctx, m, known);
                            assert_eq!(got, expected, "{backend} {curr} {m} {known}");
                        }
                    }
                }
            });
        }
    }

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
                    .compress(engine, requested, &mut buffer[..], &data[in_pos..], op)
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
