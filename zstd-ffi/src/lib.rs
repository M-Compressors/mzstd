//! Raw Rust bindings to the vendored Zstandard C library, plus a small safe
//! layer for tests and benchmarks.
//!
//! The bundled C source is zstd v1.5.7, compiled single-threaded without
//! legacy-format support. This crate is the oracle `mzstd` is checked against:
//! its raw functions keep the pointer-based semantics of the upstream API and
//! require every buffer validity, lifetime and aliasing invariant documented in
//! `zstd.h`. The [`oracle`] module wraps the handful of call shapes the tests
//! need in safe functions.

#![no_std]
// The bindings keep the reference's C names so they can be checked against
// `zstd.h` line by line.
#![expect(
    non_camel_case_types,
    non_upper_case_globals,
    non_snake_case,
    reason = "bindings mirror the C identifiers of zstd.h"
)]

extern crate alloc;

use core::ffi::{c_char, c_int, c_uint, c_ulonglong, c_void};

/// Upstream zstd version vendored by this crate.
pub const UPSTREAM_VERSION: &str = "1.5.7";

/// `ZSTD_CONTENTSIZE_UNKNOWN`.
pub const ZSTD_CONTENTSIZE_UNKNOWN: c_ulonglong = u64::MAX;
/// `ZSTD_CONTENTSIZE_ERROR`.
pub const ZSTD_CONTENTSIZE_ERROR: c_ulonglong = u64::MAX - 1;

/// Opaque compression context.
#[repr(C)]
pub struct ZSTD_CCtx {
    _private: [u8; 0],
}

/// Opaque decompression context.
#[repr(C)]
pub struct ZSTD_DCtx {
    _private: [u8; 0],
}

/// `ZSTD_strategy`.
pub type ZSTD_strategy = c_int;
/// `ZSTD_cParameter`.
pub type ZSTD_cParameter = c_int;
/// `ZSTD_dParameter`.
pub type ZSTD_dParameter = c_int;
/// `ZSTD_EndDirective`.
pub type ZSTD_EndDirective = c_int;
/// `ZSTD_ResetDirective`.
pub type ZSTD_ResetDirective = c_int;

pub const ZSTD_fast: ZSTD_strategy = 1;
pub const ZSTD_dfast: ZSTD_strategy = 2;
pub const ZSTD_greedy: ZSTD_strategy = 3;
pub const ZSTD_lazy: ZSTD_strategy = 4;
pub const ZSTD_lazy2: ZSTD_strategy = 5;
pub const ZSTD_btlazy2: ZSTD_strategy = 6;
pub const ZSTD_btopt: ZSTD_strategy = 7;
pub const ZSTD_btultra: ZSTD_strategy = 8;
pub const ZSTD_btultra2: ZSTD_strategy = 9;

pub const ZSTD_c_compressionLevel: ZSTD_cParameter = 100;
pub const ZSTD_c_windowLog: ZSTD_cParameter = 101;
pub const ZSTD_c_hashLog: ZSTD_cParameter = 102;
pub const ZSTD_c_chainLog: ZSTD_cParameter = 103;
pub const ZSTD_c_searchLog: ZSTD_cParameter = 104;
pub const ZSTD_c_minMatch: ZSTD_cParameter = 105;
pub const ZSTD_c_targetLength: ZSTD_cParameter = 106;
pub const ZSTD_c_strategy: ZSTD_cParameter = 107;
pub const ZSTD_c_targetCBlockSize: ZSTD_cParameter = 130;
pub const ZSTD_c_enableLongDistanceMatching: ZSTD_cParameter = 160;
pub const ZSTD_c_ldmHashLog: ZSTD_cParameter = 161;
pub const ZSTD_c_ldmMinMatch: ZSTD_cParameter = 162;
pub const ZSTD_c_ldmBucketSizeLog: ZSTD_cParameter = 163;
pub const ZSTD_c_ldmHashRateLog: ZSTD_cParameter = 164;
pub const ZSTD_c_contentSizeFlag: ZSTD_cParameter = 200;
pub const ZSTD_c_checksumFlag: ZSTD_cParameter = 201;
pub const ZSTD_c_dictIDFlag: ZSTD_cParameter = 202;
pub const ZSTD_c_nbWorkers: ZSTD_cParameter = 400;
pub const ZSTD_c_jobSize: ZSTD_cParameter = 401;
pub const ZSTD_c_overlapLog: ZSTD_cParameter = 402;
/// `ZSTD_c_srcSizeHint` (`ZSTD_c_experimentalParam7`).
pub const ZSTD_c_srcSizeHint: ZSTD_cParameter = 1004;
/// `ZSTD_c_stableInBuffer` (`ZSTD_c_experimentalParam9`).
pub const ZSTD_c_stableInBuffer: ZSTD_cParameter = 1006;
/// `ZSTD_c_stableOutBuffer` (`ZSTD_c_experimentalParam10`).
pub const ZSTD_c_stableOutBuffer: ZSTD_cParameter = 1007;
/// `ZSTD_c_useRowMatchFinder` (`ZSTD_c_experimentalParam14`).
pub const ZSTD_c_useRowMatchFinder: ZSTD_cParameter = 1011;
/// `ZSTD_c_blockSplitterLevel` (`ZSTD_c_experimentalParam20`).
pub const ZSTD_c_blockSplitterLevel: ZSTD_cParameter = 1017;
/// `ZSTD_c_splitAfterSequences` (`ZSTD_c_experimentalParam13`).
pub const ZSTD_c_splitAfterSequences: ZSTD_cParameter = 1010;

pub const ZSTD_d_windowLogMax: ZSTD_dParameter = 100;

pub const ZSTD_e_continue: ZSTD_EndDirective = 0;
pub const ZSTD_e_flush: ZSTD_EndDirective = 1;
pub const ZSTD_e_end: ZSTD_EndDirective = 2;

pub const ZSTD_reset_session_only: ZSTD_ResetDirective = 1;
pub const ZSTD_reset_parameters: ZSTD_ResetDirective = 2;
pub const ZSTD_reset_session_and_parameters: ZSTD_ResetDirective = 3;

/// `ZSTD_compressionParameters`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct ZSTD_compressionParameters {
    pub windowLog: c_uint,
    pub chainLog: c_uint,
    pub hashLog: c_uint,
    pub searchLog: c_uint,
    pub minMatch: c_uint,
    pub targetLength: c_uint,
    pub strategy: ZSTD_strategy,
}

/// `ZSTD_inBuffer`.
#[repr(C)]
#[derive(Debug)]
pub struct ZSTD_inBuffer {
    pub src: *const c_void,
    pub size: usize,
    pub pos: usize,
}

/// `ZSTD_outBuffer`.
#[repr(C)]
#[derive(Debug)]
pub struct ZSTD_outBuffer {
    pub dst: *mut c_void,
    pub size: usize,
    pub pos: usize,
}

/// `ZSTD_bounds`.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct ZSTD_bounds {
    pub error: usize,
    pub lowerBound: c_int,
    pub upperBound: c_int,
}

/// Parameters the reference resolves internally for one compression, as the
/// shim reports them (`mzstd_shim_resolved_params`).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct MzstdResolvedParams {
    pub cparams: ZSTD_compressionParameters,
    pub enable_ldm: c_int,
    pub ldm_hash_log: c_uint,
    pub ldm_bucket_size_log: c_uint,
    pub ldm_min_match_length: c_uint,
    pub ldm_hash_rate_log: c_uint,
    pub ldm_window_log: c_uint,
    pub use_row_match_finder: c_int,
    pub post_block_splitter: c_int,
    pub pre_block_splitter_level: c_int,
    pub max_block_size: usize,
    pub search_for_external_repcodes: c_int,
}

unsafe extern "C" {
    pub fn ZSTD_versionNumber() -> c_uint;
    pub fn ZSTD_compressBound(srcSize: usize) -> usize;
    pub fn ZSTD_isError(result: usize) -> c_uint;
    pub fn ZSTD_getErrorName(result: usize) -> *const c_char;
    pub fn ZSTD_minCLevel() -> c_int;
    pub fn ZSTD_maxCLevel() -> c_int;
    pub fn ZSTD_defaultCLevel() -> c_int;

    pub fn ZSTD_compress(
        dst: *mut c_void,
        dstCapacity: usize,
        src: *const c_void,
        srcSize: usize,
        compressionLevel: c_int,
    ) -> usize;
    pub fn ZSTD_decompress(
        dst: *mut c_void,
        dstCapacity: usize,
        src: *const c_void,
        compressedSize: usize,
    ) -> usize;
    pub fn ZSTD_getFrameContentSize(src: *const c_void, srcSize: usize) -> c_ulonglong;
    pub fn ZSTD_findFrameCompressedSize(src: *const c_void, srcSize: usize) -> usize;

    pub fn ZSTD_createCCtx() -> *mut ZSTD_CCtx;
    pub fn ZSTD_freeCCtx(cctx: *mut ZSTD_CCtx) -> usize;
    pub fn ZSTD_compressCCtx(
        cctx: *mut ZSTD_CCtx,
        dst: *mut c_void,
        dstCapacity: usize,
        src: *const c_void,
        srcSize: usize,
        compressionLevel: c_int,
    ) -> usize;
    pub fn ZSTD_cParam_getBounds(cParam: ZSTD_cParameter) -> ZSTD_bounds;
    pub fn ZSTD_CCtx_setParameter(
        cctx: *mut ZSTD_CCtx,
        param: ZSTD_cParameter,
        value: c_int,
    ) -> usize;
    pub fn ZSTD_CCtx_setPledgedSrcSize(cctx: *mut ZSTD_CCtx, pledgedSrcSize: c_ulonglong) -> usize;
    pub fn ZSTD_CCtx_reset(cctx: *mut ZSTD_CCtx, reset: ZSTD_ResetDirective) -> usize;
    pub fn ZSTD_compress2(
        cctx: *mut ZSTD_CCtx,
        dst: *mut c_void,
        dstCapacity: usize,
        src: *const c_void,
        srcSize: usize,
    ) -> usize;
    pub fn ZSTD_compressStream2(
        cctx: *mut ZSTD_CCtx,
        output: *mut ZSTD_outBuffer,
        input: *mut ZSTD_inBuffer,
        endOp: ZSTD_EndDirective,
    ) -> usize;
    pub fn ZSTD_CStreamInSize() -> usize;
    pub fn ZSTD_CStreamOutSize() -> usize;

    pub fn ZSTD_createDCtx() -> *mut ZSTD_DCtx;
    pub fn ZSTD_freeDCtx(dctx: *mut ZSTD_DCtx) -> usize;
    pub fn ZSTD_DCtx_setParameter(
        dctx: *mut ZSTD_DCtx,
        param: ZSTD_dParameter,
        value: c_int,
    ) -> usize;
    pub fn ZSTD_DCtx_reset(dctx: *mut ZSTD_DCtx, reset: ZSTD_ResetDirective) -> usize;
    pub fn ZSTD_decompressDCtx(
        dctx: *mut ZSTD_DCtx,
        dst: *mut c_void,
        dstCapacity: usize,
        src: *const c_void,
        srcSize: usize,
    ) -> usize;
    pub fn ZSTD_decompressStream(
        dctx: *mut ZSTD_DCtx,
        output: *mut ZSTD_outBuffer,
        input: *mut ZSTD_inBuffer,
    ) -> usize;

    pub fn ZSTD_getCParams(
        compressionLevel: c_int,
        estimatedSrcSize: c_ulonglong,
        dictSize: usize,
    ) -> ZSTD_compressionParameters;
    pub fn ZSTD_adjustCParams(
        cPar: ZSTD_compressionParameters,
        srcSize: c_ulonglong,
        dictSize: usize,
    ) -> ZSTD_compressionParameters;
    pub fn ZSTD_checkCParams(params: ZSTD_compressionParameters) -> usize;

    /// Reports the parameters the last compression on `cctx` was applied with.
    pub fn mzstd_shim_applied_params(cctx: *const ZSTD_CCtx, out: *mut MzstdResolvedParams);

    /// `FSE_normalizeCount`.
    pub fn mzstd_shim_fse_normalize_count(
        normalized: *mut i16,
        table_log: c_uint,
        count: *const c_uint,
        total: usize,
        max_symbol_value: c_uint,
        use_low_prob_count: c_uint,
    ) -> usize;
    /// `FSE_optimalTableLog`.
    pub fn mzstd_shim_fse_optimal_table_log(
        max_table_log: c_uint,
        src_size: usize,
        max_symbol_value: c_uint,
    ) -> c_uint;
    /// `HUF_buildCTable_wksp` followed by reading every symbol's code length
    /// and value back out of the opaque table.
    pub fn mzstd_shim_huf_build(
        count: *const c_uint,
        max_symbol_value: c_uint,
        max_nb_bits: c_uint,
        nb_bits: *mut u8,
        values: *mut u16,
    ) -> usize;
    /// `HUF_optimalTableLog`.
    pub fn mzstd_shim_huf_optimal_table_log(
        max_table_log: c_uint,
        src_size: usize,
        max_symbol_value: c_uint,
        count: *const c_uint,
        flags: c_int,
    ) -> c_uint;
}

pub mod corpus;
pub mod oracle;
