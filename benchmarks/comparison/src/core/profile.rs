use super::{Error, Result, c_compress, compress, corpora, decoder::decompress, validate};
use std::hint::black_box;
use std::time::Instant;

pub(crate) fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [codec, implementation, corpus, level, iterations] = args.as_slice() else {
        return Err("usage: profile encode|decode c-zstd|mzstd CORPUS LEVEL ITERATIONS".into());
    };
    let level: i32 = level.parse()?;
    let iterations: usize = iterations.parse()?;
    if iterations == 0 || !matches!(codec.as_str(), "encode" | "decode") {
        return Err("expected encode|decode and a positive iteration count".into());
    }
    let (name, input) = corpora()
        .into_iter()
        .find(|(name, _)| *name == corpus)
        .ok_or("unknown corpus")?;
    let frame = c_compress(&input, level)?;
    let elapsed = if codec == "encode" {
        let actual = compress(implementation, &input, level)?;
        if actual != frame {
            return Err(Error::NotIdentical {
                corpus: name,
                level,
            }
            .into());
        }
        validate(&input, &actual)?;
        let start = Instant::now();
        profile_encode(implementation, &input, level, iterations)?;
        start.elapsed()
    } else {
        if decompress(implementation, &frame, input.len())? != input {
            return Err(Error::RoundTrip.into());
        }
        let start = Instant::now();
        profile_decode(implementation, &frame, input.len(), iterations)?;
        start.elapsed()
    };
    println!(
        "{codec} {implementation} {name} l{level}: {iterations} iterations, {:.1} ns/iteration, {} input bytes, {} frame bytes",
        elapsed.as_nanos() as f64 / iterations as f64,
        input.len(),
        frame.len(),
    );
    Ok(())
}

// Stable, uninlined boundaries let Callgrind collect only the repeated
// operation with --collect-atstart=no --toggle-collect='*profile_encode*'
// (or profile_decode), excluding corpus construction and validation.
#[inline(never)]
fn profile_encode(implementation: &str, input: &[u8], level: i32, iterations: usize) -> Result<()> {
    for _ in 0..iterations {
        black_box(compress(implementation, black_box(input), level)?);
    }
    Ok(())
}

#[inline(never)]
fn profile_decode(
    implementation: &str,
    frame: &[u8],
    size: usize,
    iterations: usize,
) -> Result<()> {
    for _ in 0..iterations {
        black_box(decompress(implementation, black_box(frame), size)?);
    }
    Ok(())
}
