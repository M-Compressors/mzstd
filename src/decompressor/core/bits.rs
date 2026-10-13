//! Backward bit reader for Huffman and FSE bitstreams.
//!
//! Zstandard writes these streams forward and ends them with a single `1` bit
//! followed by zero padding up to the byte boundary, so the decoder starts at
//! the last byte, skips the padding and the marker bit, and reads towards the
//! first byte. Reading past the start yields zero bits and is remembered:
//! [`ReverseBitReader::overflowed`] reports it and
//! [`ReverseBitReader::is_exhausted`] only holds when every bit was consumed
//! exactly, which is how the reference decoder validates stream ends.

use crate::shared::format::highbit32;

/// A backward reader over one bitstream.
#[derive(Clone, Debug)]
pub(crate) struct ReverseBitReader<'a> {
    src: &'a [u8],
    /// Number of unread bits; negative once reads ran past the first byte.
    position: i64,
}

/// Loads up to eight bytes from `src[start..]` as a little-endian word,
/// padding with zeros past the end.
fn load_word(src: &[u8], start: usize) -> u64 {
    let tail = src.get(start..).unwrap_or(&[]);
    if let Some(word) = tail.first_chunk::<8>() {
        return u64::from_le_bytes(*word);
    }
    let mut word = [0u8; 8];
    word[..tail.len()].copy_from_slice(tail);
    u64::from_le_bytes(word)
}

impl<'a> ReverseBitReader<'a> {
    /// Opens `src`, or returns `None` when it is empty or its last byte holds
    /// no end marker.
    pub(crate) fn new(src: &'a [u8]) -> Option<Self> {
        let &last = src.last()?;
        if last == 0 {
            return None;
        }
        let whole = i64::try_from(src.len() - 1).ok()?.checked_mul(8)?;
        Some(Self {
            src,
            position: whole + i64::from(highbit32(u32::from(last))),
        })
    }

    /// Bits `[start, start + count)` of the stream as an integer whose most
    /// significant bit is the highest-numbered one; bits below zero read as
    /// zeros. `count` must not exceed 56.
    fn bits_at(&self, start: i64, count: u32) -> u64 {
        debug_assert!(count <= 56);
        if count == 0 {
            return 0;
        }
        let end = start + i64::from(count);
        if end <= 0 {
            return 0;
        }
        let (low, shift_in) = if start < 0 {
            (0, (-start) as u32)
        } else {
            (start, 0)
        };
        let width = (end - low) as u32;
        let byte = (low / 8) as usize;
        let word = load_word(self.src, byte) >> (low % 8);
        (word & ((1u64 << width) - 1)) << shift_in
    }

    /// The next `count` bits without consuming them; `count <= 56`.
    pub(crate) fn peek(&self, count: u32) -> u64 {
        self.bits_at(self.position - i64::from(count), count)
    }

    /// Consumes `count` bits.
    pub(crate) fn consume(&mut self, count: u32) {
        self.position -= i64::from(count);
    }

    /// Reads and consumes `count` bits; `count <= 56`.
    pub(crate) fn read(&mut self, count: u32) -> u64 {
        let value = self.peek(count);
        self.consume(count);
        value
    }

    /// Whether more bits were consumed than the stream holds.
    pub(crate) const fn overflowed(&self) -> bool {
        self.position < 0
    }

    /// Whether every bit was consumed, and no more.
    pub(crate) const fn is_exhausted(&self) -> bool {
        self.position == 0
    }
}

/// A forward little-endian bit reader for FSE table descriptions.
///
/// Bits past the end read as zeros; [`ForwardBitReader::bytes_used`] tells
/// the caller how many bytes the read prefix spans so it can reject
/// descriptions that ran past their input.
#[derive(Clone, Debug)]
pub(crate) struct ForwardBitReader<'a> {
    src: &'a [u8],
    position: usize,
}

impl<'a> ForwardBitReader<'a> {
    /// Starts reading at the first bit of `src`.
    pub(crate) const fn new(src: &'a [u8]) -> Self {
        Self { src, position: 0 }
    }

    /// The next `count` bits, lowest first; `count <= 32`.
    pub(crate) fn peek(&self, count: u32) -> u32 {
        debug_assert!(count <= 32);
        let word = load_word(self.src, self.position / 8) >> (self.position % 8);
        (word & ((1u64 << count) - 1)) as u32
    }

    /// Consumes `count` bits.
    pub(crate) const fn consume(&mut self, count: u32) {
        self.position += count as usize;
    }

    /// Reads and consumes `count` bits; `count <= 32`.
    pub(crate) fn read(&mut self, count: u32) -> u32 {
        let value = self.peek(count);
        self.consume(count);
        value
    }

    /// Bytes spanned by the bits read so far, rounded up.
    pub(crate) const fn bytes_used(&self) -> usize {
        self.position.div_ceil(8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_unterminated_streams_are_rejected() {
        assert!(ReverseBitReader::new(&[]).is_none());
        assert!(ReverseBitReader::new(&[0x12, 0x00]).is_none());
    }

    #[test]
    fn the_marker_bit_and_padding_are_skipped() {
        // 0b0000_0101: marker at bit 2, two payload bits `01`.
        let mut bits = ReverseBitReader::new(&[0b0000_0101]).expect("terminated");
        assert_eq!(bits.read(1), 0);
        assert_eq!(bits.read(1), 1);
        assert!(bits.is_exhausted());
        assert!(!bits.overflowed());
    }

    #[test]
    fn reads_cross_byte_boundaries_from_the_end() {
        // Bits from the top: byte 1 = 0b1_0110011 (marker then 0110011),
        // byte 0 = 0b1010_0101.
        let src = [0b1010_0101, 0b1011_0011];
        let mut bits = ReverseBitReader::new(&src).expect("terminated");
        assert_eq!(bits.read(7), 0b011_0011);
        assert_eq!(bits.read(4), 0b1010);
        assert_eq!(bits.read(4), 0b0101);
        assert!(bits.is_exhausted());
    }

    #[test]
    fn reading_past_the_start_yields_zeros_and_overflows() {
        let mut bits = ReverseBitReader::new(&[0b0000_0111]).expect("terminated");
        assert_eq!(bits.peek(4), 0b1100);
        assert_eq!(bits.read(4), 0b1100);
        assert!(bits.overflowed());
        assert!(!bits.is_exhausted());
        assert_eq!(bits.read(8), 0);
    }

    #[test]
    fn wide_reads_use_whole_words() {
        let src: [u8; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 0x80];
        let mut bits = ReverseBitReader::new(&src).expect("terminated");
        assert_eq!(bits.read(7), 0);
        assert_eq!(bits.read(32), 0x0b0a_0908);
        assert_eq!(bits.read(32), 0x0706_0504);
        assert_eq!(bits.read(24), 0x03_0201);
        assert!(bits.is_exhausted());
    }

    #[test]
    fn forward_reads_are_little_endian_and_count_bytes() {
        let mut bits = ForwardBitReader::new(&[0b1010_0101, 0b0000_0011]);
        assert_eq!(bits.read(4), 0b0101);
        assert_eq!(bits.bytes_used(), 1);
        assert_eq!(bits.peek(6), 0b11_1010);
        assert_eq!(bits.read(6), 0b11_1010);
        assert_eq!(bits.bytes_used(), 2);
        assert_eq!(bits.read(16), 0);
        assert_eq!(bits.bytes_used(), 4);
    }
}
