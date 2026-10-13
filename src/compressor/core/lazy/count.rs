//! `ZSTD_count` for the lazy match finders: the length of the common run of
//! two byte windows, eight bytes at a time and then in native vectors.
//!
//! The reference compares machine words with an XOR and a trailing-zero
//! count. Most candidates the lazy finders measure differ within the first
//! word, so that word is settled inline; only a match that runs through it
//! continues into the word and vector loops, which are where the long matches
//! of repetitive input spend their time. Every path returns the exact common
//! length, so the backend never changes a frame.

use fearless_simd::{Simd, SimdBase, SimdMask, u8x16, u8x32, u8x64};

/// Words compared before the vector loop is entered: short matches are the
/// common case, and a vector loop's setup costs more than it saves on them.
const SCALAR_PREFIX_WORDS: usize = 2;

/// Common length of `src[a..limit]` and `src[b..]` for `b < a`
/// (`ZSTD_count(src + a, src + b, src + limit)`).
///
/// # Panics
///
/// When `a > limit` or `limit > src.len()`: the reference's pointers would
/// leave the window, which its index invariants rule out.
#[inline(always)]
pub(super) fn count<S: Simd>(simd: S, src: &[u8], a: usize, b: usize, limit: usize) -> usize {
    let current = &src[a..limit];
    let Some(earlier) = src.get(b..b + current.len()) else {
        return common_bytes(&src[b..], current);
    };
    common_run(simd, current, earlier)
}

/// Common length of two windows, at most the shorter one's length.
#[inline(always)]
pub(super) fn common_run<S: Simd>(simd: S, x: &[u8], y: &[u8]) -> usize {
    let limit = x.len().min(y.len());
    let (x, y) = (&x[..limit], &y[..limit]);
    if let (Some(x_word), Some(y_word)) = (x.first_chunk::<8>(), y.first_chunk::<8>()) {
        let difference = u64::from_le_bytes(*x_word) ^ u64::from_le_bytes(*y_word);
        if difference != 0 {
            return (difference.trailing_zeros() >> 3) as usize;
        }
        return 8 + long_run(simd, &x[8..], &y[8..]);
    }
    common_bytes(x, y)
}

/// [`common_run`] past a first word that matched in full, kept out of line
/// so the candidates that differ within that word pay for none of its
/// setup.
#[fearless_simd_macros::simd]
#[inline(never)]
fn long_run<S: Simd>(simd: S, x: &[u8], y: &[u8]) -> usize {
    let limit = x.len();
    let prefix = limit.min(8 * SCALAR_PREFIX_WORDS) & !7;
    let mut matched = common_words(&x[..prefix], &y[..prefix]);
    if matched < prefix {
        return matched;
    }

    let stride = native_stride::<S>();
    let whole_vectors = (limit - matched) - (limit - matched) % stride;
    let vectored = common_vectors(simd, &x[matched..], &y[matched..]);
    matched += vectored;
    if vectored < whole_vectors {
        return matched;
    }

    let whole_words = (limit - matched) & !7;
    let words = common_words(
        &x[matched..matched + whole_words],
        &y[matched..matched + whole_words],
    );
    matched += words;
    if words < whole_words || matched == limit {
        return matched;
    }

    // Fewer than eight bytes are left: one more word that ends at the limit
    // overlaps bytes already found equal, so its first difference is the
    // first difference of the tail.
    if let (Some(x_last), Some(y_last)) = (x.last_chunk::<8>(), y.last_chunk::<8>()) {
        let difference = u64::from_le_bytes(*x_last) ^ u64::from_le_bytes(*y_last);
        return if difference == 0 {
            limit
        } else {
            limit - 8 + (difference.trailing_zeros() >> 3) as usize
        };
    }
    matched + common_bytes(&x[matched..], &y[matched..])
}

/// Common length of two windows compared one byte at a time.
#[inline(always)]
fn common_bytes(x: &[u8], y: &[u8]) -> usize {
    x.iter().zip(y).take_while(|(a, b)| a == b).count()
}

/// Common length of two equal-length windows eight bytes at a time, or the
/// length of their whole-word prefix when every word matches.
#[inline(always)]
fn common_words(x: &[u8], y: &[u8]) -> usize {
    let (x_words, _) = x.as_chunks::<8>();
    let (y_words, _) = y.as_chunks::<8>();
    let mut matched = 0usize;
    for (x_word, y_word) in x_words.iter().zip(y_words) {
        let difference = u64::from_le_bytes(*x_word) ^ u64::from_le_bytes(*y_word);
        if difference != 0 {
            return matched + (difference.trailing_zeros() >> 3) as usize;
        }
        matched += 8;
    }
    matched
}

/// Defines the vector loop of one width.
macro_rules! vector_run {
    ($name:ident, $vector:ident, $lanes:literal) => {
        /// Common length of two windows one vector of the macro's width at a
        /// time, or the length of their whole-vector prefix.
        #[inline(always)]
        fn $name<S: Simd>(simd: S, x: &[u8], y: &[u8]) -> usize {
            let (x_vectors, _) = x.as_chunks::<$lanes>();
            let (y_vectors, _) = y.as_chunks::<$lanes>();
            let mut matched = 0usize;
            for (x_lanes, y_lanes) in x_vectors.iter().zip(y_vectors) {
                let equal = $vector::<S>::load_array_ref(simd, x_lanes)
                    .simd_eq($vector::<S>::load_array_ref(simd, y_lanes));
                if !equal.all_true() {
                    return matched + equal.to_bitmask().trailing_ones() as usize;
                }
                matched += $lanes;
            }
            matched
        }
    };
}

vector_run!(common_vectors_16, u8x16, 16);
vector_run!(common_vectors_32, u8x32, 32);
vector_run!(common_vectors_64, u8x64, 64);

/// Bytes [`common_vectors`] advances per step on `S`.
#[inline(always)]
const fn native_stride<S: Simd>() -> usize {
    match <S::u8s as SimdBase<S>>::LEN {
        16 => 16,
        32 => 32,
        64 => 64,
        _ => 1,
    }
}

/// The vector loop at `S`'s native width; the match folds at
/// monomorphisation.
#[inline(always)]
fn common_vectors<S: Simd>(simd: S, x: &[u8], y: &[u8]) -> usize {
    match <S::u8s as SimdBase<S>>::LEN {
        16 => common_vectors_16(simd, x, y),
        32 => common_vectors_32(simd, x, y),
        64 => common_vectors_64(simd, x, y),
        // A width without a loop scans single bytes, which keeps the
        // caller's "every step matched" test exact with a stride of one.
        _ => common_bytes(x, y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Backend;
    use alloc::vec::Vec;

    fn reference(src: &[u8], a: usize, b: usize, limit: usize) -> usize {
        let mut n = 0;
        while a + n < limit && src[a + n] == src[b + n] {
            n += 1;
        }
        n
    }

    #[test]
    fn counts_equal_the_bytewise_reference_on_every_backend() {
        // A run long enough for every vector width, then a difference at
        // each position and limits at each boundary.
        let mut src: Vec<u8> = (0..400u32).map(|i| (i % 7) as u8).collect();
        src.extend((0..400u32).map(|i| (i % 7) as u8));
        for backend in Backend::available() {
            for diff in [
                0usize, 1, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 130, 399,
            ] {
                let mut data = src.clone();
                data[400 + diff] ^= 0x80;
                for limit in [400, 401, 407, 408, 409, 424, 433, 464, 528, 799, 800] {
                    let expected = reference(&data, 400, 0, limit);
                    let got = fearless_simd::dispatch!(backend.0, simd => count(simd, &data, 400, 0, limit));
                    assert_eq!(got, expected, "{backend} diff {diff} limit {limit}");
                }
            }
        }
    }

    #[test]
    fn a_short_earlier_window_stops_the_count() {
        let src = [5u8; 20];
        for backend in Backend::available() {
            // `b + len` past the end can only happen when `b > a`.
            let got = fearless_simd::dispatch!(backend.0, simd => count(simd, &src, 2, 10, 20));
            assert_eq!(got, 10, "{backend}");
            let got =
                fearless_simd::dispatch!(backend.0, simd => common_run(simd, &src[..3], &src));
            assert_eq!(got, 3, "{backend}");
        }
    }
}
