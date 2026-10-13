//! The streaming decoder's output buffer, which doubles as the window.
//!
//! Blocks are regenerated onto the end of one `Vec`; the caller's output is
//! filled from its undelivered tail. Before each block, once everything is
//! delivered, the buffer drops all but the last window of history, but only
//! after it has grown by at least another window (or block) beyond it, so
//! each byte is moved at most once on average and history never has to wrap.

use ::core::fmt;
use alloc::vec::Vec;

use crate::shared::format::BLOCK_SIZE_MAX;

/// Regenerated output and the history blocks reference.
#[derive(Default)]
pub(crate) struct History {
    buffer: Vec<u8>,
    delivered: usize,
}

impl fmt::Debug for History {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("History")
            .field("len", &self.buffer.len())
            .field("delivered", &self.delivered)
            .finish()
    }
}

impl History {
    /// Forgets everything at a frame start, keeping the allocation.
    pub(crate) fn reset(&mut self) {
        self.buffer.clear();
        self.delivered = 0;
    }

    /// Bytes regenerated but not yet delivered.
    pub(crate) const fn pending(&self) -> usize {
        self.buffer.len() - self.delivered
    }

    /// Copies pending bytes into `output`, returning how many.
    pub(crate) fn deliver(&mut self, output: &mut [u8]) -> usize {
        let pending = &self.buffer[self.delivered..];
        let count = pending.len().min(output.len());
        output[..count].copy_from_slice(&pending[..count]);
        self.delivered += count;
        count
    }

    /// Prepares for the next block of a frame with `window` bytes of
    /// history and returns the buffer to append it to. All output must have
    /// been delivered.
    pub(crate) fn prepare(&mut self, window: u64) -> &mut Vec<u8> {
        debug_assert_eq!(self.pending(), 0);
        let keep = usize::try_from(window).unwrap_or(usize::MAX);
        let slack = keep.max(BLOCK_SIZE_MAX);
        if self.buffer.len() > keep.saturating_add(slack) {
            let drop = self.buffer.len() - keep;
            self.buffer.drain(..drop);
            self.delivered = self.buffer.len();
        }
        &mut self.buffer
    }

    /// Drops the buffer's allocation; nothing may be pending.
    pub(crate) fn release(&mut self) {
        *self = Self::default();
    }

    /// Heap bytes the buffer holds.
    pub(crate) fn retained_bytes(&self) -> usize {
        self.buffer.capacity()
    }

    /// Bytes the buffer currently holds, for tests and accounting.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.buffer.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_drains_the_pending_tail() {
        let mut history = History::default();
        history.prepare(1024).extend_from_slice(b"hello");
        assert_eq!(history.pending(), 5);
        let mut out = [0u8; 3];
        assert_eq!(history.deliver(&mut out), 3);
        assert_eq!(&out, b"hel");
        assert_eq!(history.deliver(&mut out), 2);
        assert_eq!(&out[..2], b"lo");
        assert_eq!(history.deliver(&mut out), 0);
        assert!(alloc::format!("{history:?}").contains("len: 5"));
    }

    #[test]
    fn history_is_compacted_only_past_a_window_of_slack() {
        let mut history = History::default();
        let window = BLOCK_SIZE_MAX as u64;
        let block = alloc::vec![7u8; BLOCK_SIZE_MAX];
        history.prepare(window).extend_from_slice(&block);
        history.deliver(&mut alloc::vec![0; BLOCK_SIZE_MAX]);
        history.prepare(window).extend_from_slice(&block);
        history.deliver(&mut alloc::vec![0; BLOCK_SIZE_MAX]);
        // Exactly window + slack: kept.
        history.prepare(window);
        assert_eq!(history.len(), 2 * BLOCK_SIZE_MAX);
        history.prepare(window).push(1);
        history.deliver(&mut [0]);
        history.prepare(window);
        assert_eq!(history.len(), BLOCK_SIZE_MAX);
        assert_eq!(history.pending(), 0);
        history.reset();
        assert_eq!(history.len(), 0);
    }
}
