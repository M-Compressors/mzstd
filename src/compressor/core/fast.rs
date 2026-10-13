//! The `fast` strategy (`zstd_fast.c`): one hash table, positions probed in
//! an interleaved pipeline with a step that grows over incompressible data.
//!
//! Positions are window indices held in `i64` so the reference's pointer
//! comparisons against `iend - 8` stay meaningful for blocks shorter than
//! eight bytes.

use super::match_state::{BlockInput, MatchState};
use super::mem::hash_ptr;
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

/// `ZSTD_compressBlock_fast`.
pub(crate) fn compress_block(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    let mls = ms.cparams.min_match.clamp(4, 7);
    let cparams = ms.cparams;
    let hlog = cparams.hash_log;
    let step_size = cparams.target_length as i64 + i64::from(cparams.target_length == 0) + 1;
    let prefix = block.window.prefix;
    let istart = i64::from(block.start);
    let iend = i64::from(block.end);
    let end_index = block.end;
    let prefix_start_index = ms.lowest_prefix_index(end_index, cparams.window_log);
    let prefix_start = i64::from(prefix_start_index);
    let ilimit = iend - i64::from(HASH_READ_SIZE);
    let hash = |p: i64| hash_ptr(&prefix, p as u32, hlog, mls);
    let read32 = |p: i64| prefix.read32(p as u32);
    let k_step_incr: i64 = 1 << (SEARCH_STRENGTH - 1);

    let mut anchor = istart;
    let mut ip0 = istart;
    let mut rep_offset1 = rep[0];
    let mut rep_offset2 = rep[1];
    let mut offset_saved1 = 0u32;
    let mut offset_saved2 = 0u32;

    ip0 += i64::from(ip0 == prefix_start);
    {
        let curr = ip0 as u32;
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

    let table = &mut ms.hash_table;
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
        let mut match_idx = table[hash0];
        let mut current0;

        // Which way the search found a match.
        enum Found {
            Rep,
            Offset,
        }
        let found = loop {
            let rval = read32(ip2 - i64::from(rep_offset1));
            current0 = ip0 as u32;
            table[hash0] = current0;
            if read32(ip2) == rval && rep_offset1 > 0 {
                ip0 = ip2;
                table[hash1] = ip1 as u32;
                break Found::Rep;
            }
            if match_idx >= prefix_start_index && read32(ip0) == prefix.read32(match_idx) {
                table[hash1] = ip1 as u32;
                break Found::Offset;
            }

            match_idx = table[hash1];
            hash0 = hash1;
            hash1 = hash(ip2);
            ip0 = ip1;
            ip1 = ip2;
            ip2 = ip3;

            current0 = ip0 as u32;
            table[hash0] = current0;
            if match_idx >= prefix_start_index && read32(ip0) == prefix.read32(match_idx) {
                if step <= 4 {
                    table[hash1] = ip1 as u32;
                }
                break Found::Offset;
            }

            match_idx = table[hash1];
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

        let (offcode, mut m_length, match0);
        match found {
            Found::Rep => {
                let mut m0 = ip0 - i64::from(rep_offset1);
                let back = i64::from(prefix.byte((ip0 - 1) as u32) == prefix.byte((m0 - 1) as u32));
                ip0 -= back;
                m0 -= back;
                match0 = m0;
                offcode = repcode_to_offbase(1);
                m_length = back + 4;
            }
            Found::Offset => {
                let mut m0 = i64::from(match_idx);
                rep_offset2 = rep_offset1;
                rep_offset1 = (ip0 - m0) as u32;
                offcode = offset_to_offbase(rep_offset1);
                m_length = 4;
                while ip0 > anchor
                    && m0 > prefix_start
                    && prefix.byte((ip0 - 1) as u32) == prefix.byte((m0 - 1) as u32)
                {
                    ip0 -= 1;
                    m0 -= 1;
                    m_length += 1;
                }
                match0 = m0;
            }
        }

        // _match:
        m_length += prefix.count(
            (ip0 + m_length) as u32,
            (match0 + m_length) as u32,
            iend as u32,
        ) as i64;
        seq_store.store_seq(
            prefix.slice(anchor as u32, ip0 as u32),
            offcode,
            m_length as usize,
        );
        ip0 += m_length;
        anchor = ip0;

        if ip0 <= ilimit {
            table[hash(i64::from(current0) + 2)] = current0 + 2;
            table[hash(ip0 - 2)] = (ip0 - 2) as u32;
            if rep_offset2 > 0 {
                while ip0 <= ilimit && read32(ip0) == read32(ip0 - i64::from(rep_offset2)) {
                    let r_length = prefix.count(
                        (ip0 + 4) as u32,
                        (ip0 + 4 - i64::from(rep_offset2)) as u32,
                        iend as u32,
                    ) + 4;
                    core::mem::swap(&mut rep_offset1, &mut rep_offset2);
                    table[hash(ip0)] = ip0 as u32;
                    ip0 += r_length as i64;
                    seq_store.store_seq(&[], repcode_to_offbase(1), r_length);
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
    (iend - anchor) as usize
}

/// `ZSTD_compressBlock_fast_extDict`.
pub(crate) fn compress_block_ext_dict(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    let mls = ms.cparams.min_match.clamp(4, 7);
    let cparams = ms.cparams;
    let hlog = cparams.hash_log;
    let step_size = cparams.target_length as i64 + i64::from(cparams.target_length == 0) + 1;
    let window = block.window;
    let prefix = window.prefix;
    let ext = window.ext;
    let istart = i64::from(block.start);
    let iend = i64::from(block.end);
    let end_index = block.end;
    let low_limit = ms.lowest_match_index(end_index, cparams.window_log);
    let dict_start_index = low_limit;
    let dict_limit = ms.window.dict_limit;
    let prefix_start_index = dict_limit.max(low_limit);
    let ilimit = iend - 8;
    let mut offset_1 = rep[0];
    let mut offset_2 = rep[1];
    let mut offset_saved1 = 0u32;
    let mut offset_saved2 = 0u32;

    if prefix_start_index == dict_start_index {
        return compress_block(ms, seq_store, rep, block);
    }

    let hash = |p: i64| hash_ptr(&prefix, p as u32, hlog, mls);
    let read32 = |p: i64| prefix.read32(p as u32);
    // Reads 4 bytes at an index in either segment.
    let read_any = |idx: u32| {
        if idx < prefix_start_index {
            ext.read32(idx)
        } else {
            prefix.read32(idx)
        }
    };
    let byte_any = |idx: u32| {
        if idx < prefix_start_index {
            ext.byte(idx)
        } else {
            prefix.byte(idx)
        }
    };
    let k_step_incr: i64 = 1 << (SEARCH_STRENGTH - 1);

    let mut ip0 = istart;
    let mut anchor = istart;
    {
        let curr = ip0 as u32;
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

    let table = &mut ms.hash_table;
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
        let mut idx = table[hash0];
        let mut current0;

        enum Found {
            Rep(u32),
            Offset,
        }
        let found = loop {
            {
                let current2 = ip2 as u32;
                let rep_index = current2.wrapping_sub(offset_1);
                let rval = if prefix_start_index.wrapping_sub(rep_index) >= 4 && offset_1 > 0 {
                    read_any(rep_index)
                } else {
                    read32(ip2) ^ 1
                };
                current0 = ip0 as u32;
                table[hash0] = current0;
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
            idx = table[hash1];
            hash0 = hash1;
            hash1 = hash(ip2);
            ip0 = ip1;
            ip1 = ip2;
            ip2 = ip3;
            current0 = ip0 as u32;
            table[hash0] = current0;
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
            idx = table[hash1];
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

        let (offcode, mut m_length, mut match0): (u32, i64, u32);
        match found {
            Found::Rep(rep_index) => {
                let back = i64::from(prefix.byte((ip0 - 1) as u32) == byte_any(rep_index - 1));
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
                while ip0 > anchor
                    && match0 > low_match
                    && prefix.byte((ip0 - 1) as u32) == byte_any(match0 - 1)
                {
                    ip0 -= 1;
                    match0 -= 1;
                    m_length += 1;
                }
            }
        }

        // _match:
        let ip_from = (ip0 + m_length) as u32;
        let m_from = match0 + m_length as u32;
        m_length += if match0 < prefix_start_index {
            window.count_2segments(
                ip_from,
                m_from,
                iend as u32,
                prefix_start_index,
                prefix_start_index,
            )
        } else {
            prefix.count(ip_from, m_from, iend as u32)
        } as i64;
        seq_store.store_seq(
            prefix.slice(anchor as u32, ip0 as u32),
            offcode,
            m_length as usize,
        );
        ip0 += m_length;
        anchor = ip0;

        if ip1 < ip0 {
            table[hash1] = ip1 as u32;
        }

        if ip0 <= ilimit {
            table[hash(i64::from(current0) + 2)] = current0 + 2;
            table[hash(ip0 - 2)] = (ip0 - 2) as u32;
            while ip0 <= ilimit {
                let rep_index2 = (ip0 as u32).wrapping_sub(offset_2);
                let overlap_ok = prefix_start_index.wrapping_sub(1).wrapping_sub(rep_index2) >= 3;
                if overlap_ok && offset_2 > 0 && read_any(rep_index2) == read32(ip0) {
                    let rep_length2 = if rep_index2 < prefix_start_index {
                        window.count_2segments(
                            (ip0 + 4) as u32,
                            rep_index2 + 4,
                            iend as u32,
                            prefix_start_index,
                            prefix_start_index,
                        )
                    } else {
                        prefix.count((ip0 + 4) as u32, rep_index2 + 4, iend as u32)
                    } + 4;
                    core::mem::swap(&mut offset_1, &mut offset_2);
                    seq_store.store_seq(&[], repcode_to_offbase(1), rep_length2);
                    table[hash(ip0)] = ip0 as u32;
                    ip0 += rep_length2 as i64;
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
    (iend - anchor) as usize
}
