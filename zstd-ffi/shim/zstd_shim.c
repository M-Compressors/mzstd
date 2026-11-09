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

/*
 * FSE_readNCount: reads a normalized distribution description. Returns the
 * description's size, or an error code.
 */
size_t mzstd_shim_fse_read_ncount(short* normalized, unsigned* max_symbol_value,
                                  unsigned* table_log, const void* src, size_t src_size)
{
    return FSE_readNCount(normalized, max_symbol_value, table_log, src, src_size);
}

/*
 * HUF_readCTable: reads a Huffman description and reports each symbol's
 * code length and value as mzstd_shim_huf_build does. Returns the
 * description's size, or an error code.
 */
size_t mzstd_shim_huf_read_ctable(const void* src, size_t src_size,
                                  unsigned* max_symbol_value, unsigned* has_zero_weights,
                                  unsigned* table_log, unsigned char* nb_bits,
                                  unsigned short* values)
{
    HUF_CREATE_STATIC_CTABLE(table, HUF_SYMBOLVALUE_MAX);
    unsigned s;
    size_t const size = HUF_readCTable(table, max_symbol_value, src, src_size, has_zero_weights);
    if (ZSTD_isError(size)) return size;
    *table_log = (unsigned)(table[0] & 0xFF);
    for (s = 0; s <= *max_symbol_value; s++) {
        HUF_CElt const elt = table[s + 1];
        unsigned const bits = (unsigned)(elt & 0xFF);
        nb_bits[s] = (unsigned char)bits;
        values[s] = bits ? (unsigned short)(elt >> (sizeof(HUF_CElt) * 8 - bits)) : 0;
    }
    return size;
}

/* ---- OpenZL's zstd node ------------------------------------------------
 *
 * Line-for-line ports of the calls `encode_zstd_binding.c` and
 * `decode_zstd_binding.c` (OpenZL v0.3.0) make into zstd, so the differential
 * tests compare against exactly what OpenZL runs. The OpenZL bookkeeping
 * (typed streams, element-width varint) is left out.
 */

#define MZSTD_MEMORY_USAGE_CAP_LOG_FACTOR 3

/* `EI_zstd_parameter_valid`. */
static int mzstd_openzl_parameter_valid(ZSTD_cParameter param, int value)
{
    if (param == ZSTD_c_format || param == ZSTD_c_contentSizeFlag) {
        return 0;
    }
    if (param == ZSTD_c_windowLog) {
        return value <= ZSTD_WINDOWLOG_MAX - MZSTD_MEMORY_USAGE_CAP_LOG_FACTOR;
    }
    if (param == ZSTD_c_hashLog) {
        return value <= ZSTD_HASHLOG_MAX - MZSTD_MEMORY_USAGE_CAP_LOG_FACTOR;
    }
    if (param == ZSTD_c_chainLog) {
        return value <= ZSTD_CHAINLOG_MAX - MZSTD_MEMORY_USAGE_CAP_LOG_FACTOR;
    }
    if (param == ZSTD_c_ldmHashLog) {
        return value <= ZSTD_LDM_HASHLOG_MAX - MZSTD_MEMORY_USAGE_CAP_LOG_FACTOR;
    }
    return 1;
}

/* `ZSTD_createCDict_advanced` as `EIZSTD_materializeCDict` calls it. */
ZSTD_CDict* mzstd_shim_openzl_create_cdict(const void* raw, size_t raw_size, int clevel)
{
    ZSTD_customMem const custom_mem = { NULL, NULL, NULL };
    return ZSTD_createCDict_advanced(raw, raw_size, ZSTD_dlm_byCopy, ZSTD_dct_auto,
                                     ZSTD_getCParams(clevel, 0, raw_size), custom_mem);
}

/* `ZSTD_createDDict_advanced` as `DIZSTD_materializeDDict` calls it. */
ZSTD_DDict* mzstd_shim_openzl_create_ddict(const void* raw, size_t raw_size)
{
    ZSTD_customMem const custom_mem = { NULL, NULL, NULL };
    return ZSTD_createDDict_advanced(raw, raw_size, ZSTD_dlm_byCopy, ZSTD_dct_auto, custom_mem);
}

/* `EI_zstdWithCCtx` without the OpenZL stream. `dst` must hold
 * `ZSTD_compressBound(nb_elts * elt_width) + 3 * nb_elts` bytes. Returns the
 * compressed size or a zstd error; `*policy_rejected` is set when
 * `EI_zstd_parameter_valid` refuses a parameter before zstd sees it. */
size_t mzstd_shim_openzl_encode(
        ZSTD_CCtx* cctx,
        int magicless,
        int compression_level,
        int decompression_level,
        const int* params,
        size_t nb_params,
        const ZSTD_CDict* cdict,
        const void* src,
        size_t elt_width,
        size_t nb_elts,
        void* dst,
        size_t dst_capacity,
        int* policy_rejected)
{
    size_t const min_block_size = 1024;
    size_t const max_nb_elts = 8;
    int const block_split = nb_elts > 0 && elt_width >= min_block_size && nb_elts <= max_nb_elts;
    size_t const src_size = nb_elts * elt_width;
    size_t const block_size = block_split ? elt_width : src_size;
    size_t n;

    *policy_rejected = 0;
    {
        size_t const r = ZSTD_CCtx_reset(cctx, ZSTD_reset_session_and_parameters);
        if (ZSTD_isError(r)) return r;
    }
    if (magicless) {
        size_t const r = ZSTD_CCtx_setParameter(cctx, ZSTD_c_format, ZSTD_f_zstd1_magicless);
        if (ZSTD_isError(r)) return r;
    }
    {
        size_t const r = ZSTD_CCtx_setParameter(cctx, ZSTD_c_compressionLevel, compression_level);
        if (ZSTD_isError(r)) return r;
    }
    if (decompression_level == 1) {
        size_t const r = ZSTD_CCtx_setParameter(cctx, ZSTD_c_literalCompressionMode, ZSTD_lcm_uncompressed);
        if (ZSTD_isError(r)) return r;
    }
    for (n = 0; n < nb_params; n++) {
        ZSTD_cParameter const param = (ZSTD_cParameter)params[2 * n];
        int const value = params[2 * n + 1];
        if (!mzstd_openzl_parameter_valid(param, value)) {
            *policy_rejected = 1;
            return 0;
        }
        {
            size_t const r = ZSTD_CCtx_setParameter(cctx, param, value);
            if (ZSTD_isError(r)) return r;
        }
    }
    if (block_size == src_size) {
        if (cdict != NULL) {
            size_t const r = ZSTD_CCtx_refCDict(cctx, cdict);
            if (ZSTD_isError(r)) return r;
        }
        return ZSTD_compress2(cctx, dst, dst_capacity, src, src_size);
    } else {
        ZSTD_outBuffer out = { dst, dst_capacity, 0 };
        ZSTD_inBuffer in = { src, block_size, 0 };
        ZSTD_CCtx_setPledgedSrcSize(cctx, src_size);
        for (; in.pos < src_size; in.size += block_size) {
            while (in.pos < in.size) {
                ZSTD_EndDirective const flush = in.size == src_size ? ZSTD_e_end : ZSTD_e_flush;
                size_t const ret = ZSTD_compressStream2(cctx, &out, &in, flush);
                if (ZSTD_isError(ret)) return ret;
            }
        }
        return out.pos;
    }
}

/* `DI_zstd` without the OpenZL stream: reads the content size from the
 * header (`getFrameContentSize`), decompresses into `dst` of exactly that
 * size, and returns the size. Returns a zstd error, or sets `*bad_header`
 * when the header does not give a usable content size. */
size_t mzstd_shim_openzl_decode(
        ZSTD_DCtx* dctx,
        int magicless,
        const ZSTD_DDict* ddict,
        const void* src,
        size_t src_size,
        void* dst,
        size_t dst_capacity,
        unsigned long long* content_size,
        int* bad_header)
{
    ZSTD_format_e const format = magicless ? ZSTD_f_zstd1_magicless : ZSTD_f_zstd1;
    ZSTD_FrameHeader header;
    size_t ret;
    *bad_header = 0;
    ret = ZSTD_getFrameHeader_advanced(&header, src, src_size, format);
    if (ZSTD_isError(ret) || ret != 0 || header.frameContentSize == ZSTD_CONTENTSIZE_ERROR
        || header.frameContentSize == ZSTD_CONTENTSIZE_UNKNOWN) {
        *bad_header = 1;
        return ZSTD_isError(ret) ? ret : 0;
    }
    *content_size = header.frameContentSize;
    if (header.frameContentSize > dst_capacity) {
        *bad_header = 2;
        return 0;
    }
    ret = ZSTD_DCtx_reset(dctx, ZSTD_reset_session_and_parameters);
    if (ZSTD_isError(ret)) return ret;
    if (magicless) {
        ret = ZSTD_DCtx_setParameter(dctx, ZSTD_d_format, ZSTD_f_zstd1_magicless);
        if (ZSTD_isError(ret)) return ret;
    }
    return ZSTD_decompress_usingDDict(dctx, dst, (size_t)header.frameContentSize, src, src_size, ddict);
}

/* `ZSTD_getFrameHeader_advanced`. Returns its result; `out` is filled when
 * that is zero: content size, window size, dictionary ID, header size,
 * checksum flag, frame type. */
size_t mzstd_shim_frame_header(const void* src, size_t src_size, int magicless,
                               unsigned long long* out_content_size,
                               unsigned long long* out_window_size,
                               unsigned* out_dict_id, unsigned* out_header_size,
                               unsigned* out_checksum, int* out_frame_type)
{
    ZSTD_FrameHeader header;
    size_t const ret = ZSTD_getFrameHeader_advanced(
            &header, src, src_size, magicless ? ZSTD_f_zstd1_magicless : ZSTD_f_zstd1);
    if (ret == 0) {
        *out_content_size = header.frameContentSize;
        *out_window_size = header.windowSize;
        *out_dict_id = header.dictID;
        *out_header_size = header.headerSize;
        *out_checksum = header.checksumFlag;
        *out_frame_type = (int)header.frameType;
    }
    return ret;
}
