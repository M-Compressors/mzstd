//! The `dfast` strategy (`zstd_double_fast.c`): a long (8-byte) hash table
//! in `hash_table` and a short one in `chain_table`.

use super::match_state::{BlockInput, MatchState};
use super::mem::hash_ptr;
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

/// `ZSTD_compressBlock_doubleFast`.
pub(crate) fn compress_block(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    let cparams = ms.cparams;
    let mls = cparams.min_match.clamp(4, 7);
    let h_bits_l = cparams.hash_log;
    let h_bits_s = cparams.chain_log;
    let prefix = block.window.prefix;
    let istart = i64::from(block.start);
    let iend = i64::from(block.end);
    let mut anchor = istart;
    let prefix_lowest_index = ms.lowest_prefix_index(block.end, cparams.window_log);
    let prefix_lowest = i64::from(prefix_lowest_index);
    let ilimit = iend - i64::from(HASH_READ_SIZE);
    let mut offset_1 = rep[0];
    let mut offset_2 = rep[1];
    let mut offset_saved1 = 0u32;
    let mut offset_saved2 = 0u32;
    let k_step_incr: i64 = 1 << SEARCH_STRENGTH;

    let hash_l = |p: i64| hash_ptr(&prefix, p as u32, h_bits_l, 8);
    let hash_s = |p: i64| hash_ptr(&prefix, p as u32, h_bits_s, mls);
    let read32 = |p: i64| prefix.read32(p as u32);
    let read64 = |p: i64| prefix.read64(p as u32);
    let byte = |p: i64| prefix.byte(p as u32);
    let count = |a: i64, b: i64| prefix.count(a as u32, b as u32, iend as u32) as i64;

    let mut ip = istart;
    ip += i64::from(ip == prefix_lowest);
    {
        let current = ip as u32;
        let window_low = ms.lowest_prefix_index(current, cparams.window_log);
        let max_rep = current - window_low;
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

    'outer: loop {
        let mut step: i64 = 1;
        let mut next_step = ip + k_step_incr;
        let mut ip1 = ip + step;
        if ip1 > ilimit {
            break 'outer;
        }
        let mut hl0 = hash_l(ip);
        let mut idxl0 = hash_long[hl0];
        let mut curr: u32;
        let mut hl1: usize = 0;
        let mut idxl1: u32 = 0;

        enum Next {
            Stored(i64),
            Found { offset: u32, m_length: i64 },
            SearchNextLong { idxs0: u32 },
        }
        let next = loop {
            let hs0 = hash_s(ip);
            let idxs0 = hash_small[hs0];
            curr = ip as u32;
            hash_long[hl0] = curr;
            hash_small[hs0] = curr;

            if offset_1 > 0 && read32(ip + 1 - i64::from(offset_1)) == read32(ip + 1) {
                let m_length = count(ip + 1 + 4, ip + 1 + 4 - i64::from(offset_1)) + 4;
                ip += 1;
                seq_store.store_seq(
                    prefix.slice(anchor as u32, ip as u32),
                    repcode_to_offbase(1),
                    m_length as usize,
                );
                break Next::Stored(m_length);
            }

            hl1 = hash_l(ip1);
            if idxl0 >= prefix_lowest_index && read64(i64::from(idxl0)) == read64(ip) {
                let mut m_length = count(ip + 8, i64::from(idxl0) + 8) + 8;
                let offset = (ip - i64::from(idxl0)) as u32;
                let mut matchl0 = i64::from(idxl0);
                while ip > anchor && matchl0 > prefix_lowest && byte(ip - 1) == byte(matchl0 - 1) {
                    ip -= 1;
                    matchl0 -= 1;
                    m_length += 1;
                }
                break Next::Found { offset, m_length };
            }

            idxl1 = hash_long[hl1];
            if idxs0 >= prefix_lowest_index && read32(i64::from(idxs0)) == read32(ip) {
                break Next::SearchNextLong { idxs0 };
            }

            if ip1 >= next_step {
                step += 1;
                next_step += k_step_incr;
            }
            ip = ip1;
            ip1 += step;
            hl0 = hl1;
            idxl0 = idxl1;
            if ip1 > ilimit {
                break 'outer;
            }
        };

        let m_length = match next {
            Next::Stored(m_length) => m_length,
            Next::Found { offset, m_length } => {
                offset_2 = offset_1;
                offset_1 = offset;
                if step < 4 {
                    hash_long[hl1] = ip1 as u32;
                }
                seq_store.store_seq(
                    prefix.slice(anchor as u32, ip as u32),
                    offset_to_offbase(offset),
                    m_length as usize,
                );
                m_length
            }
            Next::SearchNextLong { idxs0 } => {
                let mut matchs0 = i64::from(idxs0);
                let mut m_length = count(ip + 4, matchs0 + 4) + 4;
                let mut offset = (ip - matchs0) as u32;
                if idxl1 > prefix_lowest_index && read64(i64::from(idxl1)) == read64(ip1) {
                    let l1len = count(ip1 + 8, i64::from(idxl1) + 8) + 8;
                    if l1len > m_length {
                        ip = ip1;
                        m_length = l1len;
                        offset = (ip - i64::from(idxl1)) as u32;
                        matchs0 = i64::from(idxl1);
                    }
                }
                while ip > anchor && matchs0 > prefix_lowest && byte(ip - 1) == byte(matchs0 - 1) {
                    ip -= 1;
                    matchs0 -= 1;
                    m_length += 1;
                }
                offset_2 = offset_1;
                offset_1 = offset;
                if step < 4 {
                    hash_long[hl1] = ip1 as u32;
                }
                seq_store.store_seq(
                    prefix.slice(anchor as u32, ip as u32),
                    offset_to_offbase(offset),
                    m_length as usize,
                );
                m_length
            }
        };

        // _match_stored:
        ip += m_length;
        anchor = ip;
        if ip <= ilimit {
            let index_to_insert = curr + 2;
            hash_long[hash_l(i64::from(index_to_insert))] = index_to_insert;
            hash_long[hash_l(ip - 2)] = (ip - 2) as u32;
            hash_small[hash_s(i64::from(index_to_insert))] = index_to_insert;
            hash_small[hash_s(ip - 1)] = (ip - 1) as u32;

            while ip <= ilimit && offset_2 > 0 && read32(ip) == read32(ip - i64::from(offset_2)) {
                let r_length = count(ip + 4, ip + 4 - i64::from(offset_2)) + 4;
                core::mem::swap(&mut offset_1, &mut offset_2);
                hash_small[hash_s(ip)] = ip as u32;
                hash_long[hash_l(ip)] = ip as u32;
                seq_store.store_seq(&[], repcode_to_offbase(1), r_length as usize);
                ip += r_length;
                anchor = ip;
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

/// `ZSTD_compressBlock_doubleFast_extDict`.
pub(crate) fn compress_block_ext_dict(
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    block: &BlockInput<'_>,
) -> usize {
    let cparams = ms.cparams;
    let mls = cparams.min_match.clamp(4, 7);
    let h_bits_l = cparams.hash_log;
    let h_bits_s = cparams.chain_log;
    let window = block.window;
    let prefix = window.prefix;
    let ext = window.ext;
    let istart = i64::from(block.start);
    let iend = i64::from(block.end);
    let ilimit = iend - 8;
    let low_limit = ms.lowest_match_index(block.end, cparams.window_log);
    let dict_start_index = low_limit;
    let dict_limit = ms.window.dict_limit;
    let prefix_start_index = dict_limit.max(low_limit);
    let mut offset_1 = rep[0];
    let mut offset_2 = rep[1];

    if prefix_start_index == dict_start_index {
        return compress_block(ms, seq_store, rep, block);
    }

    let hash_l = |p: i64| hash_ptr(&prefix, p as u32, h_bits_l, 8);
    let hash_s = |p: i64| hash_ptr(&prefix, p as u32, h_bits_s, mls);
    let read32 = |p: i64| prefix.read32(p as u32);
    let read64 = |p: i64| prefix.read64(p as u32);
    let in_dict = |idx: u32| idx < prefix_start_index;
    let any32 = |idx: u32| {
        if in_dict(idx) {
            ext.read32(idx)
        } else {
            prefix.read32(idx)
        }
    };
    let any64 = |idx: u32| {
        if in_dict(idx) {
            ext.read64(idx)
        } else {
            prefix.read64(idx)
        }
    };
    let any_byte = |idx: u32| {
        if in_dict(idx) {
            ext.byte(idx)
        } else {
            prefix.byte(idx)
        }
    };
    // Forward match length of `ip` against `m`, across the segment boundary.
    let count_any = |ip: i64, m: u32| {
        if in_dict(m) {
            window.count_2segments(
                ip as u32,
                m,
                iend as u32,
                prefix_start_index,
                prefix_start_index,
            )
        } else {
            prefix.count(ip as u32, m, iend as u32)
        }
    };
    // Backward extension of a match at `m_start` for `ip`.
    let catch_up = |ip: &mut i64, m: &mut u32, m_length: &mut usize, anchor: i64, low: u32| {
        while *ip > anchor && *m > low && prefix.byte((*ip - 1) as u32) == any_byte(*m - 1) {
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

    let mut ip = istart;
    let mut anchor = istart;
    while ip < ilimit {
        let h_small = hash_s(ip);
        let match_index = hash_small[h_small];
        let h_long = hash_l(ip);
        let match_long_index = hash_long[h_long];
        let curr = ip as u32;
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
            seq_store.store_seq(
                prefix.slice(anchor as u32, ip as u32),
                repcode_to_offbase(1),
                m_length,
            );
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
            seq_store.store_seq(
                prefix.slice(anchor as u32, ip as u32),
                offset_to_offbase(offset),
                ml,
            );
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
            seq_store.store_seq(
                prefix.slice(anchor as u32, ip as u32),
                offset_to_offbase(offset),
                ml,
            );
            m_length = ml;
        } else {
            ip += ((ip - anchor) >> SEARCH_STRENGTH) + 1;
            continue;
        }

        ip += m_length as i64;
        anchor = ip;
        if ip <= ilimit {
            let index_to_insert = curr + 2;
            hash_long[hash_l(i64::from(index_to_insert))] = index_to_insert;
            hash_long[hash_l(ip - 2)] = (ip - 2) as u32;
            hash_small[hash_s(i64::from(index_to_insert))] = index_to_insert;
            hash_small[hash_s(ip - 1)] = (ip - 1) as u32;

            while ip <= ilimit {
                let current2 = ip as u32;
                let rep_index2 = current2.wrapping_sub(offset_2);
                let overlap_ok = prefix_start_index.wrapping_sub(1).wrapping_sub(rep_index2) >= 3;
                if overlap_ok
                    && offset_2 <= current2 - dict_start_index
                    && any32(rep_index2) == read32(ip)
                {
                    let rep_length2 = count_any(ip + 4, rep_index2 + 4) + 4;
                    core::mem::swap(&mut offset_1, &mut offset_2);
                    seq_store.store_seq(&[], repcode_to_offbase(1), rep_length2);
                    hash_small[hash_s(ip)] = current2;
                    hash_long[hash_l(ip)] = current2;
                    ip += rep_length2 as i64;
                    anchor = ip;
                    continue;
                }
                break;
            }
        }
    }

    rep[0] = offset_1;
    rep[1] = offset_2;
    (iend - anchor) as usize
}
