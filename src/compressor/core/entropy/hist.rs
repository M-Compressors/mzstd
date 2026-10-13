//! Symbol histograms (`hist.c`).

use crate::compressor::core::error::CoreError as EntropyError;

/// Counts the symbols of `src` into `count[..=*max_symbol_value]`.
///
/// On return `*max_symbol_value` is the largest symbol present (`0` for an
/// empty input) and the result is the largest count. With `check`, a symbol
/// above the incoming bound is an error, as in `HIST_count_wksp`; without it
/// the caller guarantees none exists, as in `HIST_countFast_wksp`.
///
/// # Errors
///
/// [`EntropyError::MaxSymbolValueTooSmall`] when `check` is set and `src`
/// holds a symbol above the incoming bound.
pub(crate) fn count(
    count: &mut [u32],
    max_symbol_value: &mut u32,
    src: &[u8],
    check: bool,
) -> Result<u32, EntropyError> {
    let bound = *max_symbol_value as usize;
    if src.is_empty() {
        count[..=bound].fill(0);
        *max_symbol_value = 0;
        return Ok(0);
    }
    let mut full = [0u32; 256];
    for &byte in src {
        full[usize::from(byte)] += 1;
    }
    let mut max = 255usize;
    while full[max] == 0 {
        max -= 1;
    }
    if check && max > bound {
        return Err(EntropyError::MaxSymbolValueTooSmall);
    }
    debug_assert!(max <= bound, "trusted input above its bound");
    count[..=bound].copy_from_slice(&full[..=bound]);
    *max_symbol_value = max as u32;
    Ok(full.iter().copied().max().unwrap_or(0))
}

/// Adds the symbols of `src` to `count` (`HIST_add`).
pub(crate) fn add(count: &mut [u32; 256], src: &[u8]) {
    for &byte in src {
        count[usize::from(byte)] += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_report_the_largest_symbol_and_count() {
        let mut counts = [0u32; 256];
        let mut max = 255;
        let largest = count(&mut counts, &mut max, b"aabbbz", true).expect("count");
        assert_eq!(largest, 3);
        assert_eq!(max, u32::from(b'z'));
        assert_eq!(counts[usize::from(b'b')], 3);
    }

    #[test]
    fn empty_input_resets_the_bound() {
        let mut counts = [7u32; 8];
        let mut max = 7;
        assert_eq!(count(&mut counts, &mut max, &[], true), Ok(0));
        assert_eq!(max, 0);
        assert_eq!(counts, [0; 8]);
    }

    #[test]
    fn a_symbol_above_a_checked_bound_is_an_error() {
        let mut counts = [0u32; 4];
        let mut max = 3;
        assert_eq!(
            count(&mut counts, &mut max, &[1, 9], true),
            Err(EntropyError::MaxSymbolValueTooSmall)
        );
    }

    #[test]
    fn add_accumulates() {
        let mut counts = [0u32; 256];
        add(&mut counts, b"aa");
        add(&mut counts, b"a");
        assert_eq!(counts[usize::from(b'a')], 3);
    }
}
