//! Shared helpers and engine-neutral bodies for the AFL fuzz targets.
//!
//! Every target consumes a raw byte string. The helpers here turn its first
//! bytes into an encoder configuration and the matching reference
//! parameters; [`targets`] holds the target bodies. Nothing here depends on
//! AFL: each binary under `src/bin/` is a three line adapter, so a minimised
//! finding replays through `cargo afl test` (see `tests/regressions.rs`).

use mzstd::{CompressionParams, EncoderConfig, LdmParams, Level, LongDistanceMatching, Strategy};
use zstd_ffi::oracle::Params;
use zstd_ffi::*;

pub mod targets;

/// Largest payload any target compresses.
///
/// Long inputs stop adding coverage quickly while slowing every iteration;
/// a quarter mebibyte still spans two full blocks.
pub const MAX_PAYLOAD: usize = 256 << 10;

/// A configuration decoded from fuzz input, and what remains of the input.
#[derive(Debug)]
pub struct Decoded<'a> {
    pub config: EncoderConfig,
    pub params: Params,
    pub rest: &'a [u8],
}

fn take<'a>(data: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if data.len() < n {
        return None;
    }
    let (head, tail) = data.split_at(n);
    *data = tail;
    Some(head)
}

/// Decodes a configuration header.
///
/// Byte 0 selects the level (`-10..=22` plus a few accelerated ones); byte
/// 1 holds flags: checksum, content size, LDM mode (two bits), window,
/// strategy, search and size-hint overrides, each followed by its byte.
pub fn decode_config(mut data: &[u8]) -> Option<Decoded<'_>> {
    let header = take(&mut data, 2)?;
    let level = match header[0] % 40 {
        n @ 0..=32 => i32::from(n) - 10,
        33 => -100,
        34 => -1000,
        35 => -131_072,
        n => i32::from(n) - 20,
    };
    let flags = header[1];
    let mut config = EncoderConfig::default()
        .with_level(Level::try_from(level).ok()?)
        .with_checksum(flags & 1 != 0)
        .with_content_size(flags & 2 == 0);
    let mut params = Params::level(level)
        .with(ZSTD_c_checksumFlag, i32::from(flags & 1 != 0))
        .with(ZSTD_c_contentSizeFlag, i32::from(flags & 2 == 0));
    let mut cparams = CompressionParams::default();
    if flags & 0x10 != 0 {
        let window = 10 + u32::from(take(&mut data, 1)?[0] % 14);
        cparams = cparams.with_window_log(window);
        params = params.with(ZSTD_c_windowLog, window as i32);
    }
    if flags & 0x20 != 0 {
        let byte = take(&mut data, 1)?[0];
        let strategy = Strategy::try_from(1 + byte % 9).ok()?;
        cparams = cparams.with_strategy(strategy);
        params = params.with(ZSTD_c_strategy, i32::from(u8::from(strategy)));
        let min_match = 3 + u32::from(byte >> 4) % 5;
        cparams = cparams.with_min_match(min_match);
        params = params.with(ZSTD_c_minMatch, min_match as i32);
    }
    if flags & 0x40 != 0 {
        let byte = take(&mut data, 1)?[0];
        let search = 1 + u32::from(byte % 8);
        let hash = 6 + u32::from(byte >> 3) % 14;
        cparams = cparams
            .with_search_log(search)
            .with_hash_log(hash)
            .with_chain_log(hash);
        params = params
            .with(ZSTD_c_searchLog, search as i32)
            .with(ZSTD_c_hashLog, hash as i32)
            .with(ZSTD_c_chainLog, hash as i32);
    }
    config = config.with_params(cparams);
    match (flags >> 2) & 3 {
        1 => {
            config = config.with_long_distance_matching(LongDistanceMatching::Disabled);
            params = params.with(ZSTD_c_enableLongDistanceMatching, 2);
        }
        2 | 3 => {
            let byte = take(&mut data, 1)?[0];
            let min_match = 4 + u32::from(byte % 60);
            config = config.with_long_distance_matching(LongDistanceMatching::Enabled(
                LdmParams::default().with_min_match(min_match),
            ));
            params = params
                .with(ZSTD_c_enableLongDistanceMatching, 1)
                .with(ZSTD_c_ldmMinMatch, min_match as i32);
        }
        _ => {}
    }
    if flags & 0x80 != 0 {
        let hint = u32::from(u16::from_le_bytes(take(&mut data, 2)?.try_into().ok()?)) * 64;
        config = config.with_size_hint(Some(hint));
        params = params.with(ZSTD_c_srcSizeHint, hint as i32);
    }
    let rest = &data[..data.len().min(MAX_PAYLOAD)];
    Some(Decoded {
        config,
        params,
        rest,
    })
}
