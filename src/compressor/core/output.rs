//! Initialized destinations for frame compression. A caller-owned slice
//! retains its exact capacity; a Vec initializes only the space needed by
//! the next block and reuses its unwritten tail between blocks.

use alloc::vec::Vec;

use super::error::CoreError;

pub(crate) trait Output {
    /// Writable tail at `position`, relative to the current call's
    /// destination. `needed` is a growth hint, not a capacity requirement:
    /// the slice adapter keeps the caller's exact limit, and the encoder
    /// performs its usual capacity checks on the returned slice.
    fn prepare(&mut self, position: usize, needed: usize) -> Result<&mut [u8], CoreError>;
}

pub(crate) struct SliceOutput<'a>(pub(crate) &'a mut [u8]);

impl Output for SliceOutput<'_> {
    #[inline]
    fn prepare(&mut self, position: usize, _needed: usize) -> Result<&mut [u8], CoreError> {
        self.0.get_mut(position..).ok_or(CoreError::DstSizeTooSmall)
    }
}

pub(crate) struct VecOutput<'a> {
    pub(crate) buffer: &'a mut Vec<u8>,
    pub(crate) start: usize,
    /// End of the frame's compressBound reservation, including the prefix.
    pub(crate) limit: usize,
}

impl Output for VecOutput<'_> {
    fn prepare(&mut self, position: usize, needed: usize) -> Result<&mut [u8], CoreError> {
        let position = self
            .start
            .checked_add(position)
            .ok_or(CoreError::DstSizeTooSmall)?;
        if position > self.limit {
            return Err(CoreError::DstSizeTooSmall);
        }
        let end = position.saturating_add(needed).min(self.limit);
        if end > self.buffer.len() {
            self.buffer.resize(end, 0);
        }
        Ok(&mut self.buffer[position..])
    }
}
