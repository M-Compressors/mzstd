//! Repeatable CPU/allocation profile, enabled only with the hotpath features.

use mzstd::{Compressor, EncoderConfig, Level};
use std::hint::black_box;

#[cfg_attr(all(feature = "hotpath", not(feature = "no_std")), hotpath::main)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let text = b"the quick brown fox jumps over the lazy dog 0123456789\n".repeat(1300);
    for number in [-5, 1, 2, 3, 4, 5, 6, 7, 8, 9, 12, 15, 16, 17, 19, 22] {
        let mut compressor =
            Compressor::new(EncoderConfig::default().with_level(Level::try_from(number)?))?;
        let mut output = Vec::with_capacity(Compressor::max_compressed_size(text.len())?);
        for _ in 0..10 {
            output.clear();
            compressor.compress_into(black_box(&text), &mut output)?;
            black_box(&output);
        }
    }
    for _ in 0..100 {
        let mut compressor = Compressor::new(EncoderConfig::default().with_level(Level::DEFAULT))?;
        black_box(compressor.compress(black_box(b"0. Zstandard is a"))?);
    }
    // An optional corpus path keeps the example usable from a published crate,
    // whose package deliberately does not include the test fixtures.
    let source = if let Some(path) = std::env::args_os().nth(1) {
        std::fs::read(path)?
    } else {
        text
    };
    for number in [1, 3, 9, 19] {
        let mut compressor =
            Compressor::new(EncoderConfig::default().with_level(Level::try_from(number)?))?;
        for _ in 0..10 {
            black_box(compressor.compress(black_box(&source))?);
        }
    }
    Ok(())
}
