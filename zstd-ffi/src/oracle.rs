//! Safe wrappers over the call shapes mzstd's tests compare against.
//!
//! Every function here owns its contexts and buffers for the duration of the
//! call, so none of the raw API's lifetime rules escape. Failures are returned
//! as the reference's error name; the tests only ever compare or print them.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::ffi::{CStr, c_int, c_void};

use crate::*;

/// The reference's error name for a result code.
fn error_name(code: usize) -> String {
    // SAFETY: `ZSTD_getErrorName` returns a pointer to a static,
    // NUL-terminated string for every input.
    let name = unsafe { CStr::from_ptr(ZSTD_getErrorName(code)) };
    String::from(name.to_str().unwrap_or("non-UTF-8 error name"))
}

fn check(code: usize) -> Result<usize, String> {
    // SAFETY: `ZSTD_isError` only inspects the integer.
    if unsafe { ZSTD_isError(code) } != 0 {
        Err(error_name(code))
    } else {
        Ok(code)
    }
}

/// `ZSTD_compressBound`.
pub fn compress_bound(len: usize) -> usize {
    // SAFETY: a pure function of its argument.
    unsafe { ZSTD_compressBound(len) }
}

/// One-shot `ZSTD_compress` at `level`.
///
/// # Panics
///
/// Panics when the reference reports an error, which it cannot for a buffer
/// of `ZSTD_compressBound` bytes.
pub fn compress(input: &[u8], level: i32) -> Vec<u8> {
    compress_capacity(input, level, compress_bound(input.len())).expect("ZSTD_compress")
}

/// One-shot `ZSTD_compress` at `level` into a destination of `capacity`
/// bytes, or the reference's error name.
pub fn compress_capacity(input: &[u8], level: i32, capacity: usize) -> Result<Vec<u8>, String> {
    let mut output = vec![0u8; capacity];
    // SAFETY: both buffers are valid for the lengths passed and do not
    // overlap.
    let written = unsafe {
        ZSTD_compress(
            output.as_mut_ptr().cast(),
            output.len(),
            input.as_ptr().cast(),
            input.len(),
            level,
        )
    };
    let written = check(written)?;
    output.truncate(written);
    Ok(output)
}

/// A compression context owned for the duration of one oracle call.
struct CCtx(*mut ZSTD_CCtx);

impl CCtx {
    fn new() -> Self {
        // SAFETY: allocation only; a null result is checked.
        let cctx = unsafe { ZSTD_createCCtx() };
        assert!(!cctx.is_null(), "ZSTD_createCCtx failed");
        Self(cctx)
    }

    fn set(&mut self, param: ZSTD_cParameter, value: i32) -> Result<(), String> {
        // SAFETY: `self.0` is a live context.
        check(unsafe { ZSTD_CCtx_setParameter(self.0, param, value as c_int) }).map(|_| ())
    }

    fn configure(&mut self, params: &Params) -> Result<(), String> {
        for &(param, value) in &params.values {
            self.set(param, value)?;
        }
        if let Some(size) = params.pledged {
            // SAFETY: `self.0` is a live context.
            check(unsafe { ZSTD_CCtx_setPledgedSrcSize(self.0, size) })?;
        }
        Ok(())
    }

    fn applied(&self) -> MzstdResolvedParams {
        let mut out = MzstdResolvedParams::default();
        // SAFETY: `self.0` is a live context and `out` a valid destination.
        unsafe { mzstd_shim_applied_params(self.0, &mut out) };
        out
    }
}

impl Drop for CCtx {
    fn drop(&mut self) {
        // SAFETY: the context was created by `ZSTD_createCCtx` and is freed
        // once.
        unsafe { ZSTD_freeCCtx(self.0) };
    }
}

/// A configured compression context kept across calls, for benchmarks that
/// must not time context creation.
pub struct Encoder(CCtx);

impl Encoder {
    /// A context with `params` applied.
    ///
    /// # Panics
    ///
    /// Panics when the reference rejects a parameter.
    pub fn new(params: &Params) -> Self {
        let mut cctx = CCtx::new();
        cctx.configure(params)
            .expect("parameters the reference accepts");
        Self(cctx)
    }

    /// `ZSTD_compress2` into `output`, returning the frame's size.
    ///
    /// # Panics
    ///
    /// Panics when the reference fails, which it cannot for an output of
    /// `ZSTD_compressBound` bytes.
    pub fn compress_into(&mut self, input: &[u8], output: &mut [u8]) -> usize {
        // SAFETY: both buffers are valid for the lengths passed and do not
        // overlap; the context is live.
        check(unsafe {
            ZSTD_compress2(
                (self.0).0,
                output.as_mut_ptr().cast(),
                output.len(),
                input.as_ptr().cast(),
                input.len(),
            )
        })
        .expect("ZSTD_compress2")
    }
}

/// A decompression context kept across calls.
pub struct Decoder(*mut ZSTD_DCtx);

impl Decoder {
    /// A context accepting windows up to 2 GiB.
    ///
    /// # Panics
    ///
    /// Panics when the context cannot be created.
    pub fn new() -> Self {
        // SAFETY: allocation only; a null result is checked.
        let dctx = unsafe { ZSTD_createDCtx() };
        assert!(!dctx.is_null(), "ZSTD_createDCtx failed");
        // SAFETY: `dctx` is a live context.
        check(unsafe { ZSTD_DCtx_setParameter(dctx, ZSTD_d_windowLogMax, 31) })
            .expect("window log 31 is accepted");
        Self(dctx)
    }

    /// `ZSTD_decompressDCtx` into `output`, returning the content size.
    ///
    /// # Panics
    ///
    /// Panics when the frame is invalid or does not fit.
    pub fn decompress_into(&mut self, frame: &[u8], output: &mut [u8]) -> usize {
        // SAFETY: both buffers are valid for the lengths passed and do not
        // overlap; the context is live.
        check(unsafe {
            ZSTD_decompressDCtx(
                self.0,
                output.as_mut_ptr().cast(),
                output.len(),
                frame.as_ptr().cast(),
                frame.len(),
            )
        })
        .expect("ZSTD_decompressDCtx")
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: the context was created by `ZSTD_createDCtx` and is freed
        // once.
        unsafe { ZSTD_freeDCtx(self.0) };
    }
}

/// Advanced parameters applied, in order, to a fresh compression context.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Params {
    /// `(ZSTD_c_*, value)` pairs.
    pub values: Vec<(ZSTD_cParameter, i32)>,
    /// `ZSTD_CCtx_setPledgedSrcSize`, when set.
    pub pledged: Option<u64>,
}

impl Params {
    /// Parameters with only a compression level set.
    pub fn level(level: i32) -> Self {
        Self::default().with(ZSTD_c_compressionLevel, level)
    }

    /// Appends one parameter.
    #[must_use]
    pub fn with(mut self, param: ZSTD_cParameter, value: i32) -> Self {
        self.values.push((param, value));
        self
    }

    /// Sets the pledged source size.
    #[must_use]
    pub fn with_pledged(mut self, size: u64) -> Self {
        self.pledged = Some(size);
        self
    }
}

/// `ZSTD_compress2` with `params`.
///
/// # Errors
///
/// Returns the reference's error name when it rejects a parameter or the
/// compression.
pub fn compress2(input: &[u8], params: &Params) -> Result<Vec<u8>, String> {
    compress2_with_params(input, params).map(|(output, _)| output)
}

/// `ZSTD_compress2` into a destination of exactly `capacity` bytes.
///
/// # Errors
///
/// Returns the reference's error name, including `Destination buffer is too
/// small` when the frame does not fit.
pub fn compress2_capacity(
    input: &[u8],
    params: &Params,
    capacity: usize,
) -> Result<Vec<u8>, String> {
    let mut cctx = CCtx::new();
    cctx.configure(params)?;
    let mut output = vec![0u8; capacity];
    // SAFETY: both buffers are valid for the lengths passed and do not
    // overlap; the context is live.
    let written = check(unsafe {
        ZSTD_compress2(
            cctx.0,
            output.as_mut_ptr().cast(),
            output.len(),
            input.as_ptr().cast(),
            input.len(),
        )
    })?;
    output.truncate(written);
    Ok(output)
}

/// `ZSTD_compress2`, also reporting the parameters it applied.
///
/// # Errors
///
/// As [`compress2`].
pub fn compress2_with_params(
    input: &[u8],
    params: &Params,
) -> Result<(Vec<u8>, MzstdResolvedParams), String> {
    let mut cctx = CCtx::new();
    cctx.configure(params)?;
    let mut output = vec![0u8; compress_bound(input.len())];
    // SAFETY: both buffers are valid for the lengths passed and do not
    // overlap; the context is live.
    let written = check(unsafe {
        ZSTD_compress2(
            cctx.0,
            output.as_mut_ptr().cast(),
            output.len(),
            input.as_ptr().cast(),
            input.len(),
        )
    })?;
    output.truncate(written);
    Ok((output, cctx.applied()))
}

/// What a streaming step asks of the reference.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum Directive {
    /// `ZSTD_e_continue`.
    Continue,
    /// `ZSTD_e_flush`.
    Flush,
    /// `ZSTD_e_end`.
    End,
}

impl Directive {
    const fn raw(self) -> ZSTD_EndDirective {
        match self {
            Self::Continue => ZSTD_e_continue,
            Self::Flush => ZSTD_e_flush,
            Self::End => ZSTD_e_end,
        }
    }
}

/// Streams `steps` through `ZSTD_compressStream2`.
///
/// Each step hands over its whole chunk with its directive, and repeats the
/// call until the chunk is consumed and, for `Flush` and `End`, the directive
/// has completed. Output space is never the limiting factor, so the result
/// depends only on the chunk boundaries and directives. A final `End` with no
/// input is appended when the last step is not already one.
///
/// # Errors
///
/// Returns the reference's error name when it rejects a parameter or a step.
pub fn compress_stream(params: &Params, steps: &[(&[u8], Directive)]) -> Result<Vec<u8>, String> {
    compress_stream_with_output(params, steps, 1 << 17)
}

/// [`compress_stream`] with an output buffer of `output_capacity` bytes per
/// call, which must be at least one.
///
/// # Errors
///
/// As [`compress_stream`].
pub fn compress_stream_with_output(
    params: &Params,
    steps: &[(&[u8], Directive)],
    output_capacity: usize,
) -> Result<Vec<u8>, String> {
    let mut cctx = CCtx::new();
    cctx.configure(params)?;
    let mut compressed = Vec::new();
    let mut chunk = vec![0u8; output_capacity.max(1)];
    let mut run = |data: &[u8], directive: Directive| -> Result<(), String> {
        let mut input = ZSTD_inBuffer {
            src: data.as_ptr().cast::<c_void>(),
            size: data.len(),
            pos: 0,
        };
        loop {
            let mut output = ZSTD_outBuffer {
                dst: chunk.as_mut_ptr().cast(),
                size: chunk.len(),
                pos: 0,
            };
            // SAFETY: the buffers describe live, non-overlapping memory of
            // the stated sizes, and the context is live.
            let remaining = check(unsafe {
                ZSTD_compressStream2(cctx.0, &mut output, &mut input, directive.raw())
            })?;
            compressed.extend_from_slice(&chunk[..output.pos]);
            let done = match directive {
                Directive::Continue => input.pos == input.size,
                Directive::Flush | Directive::End => remaining == 0,
            };
            if done {
                return Ok(());
            }
        }
    };
    for &(data, directive) in steps {
        run(data, directive)?;
    }
    if steps
        .last()
        .is_none_or(|&(_, directive)| directive != Directive::End)
    {
        run(&[], Directive::End)?;
    }
    Ok(compressed)
}

/// One-shot decompression of every frame in `input`.
///
/// # Errors
///
/// Returns the reference's error name for malformed input or when the output
/// would exceed `capacity`.
pub fn decompress(input: &[u8], capacity: usize) -> Result<Vec<u8>, String> {
    let mut output = vec![0u8; capacity];
    // SAFETY: both buffers are valid for the lengths passed and do not
    // overlap.
    let written = check(unsafe {
        ZSTD_decompress(
            output.as_mut_ptr().cast(),
            output.len(),
            input.as_ptr().cast(),
            input.len(),
        )
    })?;
    output.truncate(written);
    Ok(output)
}

/// Streaming decompression of `input`, fed in `chunk`-byte pieces, with the
/// decoder's window limited to `window_log_max` bits.
///
/// # Errors
///
/// Returns the reference's error name for malformed input, an exceeded window
/// limit, or input that ends inside a frame.
pub fn decompress_stream(
    input: &[u8],
    chunk: usize,
    window_log_max: Option<i32>,
) -> Result<Vec<u8>, String> {
    // SAFETY: allocation only; a null result is checked.
    let dctx = unsafe { ZSTD_createDCtx() };
    assert!(!dctx.is_null(), "ZSTD_createDCtx failed");
    struct Guard(*mut ZSTD_DCtx);
    impl Drop for Guard {
        fn drop(&mut self) {
            // SAFETY: created by `ZSTD_createDCtx`, freed once.
            unsafe { ZSTD_freeDCtx(self.0) };
        }
    }
    let guard = Guard(dctx);
    if let Some(log) = window_log_max {
        // SAFETY: the context is live.
        check(unsafe { ZSTD_DCtx_setParameter(guard.0, ZSTD_d_windowLogMax, log) })?;
    }
    let mut decompressed = Vec::new();
    let mut buffer = vec![0u8; 1 << 17];
    let mut last = 0;
    for piece in input.chunks(chunk.max(1)) {
        let mut input = ZSTD_inBuffer {
            src: piece.as_ptr().cast(),
            size: piece.len(),
            pos: 0,
        };
        loop {
            let mut output = ZSTD_outBuffer {
                dst: buffer.as_mut_ptr().cast(),
                size: buffer.len(),
                pos: 0,
            };
            let consumed_before = input.pos;
            // SAFETY: the buffers describe live, non-overlapping memory of
            // the stated sizes, and the context is live.
            let hint = check(unsafe { ZSTD_decompressStream(guard.0, &mut output, &mut input) })?;
            // A call that neither consumes nor produces anything returns the
            // next frame's header size after a frame that ended exactly at a
            // full output buffer; only calls that made progress say whether
            // the input stopped inside a frame.
            if input.pos != consumed_before || output.pos != 0 {
                last = hint;
            }
            decompressed.extend_from_slice(&buffer[..output.pos]);
            if input.pos == input.size && output.pos < output.size {
                break;
            }
        }
    }
    if last != 0 {
        return Err(String::from("truncated input"));
    }
    Ok(decompressed)
}

/// `ZSTD_getCParams`.
pub fn get_cparams(
    level: i32,
    estimated_src_size: u64,
    dict_size: usize,
) -> ZSTD_compressionParameters {
    // SAFETY: a pure function of its arguments.
    unsafe { ZSTD_getCParams(level, estimated_src_size, dict_size) }
}

/// `FSE_normalizeCount`; returns the table log or the reference's error.
///
/// # Errors
///
/// Returns the reference's error name for an invalid table log or histogram.
pub fn fse_normalize_count(
    count: &[u32],
    total: usize,
    table_log: u32,
    use_low_prob_count: bool,
) -> Result<(Vec<i16>, usize), String> {
    let mut normalized = vec![0i16; count.len()];
    let max_symbol_value = (count.len() - 1) as u32;
    // SAFETY: `normalized` and `count` both hold `max_symbol_value + 1`
    // entries.
    let log = check(unsafe {
        mzstd_shim_fse_normalize_count(
            normalized.as_mut_ptr(),
            table_log,
            count.as_ptr(),
            total,
            max_symbol_value,
            u32::from(use_low_prob_count),
        )
    })?;
    Ok((normalized, log))
}

/// `FSE_optimalTableLog`.
pub fn fse_optimal_table_log(max_table_log: u32, src_size: usize, max_symbol_value: u32) -> u32 {
    // SAFETY: a pure function of its arguments.
    unsafe { mzstd_shim_fse_optimal_table_log(max_table_log, src_size, max_symbol_value) }
}

/// A Huffman table as the reference builds it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HufTable {
    /// The table log the builder settled on.
    pub table_log: u32,
    /// Code length per symbol; zero for absent symbols.
    pub nb_bits: Vec<u8>,
    /// Code value per symbol.
    pub values: Vec<u16>,
}

/// `HUF_buildCTable_wksp` for `count`, limited to `max_nb_bits`.
///
/// # Errors
///
/// Returns the reference's error name when it cannot build the table.
pub fn huf_build(count: &[u32], max_nb_bits: u32) -> Result<HufTable, String> {
    let mut nb_bits = vec![0u8; count.len()];
    let mut values = vec![0u16; count.len()];
    // SAFETY: the three arrays hold `count.len()` entries each, and the
    // reference reads at most `max_symbol_value + 1` of them.
    let log = check(unsafe {
        mzstd_shim_huf_build(
            count.as_ptr(),
            (count.len() - 1) as u32,
            max_nb_bits,
            nb_bits.as_mut_ptr(),
            values.as_mut_ptr(),
        )
    })?;
    Ok(HufTable {
        table_log: log as u32,
        nb_bits,
        values,
    })
}

/// `HUF_optimalTableLog`; `optimal_depth` selects `HUF_flags_optimalDepth`.
pub fn huf_optimal_table_log(
    max_table_log: u32,
    src_size: usize,
    count: &[u32],
    optimal_depth: bool,
) -> u32 {
    let flags = if optimal_depth { 1 << 1 } else { 0 };
    // SAFETY: `count` holds `max_symbol_value + 1` entries.
    unsafe {
        mzstd_shim_huf_optimal_table_log(
            max_table_log,
            src_size,
            (count.len() - 1) as u32,
            count.as_ptr(),
            flags,
        )
    }
}
