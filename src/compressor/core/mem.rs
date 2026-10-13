//! Index-addressed views of the window's bytes, match counting and the
//! match finders' hash functions (`ZSTD_count`, `ZSTD_hashPtr` and
//! friends).
//!
//! The reference addresses history through two base pointers: indices at or
//! above `dictLimit` live in the current segment (`base + index`), indices
//! between `lowLimit` and `dictLimit` in the previous one
//! (`dictBase + index`). A [`Mem`] is one of those segments as a slice that
//! remembers which index its first byte has, so the ported match finders keep
//! the reference's index arithmetic and every byte they read is bounds
//! checked against the segment it belongs to.

/// One contiguous segment of history, addressed by window index.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Mem<'a> {
    bytes: &'a [u8],
    start: u32,
}

impl<'a> Mem<'a> {
    /// A segment whose first byte has index `start`.
    pub(crate) const fn new(bytes: &'a [u8], start: u32) -> Self {
        Self { bytes, start }
    }

    /// The same bytes, addressed from `start`.
    pub(crate) const fn rebased(self, start: u32) -> Self {
        Self {
            bytes: self.bytes,
            start,
        }
    }

    /// Index of the first byte.
    pub(crate) const fn start(&self) -> u32 {
        self.start
    }

    /// Index one past the last byte.
    pub(crate) const fn end(&self) -> u32 {
        self.start + self.bytes.len() as u32
    }

    #[inline]
    fn offset(&self, index: u32) -> usize {
        (index - self.start) as usize
    }

    /// The byte at `index`.
    #[inline]
    pub(crate) fn byte(&self, index: u32) -> u8 {
        self.bytes[self.offset(index)]
    }

    /// The bytes from `from` to `to`.
    #[inline]
    pub(crate) fn slice(&self, from: u32, to: u32) -> &'a [u8] {
        &self.bytes[self.offset(from)..self.offset(to)]
    }

    #[inline]
    fn array<const N: usize>(&self, index: u32) -> [u8; N] {
        let at = self.offset(index);
        match self.bytes.get(at..at + N) {
            Some(bytes) => {
                let mut out = [0u8; N];
                out.copy_from_slice(bytes);
                out
            }
            None => panic!(
                "read of {N} bytes at index {index} outside {}..{}",
                self.start,
                self.end()
            ),
        }
    }

    /// Little-endian `u32` at `index`.
    #[inline]
    pub(crate) fn read32(&self, index: u32) -> u32 {
        u32::from_le_bytes(self.array(index))
    }

    /// Little-endian `u64` at `index`.
    #[inline]
    pub(crate) fn read64(&self, index: u32) -> u64 {
        u64::from_le_bytes(self.array(index))
    }

    /// Length of the common run of `self[ip..limit]` and `other[m..]`
    /// (`ZSTD_count`); `other` must hold at least as many bytes from `m`.
    #[inline]
    pub(crate) fn count_with(&self, ip: u32, other: &Mem<'_>, m: u32, limit: u32) -> usize {
        if ip >= limit {
            return 0;
        }
        let a = &self.bytes[self.offset(ip)..self.offset(limit)];
        let b = &other.bytes[other.offset(m)..];
        a.iter().zip(b).take_while(|(x, y)| x == y).count()
    }

    /// `ZSTD_count` within this segment.
    #[inline]
    pub(crate) fn count(&self, ip: u32, m: u32, limit: u32) -> usize {
        self.count_with(ip, self, m, limit)
    }
}

/// The two segments of history visible to one block.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Window<'a> {
    /// Indices `[dictLimit, block end)`.
    pub(crate) prefix: Mem<'a>,
    /// Indices `[lowLimit, dictLimit)`; empty without an external dictionary.
    pub(crate) ext: Mem<'a>,
}

impl Window<'_> {
    /// The byte at window index `index`, from whichever segment holds it.
    #[inline]
    pub(crate) fn byte_at(&self, index: u32) -> u8 {
        if index < self.prefix.start() {
            self.ext.byte(index)
        } else {
            self.prefix.byte(index)
        }
    }

    /// The `u32` at window index `index`; it must not straddle the
    /// boundary between segments.
    #[inline]
    pub(crate) fn read32_at(&self, index: u32) -> u32 {
        if index < self.prefix.start() {
            self.ext.read32(index)
        } else {
            self.prefix.read32(index)
        }
    }

    /// Common length of the runs at window indices `a` and `b`, stopping
    /// before `limit` for `a`. Indices below the prefix read the external
    /// dictionary, and a run leaving it continues at the prefix start —
    /// the reference's `ZSTD_count` / `ZSTD_count_2segments` pair seen as
    /// one contiguous index space.
    pub(crate) fn count_virtual(&self, a: u32, b: u32, limit: u32) -> usize {
        let boundary = self.prefix.start();
        if a >= boundary && b >= boundary {
            return self.prefix.count(a, b, limit);
        }
        if a >= boundary {
            return self.count_2segments(a, b, limit, boundary, boundary);
        }
        let mut n = 0u32;
        while a + n < limit && self.byte_at(a + n) == self.byte_at(b + n) {
            n += 1;
        }
        n as usize
    }

    /// `ZSTD_count_2segments`: a match starting in `ext` at `m` continues
    /// into the prefix at `prefix_start` once it reaches `m_end`.
    pub(crate) fn count_2segments(
        &self,
        ip: u32,
        m: u32,
        i_end: u32,
        m_end: u32,
        prefix_start: u32,
    ) -> usize {
        let v_end = (ip + (m_end - m)).min(i_end);
        let match_length = self.prefix.count_with(ip, &self.ext, m, v_end);
        if m + match_length as u32 != m_end {
            return match_length;
        }
        match_length
            + self
                .prefix
                .count(ip + match_length as u32, prefix_start, i_end)
    }
}

const PRIME_3_BYTES: u32 = 506_832_829;
const PRIME_4_BYTES: u32 = 2_654_435_761;
const PRIME_5_BYTES: u64 = 889_523_592_379;
const PRIME_6_BYTES: u64 = 227_718_039_650_203;
const PRIME_7_BYTES: u64 = 58_295_818_150_454_627;
/// `prime8bytes`, also the rolling hash multiplier.
pub(crate) const PRIME_8_BYTES: u64 = 0xCF1B_BCDC_B7A5_6463;

/// `ZSTD_hash3`.
#[inline]
pub(crate) const fn hash3(u: u32, h: u32, s: u32) -> usize {
    ((((u << 8).wrapping_mul(PRIME_3_BYTES)) ^ s) >> (32 - h)) as usize
}

#[inline]
const fn hash4(u: u32, h: u32, s: u32) -> usize {
    ((u.wrapping_mul(PRIME_4_BYTES) ^ s) >> (32 - h)) as usize
}

#[inline]
const fn hash5(u: u64, h: u32, s: u64) -> usize {
    (((u << 24).wrapping_mul(PRIME_5_BYTES) ^ s) >> (64 - h)) as usize
}

#[inline]
const fn hash6(u: u64, h: u32, s: u64) -> usize {
    (((u << 16).wrapping_mul(PRIME_6_BYTES) ^ s) >> (64 - h)) as usize
}

#[inline]
const fn hash7(u: u64, h: u32, s: u64) -> usize {
    (((u << 8).wrapping_mul(PRIME_7_BYTES) ^ s) >> (64 - h)) as usize
}

#[inline]
const fn hash8(u: u64, h: u32, s: u64) -> usize {
    ((u.wrapping_mul(PRIME_8_BYTES) ^ s) >> (64 - h)) as usize
}

/// `ZSTD_hashPtrSalted` over the bytes at `index` of `mem`.
#[inline]
pub(crate) fn hash_salted(mem: &Mem<'_>, index: u32, h_bits: u32, mls: u32, salt: u64) -> usize {
    match mls {
        5 => hash5(mem.read64(index), h_bits, salt),
        6 => hash6(mem.read64(index), h_bits, salt),
        7 => hash7(mem.read64(index), h_bits, salt),
        8 => hash8(mem.read64(index), h_bits, salt),
        _ => hash4(mem.read32(index), h_bits, salt as u32),
    }
}

/// `ZSTD_hashPtr` over the bytes at `index` of `mem`.
#[inline]
pub(crate) fn hash_ptr(mem: &Mem<'_>, index: u32, h_bits: u32, mls: u32) -> usize {
    hash_salted(mem, index, h_bits, mls, 0)
}

/// `ZSTD_hash3Ptr` over the bytes at `index` of `mem`.
#[inline]
pub(crate) fn hash3_ptr(mem: &Mem<'_>, index: u32, h_bits: u32) -> usize {
    hash3(mem.read32(index), h_bits, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_are_little_endian_and_index_addressed() {
        let bytes = [1u8, 2, 3, 4, 5, 6, 7, 8, 9];
        let mem = Mem::new(&bytes, 100);
        assert_eq!(mem.byte(100), 1);
        assert_eq!(mem.read32(100), 0x0403_0201);
        assert_eq!(mem.read64(101), 0x0908_0706_0504_0302);
        assert_eq!(mem.slice(102, 104), &[3, 4]);
        assert_eq!((mem.start(), mem.end()), (100, 109));
    }

    #[test]
    #[should_panic(expected = "outside")]
    fn reading_past_a_segment_panics() {
        let bytes = [0u8; 4];
        Mem::new(&bytes, 0).read64(0);
    }

    #[test]
    fn counts_stop_at_the_first_difference_or_limit() {
        let bytes = b"abcabcabd";
        let mem = Mem::new(bytes, 0);
        assert_eq!(mem.count(3, 0, 9), 5);
        assert_eq!(mem.count(3, 0, 5), 2);
        assert_eq!(mem.count(9, 0, 9), 0);
    }

    #[test]
    fn two_segment_counts_continue_into_the_prefix() {
        let ext = b"xyab";
        let prefix = b"abab_";
        let window = Window {
            prefix: Mem::new(prefix, 10),
            ext: Mem::new(ext, 6),
        };
        // "ab" at ext index 8, then the prefix from index 10 continues "ab".
        assert_eq!(window.count_2segments(12, 8, 15, 10, 10), 2);
        assert_eq!(window.count_2segments(10, 8, 15, 10, 10), 4);
    }

    #[test]
    fn hashes_use_the_requested_width() {
        let bytes = [7u8; 16];
        let mem = Mem::new(&bytes, 0);
        for mls in 3..=8 {
            assert!(hash_ptr(&mem, 0, 12, mls) < 1 << 12);
        }
        assert!(hash3_ptr(&mem, 0, 10) < 1 << 10);
        assert_ne!(
            hash_salted(&mem, 0, 16, 5, 1 << 63),
            hash_salted(&mem, 0, 16, 5, 0)
        );
    }
}
