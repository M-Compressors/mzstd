//! The `fast` strategy (`zstd_fast.c`): one hash table, positions probed in
//! an interleaved pipeline with a step that grows over incompressible data.
//!
//! Positions are window indices held in `i64` so the reference's pointer
//! comparisons against `iend - 8` stay meaningful for blocks shorter than
//! eight bytes.

use fearless_simd::Simd;
use fearless_simd_macros::simd;

use super::match_state::{BlockInput, MatchState};
use super::mem::{count_slices, hash_at, hash_ptr, hash_word, load32, window_word, window9};
use super::seq_store::{SeqStore, offset_to_offbase, repcode_to_offbase};
use super::window::HASH_READ_SIZE;

/// `kSearchStrength`.
const SEARCH_STRENGTH: u32 = 8;

/// `ZSTD_fillHashTableForCCtx` with `ZSTD_dtlm_fast`: indexes every third
/// position from `next_to_update` up to eight bytes before `end`.
pub(crate) fn fill_hash_table(ms: &mut MatchState, prefix: &super::mem::Mem<'_>, end: u32) {
    let h_bits = ms.cparams.hash_log;
    let mls = ms.cparams.min_match;
    let iend = i64::from(end) - i64::from(HASH_READ_SIZE);
    let mut ip = i64::from(ms.next_to_update);
    while ip + 3 < iend + 2 {
        let curr = ip as u32;
        ms.hash_table[hash_ptr(prefix, curr, h_bits, mls)] = curr;
        ip += 3;
    }
}

/// `ZSTD_compressBlock_fast`: selects the variant compiled for the
/// minimum match length, as the reference's `ZSTD_GEN_FAST_FN` templates do.
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

/// `ZSTD_compressBlock_fast_noDict_generic` for one minimum match length,
/// compiled for one SIMD level.
#[simd]
fn compress_block_mls<S: Simd, const MLS: u32>(
    simd: S,
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    let cparams = ms.cparams;
    let hlog = cparams.hash_log;
    let step_size = cparams.target_length as usize + usize::from(cparams.target_length == 0) + 1;
    // Positions are offsets into `src`, whose first byte has window index
    // `base`: the reference's pointers, with `base` its `base` pointer.
    let prefix = block.window.prefix;
    let base = prefix.start();
    let src = prefix.slice(base, block.end);
    let istart = (block.start - base) as usize;
    let iend = src.len();
    let prefix_start_index = ms.lowest_prefix_index(block.end, cparams.window_log);
    let prefix_start = (prefix_start_index - base) as usize;
    // A block shorter than HASH_READ_SIZE never enters the search: the
    // first `ip3 >= ilimit` test fails for any `ilimit <= 0`.
    let ilimit = iend.saturating_sub(HASH_READ_SIZE as usize);
    let index = |p: usize| p as u32 + base;
    let read32 = |p: usize| load32(src, p);
    let k_step_incr: usize = 1 << (SEARCH_STRENGTH - 1);

    let mut anchor = istart;
    let mut ip0 = istart;
    let mut rep_offset1 = rep[0];
    let mut rep_offset2 = rep[1];
    let mut offset_saved1 = 0u32;
    let mut offset_saved2 = 0u32;

    ip0 += usize::from(ip0 == prefix_start);
    {
        let curr = index(ip0);
        let window_low = ms.lowest_prefix_index(curr, cparams.window_log);
        let max_rep = curr - window_low;
        if rep_offset2 > max_rep {
            offset_saved2 = rep_offset2;
            rep_offset2 = 0;
        }
        if rep_offset1 > max_rep {
            offset_saved1 = rep_offset1;
            rep_offset1 = 0;
        }
    }

    let table = ms.hash_table.as_mut_slice();
    // Hashes are below the table size (`1 << hash_log`); masking with
    // `len - 1` changes nothing and, with the table known to be non-empty,
    // proves every index in bounds.
    assert!(!table.is_empty());
    let mask = table.len() - 1;
    let hash = |p: usize| hash_at::<MLS>(src, p, hlog) & mask;
    'start: loop {
        // The reference keeps `ip0..ip3`; at every probe `ip1` is `ip0 + 1`
        // and `ip3` is `ip2 + 1`, so only `ip0` and `ip2` are kept here,
        // which leaves the loop's registers to the values it needs.
        let mut step = step_size;
        let mut next_step = ip0 + k_step_incr;
        let mut ip2 = ip0 + step;
        if ip2 + 1 >= ilimit {
            break 'start;
        }
        // Every probe reads inside the nine bytes at `ip0` or at `ip2`
        // (`ip2 + 1 < ilimit` keeps both whole); `ip2`'s window becomes the
        // next `ip0`'s, so each step bound-checks one window.
        let hash_w = |word: u64| hash_word::<MLS>(word, hlog) & mask;
        let mut w0 = window9(src, ip0);
        let mut w2 = window9(src, ip2);
        let mut hash0 = hash_w(window_word::<0>(w0));
        let mut hash1 = hash_w(window_word::<1>(w0));
        let mut match_idx = table[hash0 & mask];
        let mut current0;

        // Which way the search found a match.
        enum Found {
            Rep,
            Offset,
        }
        let found = loop {
            // Probe `ip0` (and the repcode at `ip2`); `ip1` is `ip0 + 1`.
            let rval = read32(ip2 - rep_offset1 as usize);
            current0 = index(ip0);
            table[hash0 & mask] = current0;
            if window_word::<0>(w2) as u32 == rval && rep_offset1 > 0 {
                table[hash1 & mask] = index(ip0 + 1);
                ip0 = ip2;
                break Found::Rep;
            }
            if match_idx >= prefix_start_index
                && window_word::<0>(w0) as u32 == read32((match_idx - base) as usize)
            {
                table[hash1 & mask] = index(ip0 + 1);
                break Found::Offset;
            }

            match_idx = table[hash1 & mask];
            hash0 = hash1;
            hash1 = hash_w(window_word::<0>(w2));
            ip0 += 1;

            // Probe `ip0` (the old `ip1`); `ip1` is now the old `ip2`.
            current0 = index(ip0);
            table[hash0 & mask] = current0;
            if match_idx >= prefix_start_index
                && window_word::<1>(w0) as u32 == read32((match_idx - base) as usize)
            {
                if step <= 4 {
                    table[hash1 & mask] = index(ip2);
                }
                break Found::Offset;
            }

            match_idx = table[hash1 & mask];
            hash0 = hash1;
            hash1 = hash_w(window_word::<1>(w2));
            ip0 = ip2;
            w0 = w2;
            ip2 = ip0 + step;
            if ip2 >= next_step {
                step += 1;
                next_step += k_step_incr;
            }
            if ip2 + 1 >= ilimit {
                break 'start;
            }
            w2 = window9(src, ip2);
        };

        let (offcode, mut m_length, match0);
        match found {
            Found::Rep => {
                let mut m0 = ip0 - rep_offset1 as usize;
                let back = usize::from(src[ip0 - 1] == src[m0 - 1]);
                ip0 -= back;
                m0 -= back;
                match0 = m0;
                offcode = repcode_to_offbase(1);
                m_length = back + 4;
            }
            Found::Offset => {
                let mut m0 = (match_idx - base) as usize;
                rep_offset2 = rep_offset1;
                rep_offset1 = (ip0 - m0) as u32;
                offcode = offset_to_offbase(rep_offset1);
                m_length = 4;
                while ip0 > anchor && m0 > prefix_start && src[ip0 - 1] == src[m0 - 1] {
                    ip0 -= 1;
                    m0 -= 1;
                    m_length += 1;
                }
                match0 = m0;
            }
        }

        // _match:
        m_length += count_slices(&src[ip0 + m_length..], &src[match0 + m_length..]);
        seq_store.store_seq_from(src, anchor, ip0 - anchor, offcode, m_length);
        ip0 += m_length;
        anchor = ip0;

        if ip0 <= ilimit {
            let current2 = (current0 - base) as usize + 2;
            table[hash(current2)] = current0 + 2;
            table[hash(ip0 - 2)] = index(ip0 - 2);
            if rep_offset2 > 0 {
                while ip0 <= ilimit && read32(ip0) == read32(ip0 - rep_offset2 as usize) {
                    let r_length =
                        count_slices(&src[ip0 + 4..], &src[ip0 + 4 - rep_offset2 as usize..]) + 4;
                    core::mem::swap(&mut rep_offset1, &mut rep_offset2);
                    table[hash(ip0)] = index(ip0);
                    ip0 += r_length;
                    seq_store.store_seq_only(0, repcode_to_offbase(1), r_length);
                    anchor = ip0;
                }
            }
        }
    }

    offset_saved2 = if offset_saved1 != 0 && rep_offset1 != 0 {
        offset_saved1
    } else {
        offset_saved2
    };
    rep[0] = if rep_offset1 != 0 {
        rep_offset1
    } else {
        offset_saved1
    };
    rep[1] = if rep_offset2 != 0 {
        rep_offset2
    } else {
        offset_saved2
    };
    iend - anchor
}

/// `ZSTD_compressBlock_fast_extDict`: selects the variant compiled for
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

/// `ZSTD_compressBlock_fast_extDict_generic` for one minimum match length,
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
    let hlog = cparams.hash_log;
    let step_size = cparams.target_length as usize + usize::from(cparams.target_length == 0) + 1;
    let window = block.window;
    let prefix = window.prefix;
    let ext = window.ext;
    let end_index = block.end;
    let low_limit = ms.lowest_match_index(end_index, cparams.window_log);
    let dict_start_index = low_limit;
    let dict_limit = ms.window.dict_limit;
    let prefix_start_index = dict_limit.max(low_limit);
    let mut offset_1 = rep[0];
    let mut offset_2 = rep[1];
    let mut offset_saved1 = 0u32;
    let mut offset_saved2 = 0u32;

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
    // A block shorter than eight bytes never enters the search: the first
    // `ip3 >= ilimit` test fails for any `ilimit <= 0`.
    let ilimit = iend.saturating_sub(8);
    let index = |p: usize| p as u32 + base;
    // Hashes are below the table size; the mask proves the indices in
    // bounds once the table is known to be non-empty.
    assert!(!ms.hash_table.is_empty());
    let mask = ms.hash_table.len() - 1;
    let hash = |p: usize| hash_at::<MLS>(src, p, hlog) & mask;
    let read32 = |p: usize| load32(src, p);
    // Reads 4 bytes at a window index in either segment.
    let read_any = |idx: u32| {
        if idx < prefix_start_index {
            ext.read32(idx)
        } else {
            load32(src, (idx - base) as usize)
        }
    };
    let byte_any = |idx: u32| {
        if idx < prefix_start_index {
            ext.byte(idx)
        } else {
            src[(idx - base) as usize]
        }
    };
    // Forward match length of offset `ip` against window index `m`, across
    // the segment boundary when the match started (at `start`) in the
    // external segment.
    let count_any = |ip: usize, m: u32, start: u32| {
        if start < prefix_start_index {
            window.count_2segments(
                index(ip),
                m,
                end_index,
                prefix_start_index,
                prefix_start_index,
            )
        } else {
            count_slices(&src[ip..], &src[(m - base) as usize..])
        }
    };
    let k_step_incr: usize = 1 << (SEARCH_STRENGTH - 1);

    let mut ip0 = istart;
    let mut anchor = istart;
    {
        let curr = index(ip0);
        let max_rep = curr - dict_start_index;
        if offset_2 >= max_rep {
            offset_saved2 = offset_2;
            offset_2 = 0;
        }
        if offset_1 >= max_rep {
            offset_saved1 = offset_1;
            offset_1 = 0;
        }
    }

    let table = ms.hash_table.as_mut_slice();
    'start: loop {
        let mut step = step_size;
        let mut next_step = ip0 + k_step_incr;
        let mut ip1 = ip0 + 1;
        let mut ip2 = ip0 + step;
        let mut ip3 = ip2 + 1;
        if ip3 >= ilimit {
            break 'start;
        }
        let mut hash0 = hash(ip0);
        let mut hash1 = hash(ip1);
        let mut idx = table[hash0 & mask];
        let mut current0;

        enum Found {
            Rep(u32),
            Offset,
        }
        let found = loop {
            {
                let rep_index = index(ip2).wrapping_sub(offset_1);
                let rval = if prefix_start_index.wrapping_sub(rep_index) >= 4 && offset_1 > 0 {
                    read_any(rep_index)
                } else {
                    read32(ip2) ^ 1
                };
                current0 = index(ip0);
                table[hash0 & mask] = current0;
                if read32(ip2) == rval {
                    ip0 = ip2;
                    break Found::Rep(rep_index);
                }
            }
            {
                let mval = if idx >= dict_start_index {
                    read_any(idx)
                } else {
                    read32(ip0) ^ 1
                };
                if read32(ip0) == mval {
                    break Found::Offset;
                }
            }
            idx = table[hash1 & mask];
            hash0 = hash1;
            hash1 = hash(ip2);
            ip0 = ip1;
            ip1 = ip2;
            ip2 = ip3;
            current0 = index(ip0);
            table[hash0 & mask] = current0;
            {
                let mval = if idx >= dict_start_index {
                    read_any(idx)
                } else {
                    read32(ip0) ^ 1
                };
                if read32(ip0) == mval {
                    break Found::Offset;
                }
            }
            idx = table[hash1 & mask];
            hash0 = hash1;
            hash1 = hash(ip2);
            ip0 = ip1;
            ip1 = ip2;
            ip2 = ip0 + step;
            ip3 = ip1 + step;
            if ip2 >= next_step {
                step += 1;
                next_step += k_step_incr;
            }
            if ip3 >= ilimit {
                break 'start;
            }
        };

        let (offcode, mut m_length, mut match0): (u32, usize, u32);
        match found {
            Found::Rep(rep_index) => {
                let back = usize::from(src[ip0 - 1] == byte_any(rep_index - 1));
                ip0 -= back;
                match0 = rep_index - back as u32;
                offcode = repcode_to_offbase(1);
                m_length = back + 4;
            }
            Found::Offset => {
                let offset = current0 - idx;
                let low_match = if idx < prefix_start_index {
                    dict_start_index
                } else {
                    prefix_start_index
                };
                match0 = idx;
                offset_2 = offset_1;
                offset_1 = offset;
                offcode = offset_to_offbase(offset);
                m_length = 4;
                while ip0 > anchor && match0 > low_match && src[ip0 - 1] == byte_any(match0 - 1) {
                    ip0 -= 1;
                    match0 -= 1;
                    m_length += 1;
                }
            }
        }

        // _match:
        m_length += count_any(ip0 + m_length, match0 + m_length as u32, match0);
        seq_store.store_seq_from(src, anchor, ip0 - anchor, offcode, m_length);
        ip0 += m_length;
        anchor = ip0;

        if ip1 < ip0 {
            table[hash1 & mask] = index(ip1);
        }

        if ip0 <= ilimit {
            table[hash((current0 - base) as usize + 2)] = current0 + 2;
            table[hash(ip0 - 2)] = index(ip0 - 2);
            while ip0 <= ilimit {
                let rep_index2 = index(ip0).wrapping_sub(offset_2);
                let overlap_ok = prefix_start_index.wrapping_sub(1).wrapping_sub(rep_index2) >= 3;
                if overlap_ok && offset_2 > 0 && read_any(rep_index2) == read32(ip0) {
                    let rep_length2 = count_any(ip0 + 4, rep_index2 + 4, rep_index2) + 4;
                    core::mem::swap(&mut offset_1, &mut offset_2);
                    seq_store.store_seq_only(0, repcode_to_offbase(1), rep_length2);
                    table[hash(ip0)] = index(ip0);
                    ip0 += rep_length2;
                    anchor = ip0;
                    continue;
                }
                break;
            }
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
    iend - anchor
}
