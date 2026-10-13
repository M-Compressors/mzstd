//! Block headers and block regeneration (RFC 8878 section 3.1.1.2).
//!
//! A [`BlockDecoder`] holds the entropy state a frame carries from one
//! compressed block to the next: the literals Huffman table, the three
//! sequence tables and the repeat offsets.

use alloc::vec::Vec;

use super::literals::LiteralsDecoder;
use super::sequences::{BlockContext, SequenceTables};
use crate::decompressor::InvalidDataKind;
use crate::shared::format::{
    BLOCK_COMPRESSED, BLOCK_HEADER_SIZE, BLOCK_RAW, BLOCK_RLE, REP_NUM, REP_START,
};

/// How a block's content regenerates its output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BlockKind {
    /// Content copied verbatim.
    Raw,
    /// One byte repeated `size` times.
    Rle,
    /// Literals and sequences.
    Compressed,
}

/// A parsed three-byte block header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BlockHeader {
    /// Whether this block ends the frame.
    pub(crate) last: bool,
    /// The block type.
    pub(crate) kind: BlockKind,
    /// Content size, or the regenerated size of an RLE block.
    pub(crate) size: usize,
}

impl BlockHeader {
    /// Parses a header, rejecting the reserved type and sizes above the
    /// frame's `block_max`.
    pub(crate) fn parse(
        bytes: [u8; BLOCK_HEADER_SIZE],
        block_max: usize,
    ) -> Result<Self, InvalidDataKind> {
        let value = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0]);
        let kind = match (value >> 1) & 3 {
            BLOCK_RAW => BlockKind::Raw,
            BLOCK_RLE => BlockKind::Rle,
            BLOCK_COMPRESSED => BlockKind::Compressed,
            _ => return Err(InvalidDataKind::BlockType),
        };
        let size = (value >> 3) as usize;
        if size > block_max {
            return Err(InvalidDataKind::BlockSize);
        }
        Ok(Self {
            last: value & 1 != 0,
            kind,
            size,
        })
    }

    /// Bytes of content that follow the header.
    pub(crate) const fn content_size(&self) -> usize {
        match self.kind {
            BlockKind::Rle => 1,
            BlockKind::Raw | BlockKind::Compressed => self.size,
        }
    }
}

/// Entropy state of one frame's compressed blocks.
#[derive(Debug)]
pub(crate) struct BlockDecoder {
    literals: LiteralsDecoder,
    sequences: SequenceTables,
    reps: [u32; REP_NUM],
}

impl Default for BlockDecoder {
    fn default() -> Self {
        Self {
            literals: LiteralsDecoder::default(),
            sequences: SequenceTables::default(),
            reps: REP_START,
        }
    }
}

impl BlockDecoder {
    /// Heap bytes held between blocks.
    pub(crate) fn retained_bytes(&self) -> usize {
        self.literals.retained_bytes() + self.sequences.retained_bytes()
    }

    /// Restores the frame-start state, keeping allocations.
    pub(crate) fn reset(&mut self) {
        self.literals.reset();
        self.sequences.reset();
        self.reps = REP_START;
    }

    /// Regenerates one block, appending its output to `out`.
    ///
    /// `content` holds exactly [`BlockHeader::content_size`] bytes.
    pub(crate) fn decode(
        &mut self,
        header: &BlockHeader,
        content: &[u8],
        out: &mut Vec<u8>,
        context: BlockContext,
    ) -> Result<(), InvalidDataKind> {
        match header.kind {
            BlockKind::Raw => out.extend_from_slice(content),
            BlockKind::Rle => {
                let &byte = content.first().ok_or(InvalidDataKind::BlockSize)?;
                out.resize(out.len() + header.size, byte);
            }
            BlockKind::Compressed => {
                let (literals, used) = self.literals.decode(content, context.block_max)?;
                let sequences = content.get(used..).unwrap_or(&[]);
                let (count, header_size) = self.sequences.read_header(sequences)?;
                if count == 0 {
                    out.extend_from_slice(literals);
                } else {
                    let stream = sequences.get(header_size..).unwrap_or(&[]);
                    self.sequences.execute(
                        count,
                        stream,
                        literals,
                        &mut self.reps,
                        out,
                        context,
                    )?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTEXT: BlockContext = BlockContext {
        history_start: 0,
        window: 1 << 17,
        block_max: 1 << 17,
    };

    #[test]
    fn headers_carry_the_last_flag_type_and_size() {
        let raw = BlockHeader::parse([0x21, 0, 0], 100).expect("raw");
        assert_eq!(
            raw,
            BlockHeader {
                last: true,
                kind: BlockKind::Raw,
                size: 4
            }
        );
        assert_eq!(raw.content_size(), 4);
        let rle = BlockHeader::parse([0x2a, 0x01, 0], 100).expect("rle");
        assert_eq!(rle.kind, BlockKind::Rle);
        assert_eq!(rle.size, 37);
        assert!(!rle.last);
        assert_eq!(rle.content_size(), 1);
        let compressed = BlockHeader::parse([0x15, 0, 0], 100).expect("compressed");
        assert_eq!(compressed.kind, BlockKind::Compressed);
        assert_eq!(compressed.content_size(), 2);
    }

    #[test]
    fn reserved_types_and_oversized_blocks_are_rejected() {
        assert_eq!(
            BlockHeader::parse([0x06, 0, 0], 100),
            Err(InvalidDataKind::BlockType)
        );
        assert_eq!(
            BlockHeader::parse([0x00, 0x04, 0], 100),
            Err(InvalidDataKind::BlockSize)
        );
    }

    #[test]
    fn raw_rle_and_literal_only_blocks_regenerate() {
        let mut decoder = BlockDecoder::default();
        let mut out = Vec::new();
        let raw = BlockHeader::parse([0x18, 0, 0], 100).expect("raw");
        decoder
            .decode(&raw, b"abc", &mut out, CONTEXT)
            .expect("raw");
        let rle = BlockHeader::parse([0x1a, 0, 0], 100).expect("rle");
        decoder.decode(&rle, b"z", &mut out, CONTEXT).expect("rle");
        assert_eq!(out, b"abczzz");
        // Raw literals "hi" and no sequences.
        let compressed = BlockHeader::parse([0x1c, 0, 0], 100).expect("compressed");
        decoder
            .decode(&compressed, &[2 << 3, b'h', b'i', 0], &mut out, CONTEXT)
            .expect("compressed");
        assert_eq!(out, b"abczzzhi");
        assert_eq!(
            decoder.decode(&rle, b"", &mut out, CONTEXT),
            Err(InvalidDataKind::BlockSize)
        );
    }

    #[test]
    fn compressed_blocks_execute_sequences_and_keep_offsets() {
        let mut decoder = BlockDecoder::default();
        let mut out = b"abc".to_vec();
        // Raw literals "x", one sequence with RLE tables: literal length 1,
        // offset code 0 (repeat 1 = 1), match length code 0 (3 bytes).
        let content = [1 << 3, b'x', 1, 0x54, 1, 0, 0, 0x01];
        let header = BlockHeader {
            last: true,
            kind: BlockKind::Compressed,
            size: content.len(),
        };
        decoder
            .decode(&header, &content, &mut out, CONTEXT)
            .expect("valid");
        assert_eq!(out, b"abcxxxx");
        decoder.reset();
        assert_eq!(decoder.reps, REP_START);
    }
}
