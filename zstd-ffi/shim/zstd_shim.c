/*
 * Test-only entry points into zstd internals.
 *
 * mzstd checks byte identity layer by layer: resolved parameters, entropy
 * tables, then whole frames. The first two are not reachable through the
 * public API, so this translation unit includes the reference's private
 * headers and exposes exactly what the differential tests compare. It adds no
 * behaviour of its own.
 */

#define ZSTD_STATIC_LINKING_ONLY
#define FSE_STATIC_LINKING_ONLY
#define HUF_STATIC_LINKING_ONLY

#include <stdlib.h>
#include <string.h>

#include "zstd.h"
#include "common/fse.h"
#include "common/huf.h"
#include "compress/zstd_compress_internal.h"

typedef struct {
    ZSTD_compressionParameters cparams;
    int enable_ldm;
    unsigned ldm_hash_log;
    unsigned ldm_bucket_size_log;
    unsigned ldm_min_match_length;
    unsigned ldm_hash_rate_log;
    unsigned ldm_window_log;
    int use_row_match_finder;
    int post_block_splitter;
    int pre_block_splitter_level;
    size_t max_block_size;
    int search_for_external_repcodes;
} mzstd_resolved_params;

/* Reports the parameters the last compression on `cctx` was applied with. */
void mzstd_shim_applied_params(const ZSTD_CCtx* cctx, mzstd_resolved_params* out)
{
    const ZSTD_CCtx_params* p = &cctx->appliedParams;
    memset(out, 0, sizeof(*out));
    out->cparams = p->cParams;
    out->enable_ldm = (int)p->ldmParams.enableLdm;
    out->ldm_hash_log = p->ldmParams.hashLog;
    out->ldm_bucket_size_log = p->ldmParams.bucketSizeLog;
    out->ldm_min_match_length = p->ldmParams.minMatchLength;
    out->ldm_hash_rate_log = p->ldmParams.hashRateLog;
    out->ldm_window_log = p->ldmParams.windowLog;
    out->use_row_match_finder = (int)p->useRowMatchFinder;
    out->post_block_splitter = (int)p->postBlockSplitter;
    out->pre_block_splitter_level = p->preBlockSplitter_level;
    out->max_block_size = p->maxBlockSize;
    out->search_for_external_repcodes = (int)p->searchForExternalRepcodes;
}

size_t mzstd_shim_fse_normalize_count(short* normalized, unsigned table_log,
                                      const unsigned* count, size_t total,
                                      unsigned max_symbol_value,
                                      unsigned use_low_prob_count)
{
    return FSE_normalizeCount(normalized, table_log, count, total,
                              max_symbol_value, use_low_prob_count);
}

unsigned mzstd_shim_fse_optimal_table_log(unsigned max_table_log, size_t src_size,
                                          unsigned max_symbol_value)
{
    return FSE_optimalTableLog(max_table_log, src_size, max_symbol_value);
}

/*
 * Builds a Huffman table and reads it back per symbol. A HUF_CElt keeps the
 * code length in its low byte and the code, left-aligned, in its high bits.
 * Returns the table log, or an error code.
 */
size_t mzstd_shim_huf_build(const unsigned* count, unsigned max_symbol_value,
                            unsigned max_nb_bits, unsigned char* nb_bits,
                            unsigned short* values)
{
    HUF_CREATE_STATIC_CTABLE(table, HUF_SYMBOLVALUE_MAX);
    unsigned workspace[HUF_CTABLE_WORKSPACE_SIZE_U32];
    size_t const log = HUF_buildCTable_wksp(table, count, max_symbol_value,
                                            max_nb_bits, workspace,
                                            sizeof(workspace));
    unsigned s;
    if (ZSTD_isError(log)) return log;
    for (s = 0; s <= max_symbol_value; s++) {
        HUF_CElt const elt = table[s + 1];
        unsigned const bits = (unsigned)(elt & 0xFF);
        nb_bits[s] = (unsigned char)bits;
        values[s] = bits ? (unsigned short)(elt >> (sizeof(HUF_CElt) * 8 - bits)) : 0;
    }
    return log;
}

unsigned mzstd_shim_huf_optimal_table_log(unsigned max_table_log, size_t src_size,
                                          unsigned max_symbol_value,
                                          const unsigned* count, int flags)
{
    HUF_CREATE_STATIC_CTABLE(table, HUF_SYMBOLVALUE_MAX);
    /* HUF_optimalTableLog needs room for its write workspace and a trial
     * output; the entropy workspace zstd itself passes is this size. */
    unsigned* const workspace = (unsigned*)malloc(ENTROPY_WORKSPACE_SIZE);
    unsigned log;
    if (workspace == NULL) return 0;
    log = HUF_optimalTableLog(max_table_log, src_size, max_symbol_value,
                              workspace, ENTROPY_WORKSPACE_SIZE, table, count, flags);
    free(workspace);
    return log;
}
