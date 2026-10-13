//! The compression context: parameter resolution, frame layout and block
//! compression (`ZSTD_CCtx` and the frame half of `zstd_compress.c`).
//!
//! [`Engine`] owns everything that survives between blocks — match finder
//! tables, entropy state, the frame's checksum and progress — and compresses
//! the bytes it is given wherever they live, resolving addresses through
//! [`Segments`]. Buffering of streamed input and output lives one layer up,
//! in [`super::stream`].

use alloc::boxed::Box;

use super::block;
use super::entropy::{FseRepeat, Tables};
use super::error::CoreError;
use super::ldm::{LdmState, RawSeqStore};
use super::match_state::MatchState;
use super::params::{
    self, CONTENT_SIZE_UNKNOWN, CParamOverrides, CParams, LDM_DEFAULT_WINDOW_LOG, ParamSwitch,
    Strategy,
};
use super::presplit;
use super::seq_store::SeqStore;
use super::window::{Addr, CURRENT_MAX, Segments};
use crate::Backend;
use crate::shared::format::{
    BLOCK_COMPRESSED, BLOCK_HEADER_SIZE, BLOCK_RAW, BLOCK_RLE, BLOCK_SIZE_MAX,
    FRAME_HEADER_SIZE_MAX, MAGIC_NUMBER, REP_NUM, REP_START, WINDOW_LOG_ABSOLUTE_MIN,
};
use crate::shared::xxhash::Xxh64;

/// `ZSTD_MAX_INPUT_SIZE` on 64-bit targets.
const MAX_INPUT_SIZE: u64 = 0xFF00_FF00_FF00_FF00;

/// `ZSTD_compressBound`, or `None` for inputs too large to bound.
pub(crate) const fn compress_bound(src_size: usize) -> Option<usize> {
    if src_size as u64 >= MAX_INPUT_SIZE {
        return None;
    }
    let margin = if src_size < 128 << 10 {
        ((128 << 10) - src_size) >> 11
    } else {
        0
    };
    Some(src_size + (src_size >> 8) + margin)
}

/// Frame header switches (`ZSTD_frameParameters`).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct FrameParams {
    /// Write the content size when known.
    pub(crate) content_size: bool,
    /// Append an XXH64 checksum of the content.
    pub(crate) checksum: bool,
    /// Omit the dictionary ID.
    pub(crate) no_dict_id: bool,
}

impl Default for FrameParams {
    fn default() -> Self {
        Self {
            content_size: true,
            checksum: false,
            no_dict_id: false,
        }
    }
}

/// Long distance matching parameters (`ldmParams_t`); `0` means automatic.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) struct LdmParams {
    pub(crate) enable: ParamSwitch,
    pub(crate) hash_log: u32,
    pub(crate) bucket_size_log: u32,
    pub(crate) min_match_length: u32,
    pub(crate) hash_rate_log: u32,
    pub(crate) window_log: u32,
}

/// `LDM_MIN_MATCH_LENGTH`.
const LDM_MIN_MATCH_LENGTH: u32 = 64;
/// `LDM_BUCKET_SIZE_LOG`.
const LDM_BUCKET_SIZE_LOG: u32 = 4;

impl LdmParams {
    /// `ZSTD_ldm_adjustParameters`.
    pub(crate) fn adjust(&mut self, cparams: &CParams) {
        self.window_log = cparams.window_log;
        if self.hash_rate_log == 0 {
            if self.hash_log > 0 {
                if self.window_log > self.hash_log {
                    self.hash_rate_log = self.window_log - self.hash_log;
                }
            } else {
                self.hash_rate_log = 7 - cparams.strategy.value() / 3;
            }
        }
        if self.hash_log == 0 {
            self.hash_log = self
                .window_log
                .wrapping_sub(self.hash_rate_log)
                .clamp(params::HASHLOG_MIN, params::HASHLOG_MAX);
        }
        if self.min_match_length == 0 {
            self.min_match_length = LDM_MIN_MATCH_LENGTH;
            if cparams.strategy >= Strategy::BtUltra {
                self.min_match_length /= 2;
            }
        }
        if self.bucket_size_log == 0 {
            self.bucket_size_log = cparams
                .strategy
                .value()
                .clamp(LDM_BUCKET_SIZE_LOG, params::LDM_BUCKETSIZELOG_MAX);
        }
        self.bucket_size_log = self.bucket_size_log.min(self.hash_log);
    }
}

/// What the caller asked for (`requestedParams`).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct RequestedParams {
    pub(crate) compression_level: i32,
    pub(crate) cparams: CParamOverrides,
    pub(crate) fparams: FrameParams,
    /// Expected size when unknown; `0` for none.
    pub(crate) src_size_hint: u32,
    pub(crate) literal_compression: ParamSwitch,
    pub(crate) ldm: LdmParams,
    pub(crate) post_block_splitter: ParamSwitch,
    /// `0` automatic, `1` never split, `2..=6` increasing effort.
    pub(crate) pre_block_splitter_level: i32,
    /// `0` for the format maximum.
    pub(crate) max_block_size: usize,
    pub(crate) use_row_match_finder: ParamSwitch,
}

impl Default for RequestedParams {
    fn default() -> Self {
        Self {
            compression_level: params::DEFAULT_CLEVEL,
            cparams: CParamOverrides::default(),
            fparams: FrameParams::default(),
            src_size_hint: 0,
            literal_compression: ParamSwitch::Auto,
            ldm: LdmParams::default(),
            post_block_splitter: ParamSwitch::Auto,
            pre_block_splitter_level: 0,
            max_block_size: 0,
            use_row_match_finder: ParamSwitch::Auto,
        }
    }
}

/// Parameters one frame is compressed with (`appliedParams`).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct AppliedParams {
    pub(crate) compression_level: i32,
    pub(crate) cparams: CParams,
    pub(crate) fparams: FrameParams,
    pub(crate) literal_compression: ParamSwitch,
    pub(crate) ldm: LdmParams,
    pub(crate) post_block_splitter: bool,
    pub(crate) pre_block_splitter_level: i32,
    pub(crate) max_block_size: usize,
    pub(crate) use_row_match_finder: ParamSwitch,
}

impl AppliedParams {
    /// Whether literals are stored raw (`ZSTD_literalsCompressionIsDisabled`).
    pub(crate) fn literals_compression_disabled(&self) -> bool {
        match self.literal_compression {
            ParamSwitch::Enable => false,
            ParamSwitch::Disable => true,
            ParamSwitch::Auto => {
                self.cparams.strategy == Strategy::Fast && self.cparams.target_length > 0
            }
        }
    }
}

impl RequestedParams {
    /// Resolves the parameters for a frame of `pledged` bytes
    /// (`ZSTD_CCtx_init_compressStream2` up to `ZSTD_compressBegin_internal`).
    pub(crate) fn resolve(&self, pledged: u64) -> AppliedParams {
        let mut hint = pledged;
        if hint == CONTENT_SIZE_UNKNOWN && self.src_size_hint > 0 {
            hint = u64::from(self.src_size_hint);
        }
        let mut cparams = params::level_cparams(self.compression_level, hint);
        if self.ldm.enable == ParamSwitch::Enable {
            cparams.window_log = LDM_DEFAULT_WINDOW_LOG;
        }
        let cparams = params::adjust_cparams(
            cparams.overridden(&self.cparams),
            hint,
            self.use_row_match_finder,
        );
        AppliedParams {
            compression_level: self.compression_level,
            cparams,
            fparams: self.fparams,
            literal_compression: self.literal_compression,
            ldm: LdmParams {
                enable: params::resolve_ldm(self.ldm.enable, &cparams),
                ..self.ldm
            },
            post_block_splitter: params::resolve_block_splitter(self.post_block_splitter, &cparams)
                == ParamSwitch::Enable,
            pre_block_splitter_level: self.pre_block_splitter_level,
            max_block_size: params::resolve_max_block_size(self.max_block_size),
            use_row_match_finder: params::resolve_row_match_finder(
                self.use_row_match_finder,
                &cparams,
            ),
        }
    }
}

/// Entropy tables and repeat offsets a block inherits
/// (`ZSTD_compressedBlockState_t`).
#[derive(Clone, Debug, Default)]
pub(crate) struct BlockState {
    pub(crate) entropy: Tables,
    pub(crate) rep: [u32; REP_NUM],
}

impl BlockState {
    /// `ZSTD_reset_compressedBlockState`.
    fn reset(&mut self) {
        self.rep = REP_START;
        self.entropy.reset();
    }
}

/// Where a frame is in its life (`ZSTD_compressionStage_e`).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum Stage {
    /// No frame begun.
    Created,
    /// Parameters applied, header not yet written.
    Init,
    /// Header written.
    Ongoing,
    /// The last block has been written.
    Ending,
}

/// The block compressor's persistent state.
#[derive(Debug)]
pub(crate) struct Engine {
    pub(crate) applied: AppliedParams,
    pub(crate) stage: Stage,
    pub(crate) block_size_max: usize,
    /// Declared size plus one; `0` when unknown.
    pub(crate) pledged_src_size_plus_one: u64,
    pub(crate) consumed_src_size: u64,
    pub(crate) produced_c_size: u64,
    xxh: Xxh64,
    pub(crate) is_first_block: bool,
    initialized: bool,
    pub(crate) seq_store: SeqStore,
    pub(crate) prev: Box<BlockState>,
    pub(crate) next: Box<BlockState>,
    pub(crate) ms: MatchState,
    pub(crate) ldm: LdmState,
    pub(crate) ldm_store: RawSeqStore,
    /// The SIMD level every kernel of this engine runs with.
    pub(crate) backend: Backend,
}

impl Engine {
    /// A context with no frame begun and no tables allocated, on the
    /// detected backend.
    pub(crate) fn new() -> Self {
        let applied = RequestedParams::default().resolve(CONTENT_SIZE_UNKNOWN);
        Self {
            applied,
            stage: Stage::Created,
            block_size_max: BLOCK_SIZE_MAX,
            pledged_src_size_plus_one: 0,
            consumed_src_size: 0,
            produced_c_size: 0,
            xxh: Xxh64::new(0),
            is_first_block: true,
            initialized: false,
            seq_store: SeqStore::default(),
            prev: Box::default(),
            next: Box::default(),
            ms: MatchState::empty(applied.cparams, applied.use_row_match_finder),
            ldm: LdmState::default(),
            ldm_store: RawSeqStore::default(),
            backend: Backend::default(),
        }
    }

    /// A context with no frame begun whose kernels run on `backend`.
    pub(crate) fn with_backend(backend: Backend) -> Self {
        let mut engine = Self {
            backend,
            ..Self::new()
        };
        engine.ms.backend = backend;
        engine
    }

    /// Starts a frame (`ZSTD_resetCCtx_internal` without dictionaries).
    pub(crate) fn begin(&mut self, applied: AppliedParams, pledged: u64) {
        self.is_first_block = true;
        self.applied = applied;
        if self.applied.ldm.enable == ParamSwitch::Enable {
            self.applied.ldm.adjust(&applied.cparams);
        }
        let cparams = self.applied.cparams;
        let window_size = (1u64 << cparams.window_log).min(pledged).max(1) as usize;
        let block_size = self.applied.max_block_size.min(window_size);

        let index_too_close = self.ms.window.end_index() > CURRENT_MAX - (16 << 20);
        let reset_index = index_too_close || !self.initialized;

        self.pledged_src_size_plus_one = pledged.wrapping_add(1);
        self.consumed_src_size = 0;
        self.produced_c_size = 0;
        if pledged == CONTENT_SIZE_UNKNOWN {
            self.applied.fparams.content_size = false;
        }
        self.block_size_max = block_size;
        self.xxh.reset(0);
        self.stage = Stage::Init;
        self.prev.reset();
        self.ms
            .reset(cparams, self.applied.use_row_match_finder, reset_index);
        self.seq_store.reserve(block_size, cparams.min_match);
        if self.applied.ldm.enable == ParamSwitch::Enable {
            self.ldm.reset(&self.applied.ldm);
            self.ldm_store.capacity = block_size / self.applied.ldm.min_match_length as usize;
        }
        self.initialized = true;
    }

    /// Writes the frame header (`ZSTD_writeFrameHeader`).
    fn write_frame_header(&self, dst: &mut [u8], pledged: u64) -> Result<usize, CoreError> {
        let fparams = self.applied.fparams;
        let dict_id_size_code = 0u8;
        let checksum = u8::from(fparams.checksum);
        let window_size = 1u64 << self.applied.cparams.window_log;
        let single_segment = fparams.content_size && window_size >= pledged;
        let window_log_byte =
            ((self.applied.cparams.window_log - WINDOW_LOG_ABSOLUTE_MIN) << 3) as u8;
        let fcs_code: u8 = if fparams.content_size {
            u8::from(pledged >= 256)
                + u8::from(pledged >= 65536 + 256)
                + u8::from(pledged >= 0xFFFF_FFFF)
        } else {
            0
        };
        let descriptor =
            dict_id_size_code + (checksum << 2) + (u8::from(single_segment) << 5) + (fcs_code << 6);
        if dst.len() < FRAME_HEADER_SIZE_MAX {
            return Err(CoreError::DstSizeTooSmall);
        }
        dst[..4].copy_from_slice(&MAGIC_NUMBER.to_le_bytes());
        let mut pos = 4;
        dst[pos] = descriptor;
        pos += 1;
        if !single_segment {
            dst[pos] = window_log_byte;
            pos += 1;
        }
        match fcs_code {
            0 => {
                if single_segment {
                    dst[pos] = pledged as u8;
                    pos += 1;
                }
            }
            1 => {
                dst[pos..pos + 2].copy_from_slice(&((pledged - 256) as u16).to_le_bytes());
                pos += 2;
            }
            2 => {
                dst[pos..pos + 4].copy_from_slice(&(pledged as u32).to_le_bytes());
                pos += 4;
            }
            _ => {
                dst[pos..pos + 8].copy_from_slice(&pledged.to_le_bytes());
                pos += 8;
            }
        }
        Ok(pos)
    }

    /// Compresses `len` bytes at `src` (`ZSTD_compressContinue_internal` in
    /// frame mode).
    pub(crate) fn compress_continue(
        &mut self,
        segments: &Segments<'_>,
        dst: &mut [u8],
        src: Addr,
        len: usize,
        last_frame_chunk: bool,
    ) -> Result<usize, CoreError> {
        if self.stage == Stage::Created {
            return Err(CoreError::StageWrong);
        }
        let mut fh_size = 0;
        if self.stage == Stage::Init {
            fh_size =
                self.write_frame_header(dst, self.pledged_src_size_plus_one.wrapping_sub(1))?;
            self.stage = Stage::Ongoing;
        }
        if len == 0 {
            return Ok(fh_size);
        }
        if !self
            .ms
            .window
            .update(src, len, self.ms.force_non_contiguous)
        {
            self.ms.force_non_contiguous = false;
            self.ms.next_to_update = self.ms.window.dict_limit;
        }
        if self.applied.ldm.enable == ParamSwitch::Enable {
            self.ldm.window.update(src, len, false);
        }
        let c_size =
            self.compress_frame_chunk(segments, &mut dst[fh_size..], src, len, last_frame_chunk)?;
        self.consumed_src_size += len as u64;
        self.produced_c_size += (c_size + fh_size) as u64;
        if self.pledged_src_size_plus_one != 0
            && self.consumed_src_size + 1 > self.pledged_src_size_plus_one
        {
            return Err(CoreError::SrcSizeWrong);
        }
        Ok(c_size + fh_size)
    }

    /// Compresses the last input of a frame and writes its epilogue
    /// (`ZSTD_compressEnd_public`).
    pub(crate) fn compress_end(
        &mut self,
        segments: &Segments<'_>,
        dst: &mut [u8],
        src: Addr,
        len: usize,
    ) -> Result<usize, CoreError> {
        let c_size = self.compress_continue(segments, dst, src, len, true)?;
        let end = self.write_epilogue(&mut dst[c_size..])?;
        if self.pledged_src_size_plus_one != 0
            && self.pledged_src_size_plus_one != self.consumed_src_size + 1
        {
            return Err(CoreError::SrcSizeWrong);
        }
        Ok(c_size + end)
    }

    /// `ZSTD_writeEpilogue`.
    fn write_epilogue(&mut self, dst: &mut [u8]) -> Result<usize, CoreError> {
        if self.stage == Stage::Created {
            return Err(CoreError::StageWrong);
        }
        let mut op = 0;
        if self.stage == Stage::Init {
            op += self.write_frame_header(dst, 0)?;
            self.stage = Stage::Ongoing;
        }
        if self.stage != Stage::Ending {
            if dst.len() - op < BLOCK_HEADER_SIZE {
                return Err(CoreError::DstSizeTooSmall);
            }
            dst[op..op + 3].copy_from_slice(&[1, 0, 0]);
            op += BLOCK_HEADER_SIZE;
        }
        if self.applied.fparams.checksum {
            if dst.len() - op < 4 {
                return Err(CoreError::DstSizeTooSmall);
            }
            let checksum = self.xxh.digest() as u32;
            dst[op..op + 4].copy_from_slice(&checksum.to_le_bytes());
            op += 4;
        }
        self.stage = Stage::Created;
        Ok(op)
    }

    /// `ZSTD_optimalBlockSize`.
    fn optimal_block_size(&self, src: &[u8], savings: i64) -> usize {
        const SPLIT_LEVELS: [usize; 10] = [0, 0, 1, 2, 2, 3, 3, 4, 4, 4];
        let block_size_max = self.block_size_max;
        if src.len() < 128 << 10 || block_size_max < 128 << 10 {
            return src.len().min(block_size_max);
        }
        if savings < 3 {
            return 128 << 10;
        }
        let level = self.applied.pre_block_splitter_level;
        if level == 1 {
            return 128 << 10;
        }
        let level = if level == 0 {
            SPLIT_LEVELS[self.applied.cparams.strategy as usize]
        } else {
            (level - 2) as usize
        };
        presplit::split_block(&src[..block_size_max], level)
    }

    /// `ZSTD_overflowCorrectIfNeeded`.
    fn overflow_correct_if_needed(&mut self, ip: u32, iend: u32) {
        let cparams = self.applied.cparams;
        let cycle_log = params::cycle_log(cparams.chain_log, cparams.strategy);
        let max_dist = 1u32 << cparams.window_log;
        let ms = &mut self.ms;
        if ms
            .window
            .need_overflow_correction(cycle_log, max_dist, ms.loaded_dict_end, ip, iend)
        {
            let correction = ms.window.correct_overflow(cycle_log, max_dist, ip);
            ms.reduce_index(correction);
            ms.next_to_update = ms.next_to_update.saturating_sub(correction);
            ms.loaded_dict_end = 0;
        }
    }

    /// Compresses a chunk into one or more blocks (`ZSTD_compress_frameChunk`).
    #[cfg_attr(all(feature = "hotpath", not(feature = "no_std")), hotpath::measure)]
    fn compress_frame_chunk(
        &mut self,
        segments: &Segments<'_>,
        dst: &mut [u8],
        src: Addr,
        len: usize,
        last_frame_chunk: bool,
    ) -> Result<usize, CoreError> {
        let max_dist = 1u32 << self.applied.cparams.window_log;
        let mut savings = self.consumed_src_size as i64 - self.produced_c_size as i64;
        let mut remaining = len;
        // Positions are tracked as addresses: overflow correction and the
        // btultra2 statistics pass both move the window's base.
        let mut pos = src;
        let mut op = 0usize;
        let start_index = self.ms.window.index_of(src);

        if self.applied.fparams.checksum {
            let bytes = self
                .ms
                .window
                .view(segments, start_index + len as u32)
                .prefix;
            self.xxh
                .update(bytes.slice(start_index, start_index + len as u32));
        }

        while remaining > 0 {
            let ip = self.ms.window.index_of(pos);
            let view = self.ms.window.view(segments, ip + remaining as u32);
            let block_size =
                self.optimal_block_size(view.prefix.slice(ip, ip + remaining as u32), savings);
            let last_block = last_frame_chunk && block_size == remaining;
            if dst.len() - op < BLOCK_HEADER_SIZE + 2 + 1 {
                return Err(CoreError::DstSizeTooSmall);
            }
            self.overflow_correct_if_needed(ip, ip + block_size as u32);
            let ip = self.ms.window.index_of(pos);
            let block_end = ip + block_size as u32;
            // ZSTD_checkDictValidity has nothing to invalidate without a
            // dictionary.
            let ms = &mut self.ms;
            ms.window
                .enforce_max_dist(ip, max_dist, &mut ms.loaded_dict_end);
            if ms.next_to_update < ms.window.low_limit {
                ms.next_to_update = ms.window.low_limit;
            }

            let view = self.ms.window.view(segments, block_end);
            let block_src = view.prefix.slice(ip, block_end);
            let out = &mut dst[op..];
            let c_size = if self.applied.post_block_splitter {
                super::split_block::compress_split_block(
                    self, segments, &view, ip, block_end, out, last_block,
                )?
            } else {
                let (header, body) = out.split_at_mut(BLOCK_HEADER_SIZE);
                let c_size = block::compress_block_internal(
                    self, segments, &view, ip, block_end, body, true,
                )?;
                if c_size == 0 {
                    no_compress_block(out, block_src, last_block)?
                } else {
                    let (kind, size) = if c_size == 1 {
                        (BLOCK_RLE, block_size as u32)
                    } else {
                        (BLOCK_COMPRESSED, c_size as u32)
                    };
                    write_block_header(header, kind, size, last_block);
                    c_size + BLOCK_HEADER_SIZE
                }
            };
            savings += block_size as i64 - c_size as i64;
            pos = pos.add(block_size as i64);
            remaining -= block_size;
            op += c_size;
            self.is_first_block = false;
        }

        if last_frame_chunk && op > 0 {
            self.stage = Stage::Ending;
        }
        Ok(op)
    }

    /// Bytes held by tables and buffers between frames.
    pub(crate) fn retained_bytes(&self) -> usize {
        let ms = &self.ms;
        let opt = &ms.opt;
        4 * (ms.hash_table.capacity() + ms.chain_table.capacity() + ms.hash_table3.capacity())
            + ms.tag_table.capacity()
            + 4 * (opt.lit_freq.capacity()
                + opt.lit_length_freq.capacity()
                + opt.match_length_freq.capacity()
                + opt.off_code_freq.capacity())
            + opt.table_bytes()
            + self.seq_store.retained_bytes()
            + self.ldm.retained_bytes()
            + self.ldm_store.seq.capacity() * core::mem::size_of::<super::ldm::RawSeq>()
    }

    /// Swaps the block states once a compressed block is committed
    /// (`ZSTD_blockState_confirmRepcodesAndEntropyTables`).
    pub(crate) fn confirm_block_state(&mut self) {
        // Boxed, so this swaps two pointers as the reference does.
        core::mem::swap(&mut self.prev, &mut self.next);
    }

    /// Demotes a dictionary-validated offset table after a block.
    pub(crate) fn demote_offcode_repeat(&mut self) {
        if self.prev.entropy.fse.offcode_repeat == FseRepeat::Valid {
            self.prev.entropy.fse.offcode_repeat = FseRepeat::Check;
        }
    }
}

/// Writes a block header.
pub(crate) fn write_block_header(dst: &mut [u8], kind: u32, size: u32, last_block: bool) {
    let header = u32::from(last_block) + (kind << 1) + (size << 3);
    dst[..3].copy_from_slice(&header.to_le_bytes()[..3]);
}

/// Stores a block uncompressed (`ZSTD_noCompressBlock`).
pub(crate) fn no_compress_block(
    dst: &mut [u8],
    src: &[u8],
    last_block: bool,
) -> Result<usize, CoreError> {
    if src.len() + BLOCK_HEADER_SIZE > dst.len() {
        return Err(CoreError::DstSizeTooSmall);
    }
    write_block_header(dst, BLOCK_RAW, src.len() as u32, last_block);
    dst[BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE + src.len()].copy_from_slice(src);
    Ok(BLOCK_HEADER_SIZE + src.len())
}

/// Stores a block of one repeated byte (`ZSTD_rleCompressBlock`).
pub(crate) fn rle_compress_block(
    dst: &mut [u8],
    byte: u8,
    src_size: usize,
    last_block: bool,
) -> Result<usize, CoreError> {
    if dst.len() < 4 {
        return Err(CoreError::DstSizeTooSmall);
    }
    write_block_header(dst, BLOCK_RLE, src_size as u32, last_block);
    dst[3] = byte;
    Ok(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compress_bound_matches_the_reference_formula() {
        for size in [0usize, 1, 100, 128 << 10, 1 << 20] {
            assert_eq!(
                compress_bound(size),
                Some(zstd_ffi::oracle::compress_bound(size))
            );
        }
        assert_eq!(compress_bound(usize::MAX), None);
    }

    #[test]
    fn ldm_parameters_derive_from_the_strategy() {
        let cparams = params::level_cparams(22, CONTENT_SIZE_UNKNOWN);
        let mut ldm = LdmParams::default();
        ldm.adjust(&cparams);
        assert_eq!(ldm.window_log, cparams.window_log);
        assert_eq!(ldm.hash_rate_log, 4);
        assert_eq!(ldm.min_match_length, 32);
        assert!(ldm.bucket_size_log <= ldm.hash_log);
        let mut explicit = LdmParams {
            hash_log: 20,
            ..LdmParams::default()
        };
        explicit.adjust(&cparams);
        assert_eq!(explicit.hash_rate_log, cparams.window_log - 20);
    }
}
