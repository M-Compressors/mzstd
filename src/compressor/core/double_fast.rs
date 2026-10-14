//! The `dfast` strategy (`zstd_double_fast.c`): a long (8-byte) hash table
//! in `hash_table` and a short one in `chain_table`.

use core::cell::Cell;

use fearless_simd::Simd;
use fearless_simd_macros::simd;

use super::match_state::{BlockInput, MatchState};
use super::mem::{count_slices_simd, hash_ptr, hash_word, load32, load64, try_load64};
use super::seq_store::{SeqStore, offset_to_offbase, repcode_to_offbase};
use super::window::HASH_READ_SIZE;

/// `kSearchStrength`.
const SEARCH_STRENGTH: u32 = 8;

/// `ZSTD_fillDoubleHashTableForCCtx` with `ZSTD_dtlm_fast`.
pub(crate) fn fill_double_hash_table(ms: &mut MatchState, prefix: &super::mem::Mem<'_>, end: u32) {
    let h_bits_l = ms.cparams.hash_log;
    let h_bits_s = ms.cparams.chain_log;
    let mls = ms.cparams.min_match;
    let iend = i64::from(end) - i64::from(HASH_READ_SIZE);
    let mut ip = i64::from(ms.next_to_update);
    while ip + 2 <= iend {
        let curr = ip as u32;
        ms.chain_table[hash_ptr(prefix, curr, h_bits_s, mls)] = curr;
        ms.hash_table[hash_ptr(prefix, curr, h_bits_l, 8)] = curr;
        ip += 3;
    }
}

/// `ZSTD_compressBlock_doubleFast`: selects the variant compiled for the
/// minimum match length, as the reference's `ZSTD_GEN_DFAST_FN` templates
/// do.
pub(crate) fn compress_block(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    fearless_simd::dispatch!(ms.backend.0, simd => match ms.cparams.min_match {
        5 => compress_block_mls::<_, 5>(simd, ms, seq_store, rep, block),
        6 => compress_block_mls::<_, 6>(simd, ms, seq_store, rep, block),
        7.. => compress_block_mls::<_, 7>(simd, ms, seq_store, rep, block),
        _ => compress_block_mls::<_, 4>(simd, ms, seq_store, rep, block),
    })
}

/// `ZSTD_compressBlock_doubleFast_noDict_generic` for one minimum match
/// length, compiled for one SIMD level.
#[simd]
fn compress_block_mls<S: Simd, const MLS: u32>(
    simd: S,
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    let cparams = ms.cparams;
    let prefix = block.window.prefix;
    let base = prefix.start();
    let prefix_lowest_index = ms.lowest_prefix_index(block.end, cparams.window_log);
    let mut offset_1 = rep[0];
    let mut offset_2 = rep[1];
    let mut offset_saved1 = 0u32;
    let mut offset_saved2 = 0u32;

    let mut ip = block.start;
    ip += u32::from(ip == prefix_lowest_index);
    {
        let window_low = ms.lowest_prefix_index(ip, cparams.window_log);
        let max_rep = ip - window_low;
        if offset_2 > max_rep {
            offset_saved2 = offset_2;
            offset_2 = 0;
        }
        if offset_1 > max_rep {
            offset_saved1 = offset_1;
            offset_1 = 0;
        }
    }

    let MatchState {
        hash_table: hash_long,
        chain_table: hash_small,
        ..
    } = ms;
    let tables = Tables {
        long: hash_long.as_mut_slice(),
        small: hash_small.as_mut_slice(),
    };
    let scan = |origin: u32| Scan {
        src: prefix.slice(origin, block.end),
        origin,
        prefix_lowest: (prefix_lowest_index - origin) as usize,
        start: (block.start - origin) as usize,
        h_bits_l: cparams.hash_log,
        h_bits_s: cparams.chain_log,
    };
    // Every repcode the search can use either is one of these two or was
    // found at a candidate inside the prefix, so when these two reach no
    // further back than the first position the block reads nothing below
    // the prefix: it is then searched as offsets from the prefix's start,
    // which spares the search loop the window's base and the history's
    // bounds.
    let in_prefix =
        block.start >= prefix_lowest_index && offset_1.max(offset_2) <= ip - prefix_lowest_index;
    let (literals, [rep_1, rep_2]) = if in_prefix {
        let scan = scan(prefix_lowest_index);
        let ip = (ip - prefix_lowest_index) as usize;
        compress_scan::<S, MLS, true>(simd, tables, seq_store, &scan, ip, [offset_1, offset_2])
    } else {
        let scan = scan(base);
        let ip = (ip - base) as usize;
        compress_scan::<S, MLS, false>(simd, tables, seq_store, &scan, ip, [offset_1, offset_2])
    };
    offset_1 = rep_1;
    offset_2 = rep_2;

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
    literals
}

/// The two hash tables a `dfast` block reads and fills.
struct Tables<'a> {
    /// Eight-byte hashes (`hashLong`).
    long: &'a mut [u32],
    /// `MLS`-byte hashes (`hashSmall`).
    small: &'a mut [u32],
}

/// The bytes one `dfast` block searches and the parameters it searches them
/// with.
struct Scan<'a> {
    /// The block and the history before it, from window index `origin` on.
    src: &'a [u8],
    /// Window index of `src[0]`: the reference's `base`, as an offset.
    origin: u32,
    /// Offset of the prefix's lowest valid index, below which no candidate
    /// is taken.
    prefix_lowest: usize,
    /// Offset of the block's first byte.
    start: usize,
    /// `hashLog`, the long table's hash width.
    h_bits_l: u32,
    /// `chainLog`, the short table's hash width.
    h_bits_s: u32,
}

/// The search and match loop of `ZSTD_compressBlock_doubleFast_noDict_generic`
/// from offset `ip` of `scan.src`, with the repcodes `reps`. Returns the
/// length of the trailing literals and the final repcodes.
///
/// `IN_PREFIX` states that `scan.prefix_lowest` is zero, so the candidates'
/// history is all of `src` and its lower bound the window index `origin`.
#[inline(always)]
fn compress_scan<S: Simd, const MLS: u32, const IN_PREFIX: bool>(
    simd: S,
    tables: Tables<'_>,
    seq_store: &mut SeqStore,
    scan: &Scan<'_>,
    mut ip: usize,
    reps: [u32; 2],
) -> (usize, [u32; 2]) {
    let Scan {
        src,
        origin: base,
        h_bits_l,
        h_bits_s,
        ..
    } = *scan;
    let prefix_lowest = if IN_PREFIX { 0 } else { scan.prefix_lowest };
    let prefix_lowest_index = base + prefix_lowest as u32;
    // The candidate load's bound also enforces the searchable lower limit.
    let history = &src[prefix_lowest..];
    let iend = src.len();
    let mut anchor = scan.start;
    // A block shorter than HASH_READ_SIZE never enters the search: the
    // first `ip1 > ilimit` test fails for any `ilimit < 1`.
    let ilimit = iend.saturating_sub(HASH_READ_SIZE as usize);
    let [mut offset_1, mut offset_2] = reps;
    let k_step_incr: usize = 1 << SEARCH_STRENGTH;

    let index = |p: usize| p as u32 + base;
    let read32 = |p: usize| load32(src, p);
    let count = |a: usize, b: usize| count_slices_simd(simd, &src[a..], &src[b..]);

    let Tables {
        long: hash_long,
        small: hash_small,
    } = tables;
    // Hashes are below the table sizes (`1 << hash_log`, `1 << chain_log`);
    // masking with `len - 1` changes nothing and, with the tables known to
    // be non-empty, proves every index in bounds.
    assert!(!hash_long.is_empty() && !hash_small.is_empty());
    let (mask_l, mask_s) = (hash_long.len() - 1, hash_small.len() - 1);
    // The search keeps references to the slots it will write rather than
    // their hashes, so a slot is bounds checked once, where its hash is
    // computed.
    let hash_long = Cell::from_mut(hash_long).as_slice_of_cells();
    let hash_small = Cell::from_mut(hash_small).as_slice_of_cells();
    let slot_l = |word: u64| &hash_long[hash_word::<8>(word, h_bits_l) & mask_l];
    let slot_s = |word: u64| &hash_small[hash_word::<MLS>(word, h_bits_s) & mask_s];
    let hash_l = |p: usize| slot_l(load64(src, p));
    let hash_s = |p: usize| slot_s(load64(src, p));

    'outer: loop {
        let mut step: usize = 1;
        let mut next_step = ip + k_step_incr;
        let mut ip1 = ip + step;
        // The eight bytes at `ip` and at `ip1`: every hash and compare at a
        // searched position reads them. The search goes on while
        // `ip1 <= ilimit`, that is while the eight bytes at `ip1` are inside
        // `src`, so loading them is the loop's exit test.
        let Some(mut word1) = try_load64(src, ip1) else {
            break 'outer;
        };
        let mut word0 = load64(src, ip);
        let mut slot_l0 = slot_l(word0);
        let mut idxl0 = slot_l0.get();
        let mut curr: u32;

        enum Next<'t> {
            Stored(usize),
            Found {
                offset: u32,
                m_length: usize,
                slot_l1: &'t Cell<u32>,
            },
            SearchNextLong {
                idxs0: u32,
                idxl1: u32,
                slot_l1: &'t Cell<u32>,
            },
        }
        let next = loop {
            let slot_s0 = slot_s(word0);
            let idxs0 = slot_s0.get();
            curr = index(ip);
            slot_l0.set(curr);
            slot_s0.set(curr);

            if offset_1 > 0 && read32(ip + 1 - offset_1 as usize) == (word0 >> 8) as u32 {
                let m_length = count(ip + 1 + 4, ip + 1 + 4 - offset_1 as usize) + 4;
                ip += 1;
                seq_store.store_seq_from(src, anchor, ip - anchor, repcode_to_offbase(1), m_length);
                break Next::Stored(m_length);
            }

            let slot_l1 = slot_l(word1);
            if super::mem::matches64(history, idxl0.wrapping_sub(prefix_lowest_index), word0) {
                let mut matchl0 = (idxl0 - base) as usize;
                let mut m_length = count(ip + 8, matchl0 + 8) + 8;
                let offset = (ip - matchl0) as u32;
                while ip > anchor && matchl0 > prefix_lowest && src[ip - 1] == src[matchl0 - 1] {
                    ip -= 1;
                    matchl0 -= 1;
                    m_length += 1;
                }
                break Next::Found {
                    offset,
                    m_length,
                    slot_l1,
                };
            }

            let idxl1 = slot_l1.get();
            if super::mem::matches32(
                history,
                idxs0.wrapping_sub(prefix_lowest_index),
                word0 as u32,
            ) {
                break Next::SearchNextLong {
                    idxs0,
                    idxl1,
                    slot_l1,
                };
            }

            if ip1 >= next_step {
                step += 1;
                next_step += k_step_incr;
            }
            ip = ip1;
            ip1 += step;
            slot_l0 = slot_l1;
            idxl0 = idxl1;
            word0 = word1;
            match try_load64(src, ip1) {
                Some(word) => word1 = word,
                None => break 'outer,
            }
        };

        let m_length = match next {
            Next::Stored(m_length) => m_length,
            Next::Found {
                offset,
                m_length,
                slot_l1,
            } => {
                offset_2 = offset_1;
                offset_1 = offset;
                if step < 4 {
                    slot_l1.set(index(ip1));
                }
                seq_store.store_seq_from(
                    src,
                    anchor,
                    ip - anchor,
                    offset_to_offbase(offset),
                    m_length,
                );
                m_length
            }
            Next::SearchNextLong {
                idxs0,
                idxl1,
                slot_l1,
            } => {
                let mut matchs0 = (idxs0 - base) as usize;
                let mut m_length = count(ip + 4, matchs0 + 4) + 4;
                let mut offset = (ip - matchs0) as u32;
                // This second long candidate uses a strict lower limit in C.
                if super::mem::matches64(
                    &history[1..],
                    idxl1.wrapping_sub(prefix_lowest_index + 1),
                    word1,
                ) {
                    let matchl1 = (idxl1 - base) as usize;
                    let l1len = count(ip1 + 8, matchl1 + 8) + 8;
                    if l1len > m_length {
                        ip = ip1;
                        m_length = l1len;
                        offset = (ip - matchl1) as u32;
                        matchs0 = matchl1;
                    }
                }
                while ip > anchor && matchs0 > prefix_lowest && src[ip - 1] == src[matchs0 - 1] {
                    ip -= 1;
                    matchs0 -= 1;
                    m_length += 1;
                }
                offset_2 = offset_1;
                offset_1 = offset;
                if step < 4 {
                    slot_l1.set(index(ip1));
                }
                seq_store.store_seq_from(
                    src,
                    anchor,
                    ip - anchor,
                    offset_to_offbase(offset),
                    m_length,
                );
                m_length
            }
        };

        // _match_stored:
        ip += m_length;
        anchor = ip;
        if ip <= ilimit {
            let index_to_insert = curr + 2;
            let insert_at = (index_to_insert - base) as usize;
            hash_l(insert_at).set(index_to_insert);
            hash_l(ip - 2).set(index(ip - 2));
            hash_s(insert_at).set(index_to_insert);
            hash_s(ip - 1).set(index(ip - 1));

            while ip <= ilimit && offset_2 > 0 && read32(ip) == read32(ip - offset_2 as usize) {
                let r_length = count(ip + 4, ip + 4 - offset_2 as usize) + 4;
                core::mem::swap(&mut offset_1, &mut offset_2);
                hash_s(ip).set(index(ip));
                hash_l(ip).set(index(ip));
                seq_store.store_seq_only(0, repcode_to_offbase(1), r_length);
                ip += r_length;
                anchor = ip;
            }
        }
    }

    (iend - anchor, [offset_1, offset_2])
}

/// `ZSTD_compressBlock_doubleFast_extDict`: selects the variant compiled for
/// the minimum match length, dispatched once per block.
pub(crate) fn compress_block_ext_dict(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    fearless_simd::dispatch!(ms.backend.0, simd => match ms.cparams.min_match {
        5 => compress_block_ext_dict_mls::<_, 5>(simd, ms, seq_store, rep, block),
        6 => compress_block_ext_dict_mls::<_, 6>(simd, ms, seq_store, rep, block),
        7.. => compress_block_ext_dict_mls::<_, 7>(simd, ms, seq_store, rep, block),
        _ => compress_block_ext_dict_mls::<_, 4>(simd, ms, seq_store, rep, block),
    })
}

/// `ZSTD_compressBlock_doubleFast_extDict_generic` for one minimum match length,
/// compiled for one SIMD level.
#[simd]
fn compress_block_ext_dict_mls<S: Simd, const MLS: u32>(
    simd: S,
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    let cparams = ms.cparams;
    let h_bits_l = cparams.hash_log;
    let h_bits_s = cparams.chain_log;
    let window = block.window;
    let prefix = window.prefix;
    let ext = window.ext;
    let low_limit = ms.lowest_match_index(block.end, cparams.window_log);
    let dict_start_index = low_limit;
    let dict_limit = ms.window.dict_limit;
    let prefix_start_index = dict_limit.max(low_limit);
    let mut offset_1 = rep[0];
    let mut offset_2 = rep[1];

    if prefix_start_index == dict_start_index {
        return compress_block(ms, seq_store, rep, block);
    }

    // Prefix positions are offsets into `src`, whose first byte has window
    // index `base` (the reference's `base` pointer); candidates below the
    // prefix are read from the external segment by window index.
    let base = prefix.start();
    let src = prefix.slice(base, block.end);
    let istart = (block.start - base) as usize;
    let iend = src.len();
    // A block shorter than eight bytes never enters the search.
    let ilimit = iend.saturating_sub(8);
    let index = |p: usize| p as u32 + base;
    // Hashes are below the table sizes; the masks prove the indices in
    // bounds once the tables are known to be non-empty.
    assert!(!ms.hash_table.is_empty() && !ms.chain_table.is_empty());
    let (mask_l, mask_s) = (ms.hash_table.len() - 1, ms.chain_table.len() - 1);
    let hash_l = |p: usize| hash_word::<8>(load64(src, p), h_bits_l) & mask_l;
    let hash_s = |p: usize| hash_word::<MLS>(load64(src, p), h_bits_s) & mask_s;
    let read32 = |p: usize| load32(src, p);
    let read64 = |p: usize| load64(src, p);
    let in_dict = |idx: u32| idx < prefix_start_index;
    let any32 = |idx: u32| {
        if in_dict(idx) {
            ext.read32(idx)
        } else {
            load32(src, (idx - base) as usize)
        }
    };
    let any64 = |idx: u32| {
        if in_dict(idx) {
            ext.read64(idx)
        } else {
            load64(src, (idx - base) as usize)
        }
    };
    let any_byte = |idx: u32| {
        if in_dict(idx) {
            ext.byte(idx)
        } else {
            src[(idx - base) as usize]
        }
    };
    // Forward match length of offset `ip` against window index `m`, across
    // the segment boundary.
    let count_any = |ip: usize, m: u32| {
        if in_dict(m) {
            window.count_2segments(
                index(ip),
                m,
                block.end,
                prefix_start_index,
                prefix_start_index,
            )
        } else {
            count_slices_simd(simd, &src[ip..], &src[(m - base) as usize..])
        }
    };
    // Backward extension of a match at window index `m` for offset `ip`.
    let catch_up = |ip: &mut usize, m: &mut u32, m_length: &mut usize, anchor: usize, low: u32| {
        while *ip > anchor && *m > low && src[*ip - 1] == any_byte(*m - 1) {
            *ip -= 1;
            *m -= 1;
            *m_length += 1;
        }
    };

    let MatchState {
        hash_table: hash_long,
        chain_table: hash_small,
        ..
    } = ms;
    let hash_long = hash_long.as_mut_slice();
    let hash_small = hash_small.as_mut_slice();

    let mut ip = istart;
    let mut anchor = istart;
    while ip < ilimit {
        let h_small = hash_s(ip);
        let match_index = hash_small[h_small];
        let h_long = hash_l(ip);
        let match_long_index = hash_long[h_long];
        let curr = index(ip);
        let rep_index = (curr + 1).wrapping_sub(offset_1);
        hash_small[h_small] = curr;
        hash_long[h_long] = curr;

        let m_length;
        let overlap_ok = prefix_start_index.wrapping_sub(1).wrapping_sub(rep_index) >= 3;
        if overlap_ok
            && offset_1 <= curr + 1 - dict_start_index
            && any32(rep_index) == read32(ip + 1)
        {
            m_length = count_any(ip + 1 + 4, rep_index + 4) + 4;
            ip += 1;
            seq_store.store_seq_from(src, anchor, ip - anchor, repcode_to_offbase(1), m_length);
        } else if match_long_index > dict_start_index && any64(match_long_index) == read64(ip) {
            let low = if in_dict(match_long_index) {
                dict_start_index
            } else {
                prefix_start_index
            };
            let mut ml = count_any(ip + 8, match_long_index + 8) + 8;
            let offset = curr - match_long_index;
            let mut m = match_long_index;
            catch_up(&mut ip, &mut m, &mut ml, anchor, low);
            offset_2 = offset_1;
            offset_1 = offset;
            seq_store.store_seq_from(src, anchor, ip - anchor, offset_to_offbase(offset), ml);
            m_length = ml;
        } else if match_index > dict_start_index && any32(match_index) == read32(ip) {
            let h3 = hash_l(ip + 1);
            let match_index3 = hash_long[h3];
            hash_long[h3] = curr + 1;
            let offset;
            let mut ml;
            if match_index3 > dict_start_index && any64(match_index3) == read64(ip + 1) {
                let low = if in_dict(match_index3) {
                    dict_start_index
                } else {
                    prefix_start_index
                };
                ml = count_any(ip + 9, match_index3 + 8) + 8;
                ip += 1;
                offset = curr + 1 - match_index3;
                let mut m = match_index3;
                catch_up(&mut ip, &mut m, &mut ml, anchor, low);
            } else {
                let low = if in_dict(match_index) {
                    dict_start_index
                } else {
                    prefix_start_index
                };
                ml = count_any(ip + 4, match_index + 4) + 4;
                offset = curr - match_index;
                let mut m = match_index;
                catch_up(&mut ip, &mut m, &mut ml, anchor, low);
            }
            offset_2 = offset_1;
            offset_1 = offset;
            seq_store.store_seq_from(src, anchor, ip - anchor, offset_to_offbase(offset), ml);
            m_length = ml;
        } else {
            ip += ((ip - anchor) >> SEARCH_STRENGTH) + 1;
            continue;
        }

        ip += m_length;
        anchor = ip;
        if ip <= ilimit {
            let index_to_insert = curr + 2;
            let insert_at = (index_to_insert - base) as usize;
            hash_long[hash_l(insert_at)] = index_to_insert;
            hash_long[hash_l(ip - 2)] = index(ip - 2);
            hash_small[hash_s(insert_at)] = index_to_insert;
            hash_small[hash_s(ip - 1)] = index(ip - 1);

            while ip <= ilimit {
                let current2 = index(ip);
                let rep_index2 = current2.wrapping_sub(offset_2);
                let overlap_ok = prefix_start_index.wrapping_sub(1).wrapping_sub(rep_index2) >= 3;
                if overlap_ok
                    && offset_2 <= current2 - dict_start_index
                    && any32(rep_index2) == read32(ip)
                {
                    let rep_length2 = count_any(ip + 4, rep_index2 + 4) + 4;
                    core::mem::swap(&mut offset_1, &mut offset_2);
                    seq_store.store_seq_only(0, repcode_to_offbase(1), rep_length2);
                    hash_small[hash_s(ip)] = current2;
                    hash_long[hash_l(ip)] = current2;
                    ip += rep_length2;
                    anchor = ip;
                    continue;
                }
                break;
            }
        }
    }

    rep[0] = offset_1;
    rep[1] = offset_2;
    iend - anchor
}
