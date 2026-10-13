//! Long distance matching (`zstd_ldm.c`).
//!
//! A gear rolling hash picks split points; each split's preceding
//! `min_match_length` bytes are hashed with XXH64 into a bucketed table.
//! Matches found there become raw sequences that the regular match finders
//! fill in around (`block_compress`), or that the optimal parser considers
//! as extra candidates.

use alloc::vec::Vec;

use super::cctx::LdmParams;
use super::double_fast;
use super::fast;
use super::ldm_geartab::GEAR_TAB;
use super::match_state::{BlockCompressor, BlockInput, MatchState};
use super::params::Strategy;
use super::seq_store::{SeqStore, offset_to_offbase};
use super::window::{HASH_READ_SIZE, Segments, WindowState};
use crate::shared::xxhash::Xxh64;

/// `LDM_BATCH_SIZE`.
const BATCH_SIZE: usize = 64;

/// A long match found by the LDM (`rawSeq`).
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RawSeq {
    pub(crate) offset: u32,
    pub(crate) lit_length: u32,
    pub(crate) match_length: u32,
}

/// Sequences and a read position (`RawSeqStore_t`).
#[derive(Clone, Debug, Default)]
pub(crate) struct RawSeqStore {
    pub(crate) seq: Vec<RawSeq>,
    pub(crate) pos: usize,
    pub(crate) pos_in_sequence: usize,
    pub(crate) capacity: usize,
}

impl RawSeqStore {
    /// Number of stored sequences.
    pub(crate) fn size(&self) -> usize {
        self.seq.len()
    }

    /// `ZSTD_ldm_skipRawSeqStoreBytes` (and its `optLdm` twin).
    pub(crate) fn skip_bytes_opt(&mut self, nb_bytes: usize) {
        let mut curr_pos = (self.pos_in_sequence + nb_bytes) as u32;
        while curr_pos != 0 && self.pos < self.size() {
            let curr = self.seq[self.pos];
            if curr_pos >= curr.lit_length + curr.match_length {
                curr_pos -= curr.lit_length + curr.match_length;
                self.pos += 1;
            } else {
                self.pos_in_sequence = curr_pos as usize;
                break;
            }
        }
        if curr_pos == 0 || self.pos == self.size() {
            self.pos_in_sequence = 0;
        }
    }

    /// `ZSTD_ldm_skipSequences`.
    fn skip_sequences(&mut self, mut src_size: usize, min_match: u32) {
        while src_size > 0 && self.pos < self.size() {
            let seq = &mut self.seq[self.pos];
            if src_size <= seq.lit_length as usize {
                seq.lit_length -= src_size as u32;
                return;
            }
            src_size -= seq.lit_length as usize;
            seq.lit_length = 0;
            if src_size < seq.match_length as usize {
                seq.match_length -= src_size as u32;
                if seq.match_length < min_match {
                    let carried = seq.match_length;
                    if self.pos + 1 < self.size() {
                        self.seq[self.pos + 1].lit_length += carried;
                    }
                    self.pos += 1;
                }
                return;
            }
            src_size -= seq.match_length as usize;
            seq.match_length = 0;
            self.pos += 1;
        }
    }

    /// `maybeSplitSequence`.
    fn maybe_split_sequence(&mut self, remaining: u32, min_match: u32) -> RawSeq {
        let mut sequence = self.seq[self.pos];
        if remaining >= sequence.lit_length + sequence.match_length {
            self.pos += 1;
            return sequence;
        }
        if remaining <= sequence.lit_length {
            sequence.offset = 0;
        } else if remaining < sequence.lit_length + sequence.match_length {
            sequence.match_length = remaining - sequence.lit_length;
            if sequence.match_length < min_match {
                sequence.offset = 0;
            }
        }
        self.skip_sequences(remaining as usize, min_match);
        sequence
    }
}

/// One entry of the LDM hash table (`ldmEntry_t`).
#[derive(Copy, Clone, Debug, Default)]
struct Entry {
    offset: u32,
    checksum: u32,
}

/// Long distance matcher state (`ldmState_t`).
#[derive(Debug, Default)]
pub(crate) struct LdmState {
    pub(crate) window: WindowState,
    hash_table: Vec<Entry>,
    loaded_dict_end: u32,
    bucket_offsets: Vec<u8>,
}

/// `ldmRollingHashState_t`.
#[derive(Copy, Clone, Debug)]
struct Gear {
    rolling: u64,
    stop_mask: u64,
}

impl Gear {
    /// `ZSTD_ldm_gear_init`.
    fn new(params: &LdmParams) -> Self {
        let max_bits_in_mask = params.min_match_length.min(64);
        let hash_rate_log = params.hash_rate_log;
        let stop_mask = if hash_rate_log > 0 && hash_rate_log <= max_bits_in_mask {
            ((1u64 << hash_rate_log) - 1) << (max_bits_in_mask - hash_rate_log)
        } else {
            (1u64 << hash_rate_log) - 1
        };
        Self {
            rolling: u64::from(u32::MAX),
            stop_mask,
        }
    }

    /// `ZSTD_ldm_gear_reset`.
    fn reset(&mut self, data: &[u8]) {
        let mut hash = self.rolling;
        for &byte in data {
            hash = (hash << 1).wrapping_add(GEAR_TAB[usize::from(byte)]);
        }
        // The reference forgets the fed bytes: the state is left unchanged.
        let _ = hash;
    }

    /// `ZSTD_ldm_gear_feed`: records split points into `splits`, stopping
    /// when the batch is full; returns how many bytes were consumed.
    fn feed(&mut self, data: &[u8], splits: &mut Vec<usize>) -> usize {
        let mut hash = self.rolling;
        let mask = self.stop_mask;
        let mut n = 0;
        while n < data.len() {
            hash = (hash << 1).wrapping_add(GEAR_TAB[usize::from(data[n])]);
            n += 1;
            if hash & mask == 0 {
                splits.push(n);
                if splits.len() == BATCH_SIZE {
                    break;
                }
            }
        }
        self.rolling = hash;
        n
    }
}

fn xxh64(data: &[u8]) -> u64 {
    let mut hasher = Xxh64::new(0);
    hasher.update(data);
    hasher.digest()
}

impl LdmState {
    /// Resets the matcher for a frame (`ZSTD_resetCCtx_internal`'s LDM part).
    pub(crate) fn reset(&mut self, params: &LdmParams) {
        self.hash_table.clear();
        self.hash_table
            .resize(1usize << params.hash_log, Entry::default());
        self.bucket_offsets.clear();
        self.bucket_offsets
            .resize(1usize << (params.hash_log - params.bucket_size_log), 0);
        self.window = WindowState::new();
        self.loaded_dict_end = 0;
    }

    /// Bytes held by the hash table and bucket offsets.
    pub(crate) fn retained_bytes(&self) -> usize {
        self.hash_table.capacity() * core::mem::size_of::<Entry>() + self.bucket_offsets.capacity()
    }

    /// `ZSTD_ldm_insertEntry`.
    fn insert_entry(&mut self, hash: usize, entry: Entry, bucket_size_log: u32) {
        let offset = self.bucket_offsets[hash];
        self.hash_table[(hash << bucket_size_log) + usize::from(offset)] = entry;
        self.bucket_offsets[hash] = ((u32::from(offset) + 1) & ((1 << bucket_size_log) - 1)) as u8;
    }

    /// `ZSTD_ldm_generateSequences_internal` over `[start, end)` (window
    /// indices of the LDM window). Returns the trailing literal count.
    fn generate_internal(
        &mut self,
        segments: &Segments<'_>,
        store: &mut RawSeqStore,
        params: &LdmParams,
        start: u32,
        end: u32,
    ) -> Result<usize, ()> {
        let ext_dict = self.window.has_ext_dict();
        let min_match_length = params.min_match_length;
        let ents_per_bucket = 1usize << params.bucket_size_log;
        let h_bits = params.hash_log - params.bucket_size_log;
        let dict_limit = self.window.dict_limit;
        let lowest_index = if ext_dict {
            self.window.low_limit
        } else {
            dict_limit
        };
        let view = self.window.view(segments, end);
        let prefix = view.prefix;
        let istart = i64::from(start);
        let iend = i64::from(end);
        let ilimit = iend - i64::from(HASH_READ_SIZE);
        let mut anchor = istart;
        let mut ip = istart;
        let mut splits = Vec::with_capacity(BATCH_SIZE);

        if ((end - start) as usize) < min_match_length as usize {
            return Ok((iend - anchor) as usize);
        }
        let mut gear = Gear::new(params);
        gear.reset(prefix.slice(start, start + min_match_length));
        ip += i64::from(min_match_length);

        while ip < ilimit {
            splits.clear();
            let hashed = gear.feed(prefix.slice(ip as u32, ilimit as u32), &mut splits) as i64;
            let mut candidates = [(0u32, 0usize, 0u32); BATCH_SIZE];
            for (n, &split_at) in splits.iter().enumerate() {
                let split = (ip + split_at as i64 - i64::from(min_match_length)) as u32;
                let xxhash = xxh64(prefix.slice(split, split + min_match_length));
                let hash = (xxhash & ((1u64 << h_bits) - 1)) as usize;
                candidates[n] = (split, hash, (xxhash >> 32) as u32);
            }
            for &(split, hash, checksum) in &candidates[..splits.len()] {
                let new_entry = Entry {
                    offset: split,
                    checksum,
                };
                if i64::from(split) < anchor {
                    self.insert_entry(hash, new_entry, params.bucket_size_log);
                    continue;
                }
                // (total, forward, backward, offset) of the first longest
                // candidate.
                let mut best: Option<(usize, usize, usize, u32)> = None;
                let bucket = hash << params.bucket_size_log;
                for slot in bucket..bucket + ents_per_bucket {
                    let cur = self.hash_table[slot];
                    if cur.checksum != checksum || cur.offset <= lowest_index {
                        continue;
                    }
                    let forward = view.count_virtual(split, cur.offset, end);
                    if forward < min_match_length as usize {
                        continue;
                    }
                    // Backwards, a match may run from the prefix into the
                    // external dictionary; the split stays in the prefix.
                    let low = if ext_dict { lowest_index } else { dict_limit };
                    let mut backward = 0usize;
                    while i64::from(split) - backward as i64 > anchor
                        && cur.offset - backward as u32 > low
                        && prefix.byte(split - backward as u32 - 1)
                            == view.byte_at(cur.offset - backward as u32 - 1)
                    {
                        backward += 1;
                    }
                    let total = forward + backward;
                    if best.is_none_or(|(b, ..)| total > b) {
                        best = Some((total, forward, backward, cur.offset));
                    }
                }
                let Some((_, forward, backward, best_offset)) = best else {
                    self.insert_entry(hash, new_entry, params.bucket_size_log);
                    continue;
                };
                let offset = split - best_offset;
                let m_length = forward + backward;
                if store.size() == store.capacity {
                    return Err(());
                }
                store.seq.push(RawSeq {
                    lit_length: (i64::from(split) - backward as i64 - anchor) as u32,
                    match_length: m_length as u32,
                    offset,
                });
                self.insert_entry(hash, new_entry, params.bucket_size_log);
                anchor = i64::from(split) + forward as i64;
                if anchor > ip + hashed {
                    gear.reset(
                        prefix.slice((anchor - i64::from(min_match_length)) as u32, anchor as u32),
                    );
                    ip = anchor - hashed;
                    break;
                }
            }
            ip += hashed;
        }
        Ok((iend - anchor) as usize)
    }
}

impl LdmState {
    /// `ZSTD_ldm_generateSequences`: finds long matches in `[start, end)`
    /// of the LDM window, a mebibyte at a time.
    #[cfg_attr(all(feature = "hotpath", not(feature = "no_std")), hotpath::measure)]
    pub(crate) fn generate_sequences(
        &mut self,
        segments: &Segments<'_>,
        store: &mut RawSeqStore,
        params: &LdmParams,
        mut start: u32,
        mut end: u32,
    ) {
        const MAX_CHUNK_SIZE: u32 = 1 << 20;
        let max_dist = 1u32 << params.window_log;
        let src_size = end - start;
        let nb_chunks =
            src_size / MAX_CHUNK_SIZE + u32::from(!src_size.is_multiple_of(MAX_CHUNK_SIZE));
        let mut leftover_size = 0u32;
        let mut chunk = 0;
        while chunk < nb_chunks && store.size() < store.capacity {
            let mut chunk_start = start + chunk * MAX_CHUNK_SIZE;
            let mut chunk_end = (chunk_start + MAX_CHUNK_SIZE).min(end);
            let chunk_size = chunk_end - chunk_start;
            let prev_size = store.size();
            if self.window.need_overflow_correction(
                0,
                max_dist,
                self.loaded_dict_end,
                chunk_start,
                chunk_end,
            ) {
                let correction = self.window.correct_overflow(0, max_dist, chunk_start);
                for entry in &mut self.hash_table {
                    entry.offset = entry.offset.saturating_sub(correction);
                }
                self.loaded_dict_end = 0;
                chunk_start -= correction;
                chunk_end -= correction;
                start -= correction;
                end -= correction;
            }
            self.window
                .enforce_max_dist(chunk_end, max_dist, &mut self.loaded_dict_end);
            let Ok(new_leftover) =
                self.generate_internal(segments, store, params, chunk_start, chunk_end)
            else {
                return;
            };
            if prev_size < store.size() {
                store.seq[prev_size].lit_length += leftover_size;
                leftover_size = new_leftover as u32;
            } else {
                leftover_size += chunk_size;
            }
            chunk += 1;
        }
    }
}

/// `ZSTD_ldm_fillFastTables`.
fn fill_fast_tables(ms: &mut MatchState, block: &BlockInput<'_>, end: u32) {
    match ms.cparams.strategy {
        Strategy::Fast => fast::fill_hash_table(ms, &block.window.prefix, end),
        Strategy::DFast => double_fast::fill_double_hash_table(ms, &block.window.prefix, end),
        _ => {}
    }
}

/// `ZSTD_ldm_limitTableUpdate`.
fn limit_table_update(ms: &mut MatchState, anchor: u32) {
    let curr = anchor;
    if curr > ms.next_to_update + 1024 {
        ms.next_to_update = curr - 512.min(curr - ms.next_to_update - 1024);
    }
}

/// `ZSTD_ldm_blockCompress`: runs `compressor` around the long matches in
/// `store`, or hands them to the optimal parser as candidates.
pub(crate) fn block_compress(
    store: &mut RawSeqStore,
    ms: &mut MatchState,
    seq_store: &mut SeqStore,
    rep: &mut [u32; 3],
    compressor: BlockCompressor,
    block: &BlockInput<'_>,
) -> usize {
    let min_match = ms.cparams.min_match;
    let src_size = (block.end - block.start) as usize;
    if ms.cparams.strategy >= Strategy::BtOpt {
        ms.ldm_seq_store = Some(store.clone());
        let last_ll_size = compressor(ms, seq_store, rep, block);
        ms.ldm_seq_store = None;
        store.skip_bytes_opt(src_size);
        return last_ll_size;
    }

    let prefix = block.window.prefix;
    let mut ip = block.start;
    while store.pos < store.size() && ip < block.end {
        let sequence = store.maybe_split_sequence(block.end - ip, min_match);
        if sequence.offset == 0 {
            break;
        }
        limit_table_update(ms, ip);
        fill_fast_tables(ms, block, ip);
        let sub = BlockInput {
            window: block.window,
            start: ip,
            end: ip + sequence.lit_length,
        };
        let new_lit_length = compressor(ms, seq_store, rep, &sub) as u32;
        ip += sequence.lit_length;
        rep[2] = rep[1];
        rep[1] = rep[0];
        rep[0] = sequence.offset;
        seq_store.store_seq(
            prefix.slice(ip - new_lit_length, ip),
            offset_to_offbase(sequence.offset),
            sequence.match_length as usize,
        );
        ip += sequence.match_length;
    }
    limit_table_update(ms, ip);
    fill_fast_tables(ms, block, ip);
    let tail = BlockInput {
        window: block.window,
        start: ip,
        end: block.end,
    };
    compressor(ms, seq_store, rep, &tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(offset: u32, lit_length: u32, match_length: u32) -> RawSeq {
        RawSeq {
            offset,
            lit_length,
            match_length,
        }
    }

    fn store(seqs: &[RawSeq]) -> RawSeqStore {
        RawSeqStore {
            seq: seqs.to_vec(),
            ..RawSeqStore::default()
        }
    }

    // Generated sequences never cross the block they were generated for, so
    // compression never splits one; these pin `maybeSplitSequence` and
    // `ZSTD_ldm_skipSequences` against the reference's semantics directly.

    #[test]
    fn a_sequence_inside_the_block_is_taken_whole() {
        let mut store = store(&[seq(100, 5, 20), seq(7, 1, 10)]);
        assert_eq!(store.maybe_split_sequence(25, 4), seq(100, 5, 20));
        assert_eq!(store.pos, 1);
    }

    #[test]
    fn a_match_cut_by_the_block_end_keeps_its_head_and_carries_its_tail() {
        let mut store = store(&[seq(100, 5, 20), seq(7, 1, 10)]);
        assert_eq!(store.maybe_split_sequence(15, 4), seq(100, 5, 10));
        // The next block starts with the match's remaining ten bytes.
        assert_eq!((store.pos, store.seq[0]), (0, seq(100, 0, 10)));
    }

    #[test]
    fn a_cut_leaving_too_short_a_match_becomes_literals() {
        let mut store = store(&[seq(100, 5, 20), seq(7, 1, 10)]);
        // Two match bytes fit: too short, so the block sees only literals.
        assert_eq!(store.maybe_split_sequence(7, 4).offset, 0);
        // Eighteen match bytes remain for the next block.
        assert_eq!((store.pos, store.seq[0]), (0, seq(100, 0, 18)));
    }

    #[test]
    fn a_block_ending_inside_the_literals_skips_only_literals() {
        let mut store = store(&[seq(100, 5, 20)]);
        assert_eq!(store.maybe_split_sequence(3, 4).offset, 0);
        assert_eq!(store.seq[0], seq(100, 2, 20));
    }

    #[test]
    fn a_remainder_shorter_than_the_minimum_match_joins_the_next_literals() {
        let mut store = store(&[seq(100, 5, 20), seq(7, 1, 10)]);
        store.skip_sequences(23, 4);
        assert_eq!(store.pos, 1);
        assert_eq!(store.seq[1], seq(7, 3, 10));
        // Skipping past everything empties the store.
        store.skip_sequences(1000, 4);
        assert_eq!(store.pos, 2);
    }
}
