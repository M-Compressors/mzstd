//! The sequences section of a compressed block
//! (`zstd_compress_sequences.c` and `ZSTD_buildSequencesStatistics`).

use super::bits::BitWriter;
use super::fse::{self, CState, CTable};
use super::hist;
use super::{FseRepeat, FseTables};
use crate::compressor::core::error::CoreError;
use crate::compressor::core::params::Strategy;
use crate::compressor::core::seq_store::SeqStore;
use crate::shared::format::{
    DEFAULT_MAX_OFF, EncodingType, LL_BITS, LL_DEFAULT_NORM, LL_DEFAULT_NORM_LOG, LL_FSE_LOG,
    MAX_LL, MAX_ML, MAX_OFF, MAX_SEQ, ML_BITS, ML_DEFAULT_NORM, ML_DEFAULT_NORM_LOG, ML_FSE_LOG,
    OF_DEFAULT_NORM, OF_DEFAULT_NORM_LOG, OFF_FSE_LOG,
};

/// `-log2(x / 256)` in 1/256 bits for `x` in `0..256`; `0` for `x == 0`.
const INVERSE_PROBABILITY_LOG256: [u32; 256] = [
    0, 2048, 1792, 1642, 1536, 1453, 1386, 1329, 1280, 1236, 1197, 1162, 1130, 1100, 1073, 1047,
    1024, 1001, 980, 960, 941, 923, 906, 889, 874, 859, 844, 830, 817, 804, 791, 779, 768, 756,
    745, 734, 724, 714, 704, 694, 685, 676, 667, 658, 650, 642, 633, 626, 618, 610, 603, 595, 588,
    581, 574, 567, 561, 554, 548, 542, 535, 529, 523, 517, 512, 506, 500, 495, 489, 484, 478, 473,
    468, 463, 458, 453, 448, 443, 438, 434, 429, 424, 420, 415, 411, 407, 402, 398, 394, 390, 386,
    382, 377, 373, 370, 366, 362, 358, 354, 350, 347, 343, 339, 336, 332, 329, 325, 322, 318, 315,
    311, 308, 305, 302, 298, 295, 292, 289, 286, 282, 279, 276, 273, 270, 267, 264, 261, 258, 256,
    253, 250, 247, 244, 241, 239, 236, 233, 230, 228, 225, 222, 220, 217, 215, 212, 209, 207, 204,
    202, 199, 197, 194, 192, 190, 187, 185, 182, 180, 178, 175, 173, 171, 168, 166, 164, 162, 159,
    157, 155, 153, 151, 149, 146, 144, 142, 140, 138, 136, 134, 132, 130, 128, 126, 123, 121, 119,
    117, 115, 114, 112, 110, 108, 106, 104, 102, 100, 98, 96, 94, 93, 91, 89, 87, 85, 83, 82, 80,
    78, 76, 74, 73, 71, 69, 67, 66, 64, 62, 61, 59, 57, 55, 54, 52, 50, 49, 47, 46, 44, 42, 41, 39,
    37, 36, 34, 33, 31, 30, 28, 26, 25, 23, 22, 20, 19, 17, 16, 14, 13, 11, 10, 8, 7, 5, 4, 2, 1,
];

/// The reference's `ERROR(GENERIC)` as a cost: larger than any real cost.
const COST_ERROR: usize = usize::MAX;

/// Static description of one sequence code alphabet.
#[derive(Copy, Clone, Debug)]
pub(crate) struct CodeKind {
    /// Accuracy log of a described table.
    pub(crate) fse_log: u32,
    /// Predefined distribution.
    pub(crate) default_norm: &'static [i16],
    /// Accuracy log of the predefined distribution.
    pub(crate) default_norm_log: u32,
    /// Largest code of the predefined distribution.
    pub(crate) default_max: u32,
    /// Largest code.
    pub(crate) max_code: u32,
}

/// Literal lengths.
pub(crate) const LITERAL_LENGTHS: CodeKind = CodeKind {
    fse_log: LL_FSE_LOG,
    default_norm: &LL_DEFAULT_NORM,
    default_norm_log: LL_DEFAULT_NORM_LOG,
    default_max: MAX_LL,
    max_code: MAX_LL,
};
/// Offset codes.
pub(crate) const OFFSETS: CodeKind = CodeKind {
    fse_log: OFF_FSE_LOG,
    default_norm: &OF_DEFAULT_NORM,
    default_norm_log: OF_DEFAULT_NORM_LOG,
    default_max: DEFAULT_MAX_OFF,
    max_code: MAX_OFF,
};
/// Match lengths.
pub(crate) const MATCH_LENGTHS: CodeKind = CodeKind {
    fse_log: ML_FSE_LOG,
    default_norm: &ML_DEFAULT_NORM,
    default_norm_log: ML_DEFAULT_NORM_LOG,
    default_max: MAX_ML,
    max_code: MAX_ML,
};

/// `ZSTD_useLowProbCount`.
const fn use_low_prob_count(nb_seq: usize) -> bool {
    nb_seq >= 2048
}

/// Header cost in bytes of describing `count` (`ZSTD_NCountCost`).
fn ncount_cost(count: &[u32], max: u32, nb_seq: usize, fse_log: u32) -> Result<usize, CoreError> {
    let mut wksp = [0u8; 512];
    let mut norm = [0i16; MAX_SEQ as usize + 1];
    let table_log = fse::optimal_table_log(fse_log, nb_seq, max);
    fse::normalize_count(
        &mut norm,
        table_log,
        count,
        nb_seq,
        max,
        use_low_prob_count(nb_seq),
    )?;
    fse::write_ncount(&mut wksp, &norm, max, table_log)
}

/// Entropy bound of `count` in bits (`ZSTD_entropyCost`).
fn entropy_cost(count: &[u32], max: u32, total: usize) -> usize {
    let mut cost: u32 = 0;
    for &c in &count[..=max as usize] {
        let mut norm = ((256 * c as usize) / total) as u32;
        if c != 0 && norm == 0 {
            norm = 1;
        }
        cost = cost.wrapping_add(c.wrapping_mul(INVERSE_PROBABILITY_LOG256[norm as usize]));
    }
    (cost >> 8) as usize
}

/// Cost in bits of `count` with an existing table (`ZSTD_fseBitCost`), or
/// [`COST_ERROR`] when the table cannot encode some present symbol.
pub(crate) fn fse_bit_cost(table: &CTable, count: &[u32], max: u32) -> usize {
    const ACCURACY_LOG: u32 = 8;
    if table.max_symbol_value < max {
        return COST_ERROR;
    }
    let table_log = table.table_log;
    let bad_cost = (table_log + 1) << ACCURACY_LOG;
    let mut cost = 0usize;
    for (s, &c) in count[..=max as usize].iter().enumerate() {
        let bit_cost = table.bit_cost(s as u32, ACCURACY_LOG);
        if c == 0 {
            continue;
        }
        if bit_cost >= bad_cost {
            return COST_ERROR;
        }
        cost += c as usize * bit_cost as usize;
    }
    cost >> ACCURACY_LOG
}

/// Cost in bits of `count` under the distribution `norm`
/// (`ZSTD_crossEntropyCost`).
pub(crate) fn cross_entropy_cost(
    norm: &[i16],
    accuracy_log: u32,
    count: &[u32],
    max: u32,
) -> usize {
    let shift = 8 - accuracy_log;
    let mut cost = 0usize;
    for s in 0..=max as usize {
        let norm_acc = if norm[s] != -1 { norm[s] as u32 } else { 1 };
        let norm256 = norm_acc << shift;
        cost += count[s] as usize * INVERSE_PROBABILITY_LOG256[norm256 as usize] as usize;
    }
    cost >> 8
}

/// Histogram of one code stream.
#[derive(Copy, Clone, Debug)]
pub(crate) struct CodeStats<'a> {
    pub(crate) count: &'a [u32],
    pub(crate) max: u32,
    pub(crate) most_frequent: usize,
    pub(crate) nb_seq: usize,
}

/// A code stream with its mutable histogram.
#[derive(Debug)]
struct Histogram<'a> {
    count: &'a mut [u32],
    max: u32,
    codes: &'a [u8],
}

/// Chooses how to encode one code stream (`ZSTD_selectEncodingType`).
pub(crate) fn select_encoding_type(
    repeat_mode: &mut FseRepeat,
    stats: CodeStats<'_>,
    kind: CodeKind,
    prev_table: &CTable,
    default_allowed: bool,
    strategy: Strategy,
) -> EncodingType {
    let CodeStats {
        count,
        max,
        most_frequent,
        nb_seq,
    } = stats;
    if most_frequent == nb_seq {
        *repeat_mode = FseRepeat::None;
        if default_allowed && nb_seq <= 2 {
            return EncodingType::Basic;
        }
        return EncodingType::Rle;
    }
    if strategy < Strategy::Lazy {
        if default_allowed {
            const STATIC_FSE_NB_SEQ_MAX: usize = 1000;
            let mult = 10 - strategy as usize;
            const BASE_LOG: usize = 3;
            let dynamic_fse_nb_seq_min = ((1usize << kind.default_norm_log) * mult) >> BASE_LOG;
            if *repeat_mode == FseRepeat::Valid && nb_seq < STATIC_FSE_NB_SEQ_MAX {
                return EncodingType::Repeat;
            }
            if nb_seq < dynamic_fse_nb_seq_min
                || most_frequent < (nb_seq >> (kind.default_norm_log - 1))
            {
                *repeat_mode = FseRepeat::None;
                return EncodingType::Basic;
            }
        }
    } else {
        let basic_cost = if default_allowed {
            cross_entropy_cost(kind.default_norm, kind.default_norm_log, count, max)
        } else {
            COST_ERROR
        };
        let repeat_cost = if *repeat_mode != FseRepeat::None {
            fse_bit_cost(prev_table, count, max)
        } else {
            COST_ERROR
        };
        let ncount = ncount_cost(count, max, nb_seq, kind.fse_log).unwrap_or(COST_ERROR);
        let compressed_cost = (ncount << 3).wrapping_add(entropy_cost(count, max, nb_seq));
        if basic_cost <= repeat_cost && basic_cost <= compressed_cost {
            *repeat_mode = FseRepeat::None;
            return EncodingType::Basic;
        }
        if repeat_cost <= compressed_cost {
            return EncodingType::Repeat;
        }
    }
    *repeat_mode = FseRepeat::Check;
    EncodingType::Compressed
}

/// Builds the table for the chosen encoding and writes its description
/// (`ZSTD_buildCTable`).
///
/// # Errors
///
/// Header writing and normalization failures, and
/// [`CoreError::DstSizeTooSmall`] when an RLE symbol does not fit.
fn build_ctable(
    dst: &mut [u8],
    next: &mut CTable,
    prev: &CTable,
    kind: CodeKind,
    encoding: EncodingType,
    histogram: Histogram<'_>,
) -> Result<usize, CoreError> {
    let Histogram { count, max, codes } = histogram;
    match encoding {
        EncodingType::Rle => {
            next.build_rle(max as u8);
            let Some(first) = dst.first_mut() else {
                return Err(CoreError::DstSizeTooSmall);
            };
            *first = codes[0];
            Ok(1)
        }
        EncodingType::Repeat => {
            next.clone_from(prev);
            Ok(0)
        }
        EncodingType::Basic => {
            next.build(kind.default_norm, kind.default_max, kind.default_norm_log);
            Ok(0)
        }
        EncodingType::Compressed => {
            let nb_seq = codes.len();
            let mut nb_seq_1 = nb_seq;
            let table_log = fse::optimal_table_log(kind.fse_log, nb_seq, max);
            let last = usize::from(codes[nb_seq - 1]);
            if count[last] > 1 {
                count[last] -= 1;
                nb_seq_1 -= 1;
            }
            let mut norm = [0i16; MAX_SEQ as usize + 1];
            fse::normalize_count(
                &mut norm,
                table_log,
                count,
                nb_seq_1,
                max,
                use_low_prob_count(nb_seq_1),
            )?;
            let ncount_size = fse::write_ncount(dst, &norm, max, table_log)?;
            next.build(&norm, max, table_log);
            Ok(ncount_size)
        }
    }
}

/// Encoding choices of a block's three code streams
/// (`ZSTD_symbolEncodingTypeStats_t`).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct EncodingStats {
    pub(crate) ll_type: EncodingType,
    pub(crate) of_type: EncodingType,
    pub(crate) ml_type: EncodingType,
    /// Bytes of table descriptions written.
    pub(crate) size: usize,
    /// Size of the last described table, for the 1.3.4 decoder workaround.
    pub(crate) last_count_size: usize,
}

/// Chooses encodings for the three code streams, builds their tables into
/// `next` and writes their descriptions (`ZSTD_buildSequencesStatistics`).
///
/// The store must hold at least one sequence; its codes are (re)computed.
///
/// # Errors
///
/// Whatever table construction reports.
pub(crate) fn build_statistics(
    store: &mut SeqStore,
    prev: &FseTables,
    next: &mut FseTables,
    dst: &mut [u8],
    strategy: Strategy,
) -> Result<EncodingStats, CoreError> {
    let nb_seq = store.nb_seq();
    debug_assert!(nb_seq != 0);
    store.fill_codes();
    let mut op = 0usize;
    let mut last_count_size = 0usize;
    let mut count = [0u32; MAX_SEQ as usize + 1];

    // Literal lengths.
    let mut max = MAX_LL;
    let most_frequent = hist::count(&mut count, &mut max, &store.ll_code, false)? as usize;
    next.litlength_repeat = prev.litlength_repeat;
    let ll_type = select_encoding_type(
        &mut next.litlength_repeat,
        CodeStats {
            count: &count,
            max,
            most_frequent,
            nb_seq,
        },
        LITERAL_LENGTHS,
        &prev.litlength,
        true,
        strategy,
    );
    let size = build_ctable(
        &mut dst[op..],
        &mut next.litlength,
        &prev.litlength,
        LITERAL_LENGTHS,
        ll_type,
        Histogram {
            count: &mut count,
            max,
            codes: &store.ll_code,
        },
    )?;
    if ll_type == EncodingType::Compressed {
        last_count_size = size;
    }
    op += size;

    // Offsets.
    let mut max = MAX_OFF;
    let most_frequent = hist::count(&mut count, &mut max, &store.of_code, false)? as usize;
    let default_allowed = max <= DEFAULT_MAX_OFF;
    next.offcode_repeat = prev.offcode_repeat;
    let of_type = select_encoding_type(
        &mut next.offcode_repeat,
        CodeStats {
            count: &count,
            max,
            most_frequent,
            nb_seq,
        },
        OFFSETS,
        &prev.offcode,
        default_allowed,
        strategy,
    );
    let size = build_ctable(
        &mut dst[op..],
        &mut next.offcode,
        &prev.offcode,
        OFFSETS,
        of_type,
        Histogram {
            count: &mut count,
            max,
            codes: &store.of_code,
        },
    )?;
    if of_type == EncodingType::Compressed {
        last_count_size = size;
    }
    op += size;

    // Match lengths.
    let mut max = MAX_ML;
    let most_frequent = hist::count(&mut count, &mut max, &store.ml_code, false)? as usize;
    next.matchlength_repeat = prev.matchlength_repeat;
    let ml_type = select_encoding_type(
        &mut next.matchlength_repeat,
        CodeStats {
            count: &count,
            max,
            most_frequent,
            nb_seq,
        },
        MATCH_LENGTHS,
        &prev.matchlength,
        true,
        strategy,
    );
    let size = build_ctable(
        &mut dst[op..],
        &mut next.matchlength,
        &prev.matchlength,
        MATCH_LENGTHS,
        ml_type,
        Histogram {
            count: &mut count,
            max,
            codes: &store.ml_code,
        },
    )?;
    if ml_type == EncodingType::Compressed {
        last_count_size = size;
    }
    op += size;

    Ok(EncodingStats {
        ll_type,
        of_type,
        ml_type,
        size: op,
        last_count_size,
    })
}

/// Writes the sequences bitstream (`ZSTD_encodeSequences`).
///
/// # Errors
///
/// [`CoreError::DstSizeTooSmall`] when the stream does not fit.
pub(crate) fn encode(
    dst: &mut [u8],
    tables: &FseTables,
    store: &SeqStore,
) -> Result<usize, CoreError> {
    let Some(mut writer) = BitWriter::new(dst) else {
        return Err(CoreError::DstSizeTooSmall);
    };
    let n = store.nb_seq();
    let seqs = &store.sequences;
    let (ll, of, ml) = (&store.ll_code, &store.of_code, &store.ml_code);
    let last = n - 1;

    let mut state_ml = CState::with_first_symbol(&tables.matchlength, u32::from(ml[last]));
    let mut state_of = CState::with_first_symbol(&tables.offcode, u32::from(of[last]));
    let mut state_ll = CState::with_first_symbol(&tables.litlength, u32::from(ll[last]));
    writer.add_bits(
        u64::from(seqs[last].lit_length),
        u32::from(LL_BITS[usize::from(ll[last])]),
    );
    writer.add_bits(
        u64::from(seqs[last].ml_base),
        u32::from(ML_BITS[usize::from(ml[last])]),
    );
    writer.add_bits(u64::from(seqs[last].off_base), u32::from(of[last]));
    writer.flush();

    for i in (0..last).rev() {
        let ll_code = ll[i];
        let of_code = of[i];
        let ml_code = ml[i];
        let ll_bits = u32::from(LL_BITS[usize::from(ll_code)]);
        let of_bits = u32::from(of_code);
        let ml_bits = u32::from(ML_BITS[usize::from(ml_code)]);
        state_of.encode(&mut writer, u32::from(of_code));
        state_ml.encode(&mut writer, u32::from(ml_code));
        state_ll.encode(&mut writer, u32::from(ll_code));
        if of_bits + ml_bits + ll_bits >= 64 - 7 - (LL_FSE_LOG + ML_FSE_LOG + OFF_FSE_LOG) {
            writer.flush();
        }
        writer.add_bits(u64::from(seqs[i].lit_length), ll_bits);
        writer.add_bits(u64::from(seqs[i].ml_base), ml_bits);
        if of_bits + ml_bits + ll_bits > 56 {
            writer.flush();
        }
        writer.add_bits(u64::from(seqs[i].off_base), of_bits);
        writer.flush();
    }

    state_ml.flush(&mut writer);
    state_of.flush(&mut writer);
    state_ll.flush(&mut writer);
    match writer.close() {
        0 => Err(CoreError::DstSizeTooSmall),
        size => Ok(size),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_code_is_rle_unless_basic_is_cheaper() {
        let count = [0, 5, 0];
        let stats = CodeStats {
            count: &count,
            max: 1,
            most_frequent: 5,
            nb_seq: 5,
        };
        let table = CTable::default();
        let mut repeat = FseRepeat::Check;
        let kind = select_encoding_type(
            &mut repeat,
            stats,
            LITERAL_LENGTHS,
            &table,
            true,
            Strategy::Fast,
        );
        assert_eq!(kind, EncodingType::Rle);
        assert_eq!(repeat, FseRepeat::None);
        let two = CodeStats {
            nb_seq: 2,
            most_frequent: 2,
            ..stats
        };
        assert_eq!(
            select_encoding_type(
                &mut repeat,
                two,
                LITERAL_LENGTHS,
                &table,
                true,
                Strategy::Fast
            ),
            EncodingType::Basic
        );
    }

    #[test]
    fn costs_reject_tables_that_cannot_encode_present_symbols() {
        let mut table = CTable::default();
        table.build(&LL_DEFAULT_NORM, MAX_LL, LL_DEFAULT_NORM_LOG);
        let mut count = [0u32; 64];
        count[..3].copy_from_slice(&[10, 5, 1]);
        assert_ne!(fse_bit_cost(&table, &count, 2), COST_ERROR);
        assert_eq!(fse_bit_cost(&table, &count, 40), COST_ERROR);
        let mut sparse = CTable::default();
        let mut norm = [0i16; 64];
        norm[0] = 32;
        sparse.build(&norm, 2, 5);
        assert_eq!(fse_bit_cost(&sparse, &count, 2), COST_ERROR);
        assert!(cross_entropy_cost(&LL_DEFAULT_NORM, LL_DEFAULT_NORM_LOG, &count, 2) > 0);
        assert!(entropy_cost(&count, 2, 16) > 0);
    }

    #[test]
    fn rle_tables_need_a_byte_of_room() {
        let mut next = CTable::default();
        let prev = CTable::default();
        let mut count = [0u32; 64];
        assert_eq!(
            build_ctable(
                &mut [],
                &mut next,
                &prev,
                LITERAL_LENGTHS,
                EncodingType::Rle,
                Histogram {
                    count: &mut count,
                    max: 3,
                    codes: &[3],
                },
            ),
            Err(CoreError::DstSizeTooSmall)
        );
    }
}
