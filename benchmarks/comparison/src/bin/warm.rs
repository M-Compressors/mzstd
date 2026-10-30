//! Warm one-shot profiling on Alice with reusable contexts and destinations.

#![forbid(unsafe_code)]

use mzstd::{Backend, Compressor, Decompressor, EncoderConfig, Level};
use std::{error::Error, hint::black_box, time::Instant};
use zstd_ffi::oracle::{self, Params};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [codec, implementation, corpus, level, iterations] = args.as_slice() else {
        return Err(
            "usage: warm encode|decode c-zstd|mzstd alice29|text-1m|file:PATH LEVEL ITERATIONS"
                .into(),
        );
    };
    let level: i32 = level.parse()?;
    let iterations: usize = iterations.parse()?;
    if iterations == 0 {
        return Err("expected a positive iteration count".into());
    }
    let text = include_bytes!("../../../../testdata/alice29.txt");
    let input = match corpus.as_str() {
        "alice29" => text.to_vec(),
        "text-1m" => text.iter().copied().cycle().take(1 << 20).collect(),
        other => match other.strip_prefix("file:") {
            Some(path) => std::fs::read(path)?,
            None => return Err("expected alice29, text-1m or file:PATH".into()),
        },
    };
    let backend = match std::env::var("MZSTD_BACKEND") {
        Ok(name) => Backend::available()
            .into_iter()
            .find(|backend| backend.name() == name)
            .ok_or("unsupported backend")?,
        Err(std::env::VarError::NotPresent) => Backend::default(),
        Err(error) => return Err(error.into()),
    };
    let frame = oracle::compress2(&input, &Params::level(level))?;
    let elapsed = match codec.as_str() {
        "encode" => {
            let config = EncoderConfig::default().with_level(Level::try_from(level)?);
            let mut ours = Compressor::builder(config).with_backend(backend).build()?;
            let mut theirs = oracle::Encoder::new(&Params::level(level));
            let mut output = vec![0; oracle::compress_bound(input.len())];
            let actual = ours.compress_to_slice(&input, &mut output)?;
            if output[..actual] != frame {
                return Err("Rust frame differs from C".into());
            }
            let actual = theirs.compress_into(&input, &mut output);
            if output[..actual] != frame {
                return Err("reused C context differs from cold C".into());
            }
            let start = Instant::now();
            match implementation.as_str() {
                "mzstd" => {
                    for _ in 0..iterations {
                        black_box(ours.compress_to_slice(black_box(&input), &mut output)?);
                    }
                }
                "c-zstd" => {
                    for _ in 0..iterations {
                        black_box(theirs.compress_into(black_box(&input), &mut output));
                    }
                }
                _ => return Err("expected mzstd or c-zstd".into()),
            }
            start.elapsed()
        }
        "decode" => {
            let mut ours = Decompressor::builder(Default::default())
                .with_backend(backend)
                .build();
            let mut theirs = oracle::Decoder::new();
            let mut output = vec![0; input.len()];
            let actual = ours.decompress_to_slice(&frame, &mut output)?;
            if actual != input.len() || output != input {
                return Err("Rust decoder did not restore input".into());
            }
            let actual = theirs.decompress_into(&frame, &mut output);
            if actual != input.len() || output != input {
                return Err("C decoder did not restore input".into());
            }
            let start = Instant::now();
            match implementation.as_str() {
                "mzstd" => {
                    for _ in 0..iterations {
                        black_box(ours.decompress_to_slice(black_box(&frame), &mut output)?);
                    }
                }
                "c-zstd" => {
                    for _ in 0..iterations {
                        black_box(theirs.decompress_into(black_box(&frame), &mut output));
                    }
                }
                _ => return Err("expected mzstd or c-zstd".into()),
            }
            start.elapsed()
        }
        _ => return Err("expected encode or decode".into()),
    };
    println!(
        "warm {codec} {implementation} {corpus} l{level}: {iterations} iterations, {:.1} ns/iteration, {} input bytes, {} frame bytes, backend {}",
        elapsed.as_nanos() as f64 / iterations as f64,
        input.len(),
        frame.len(),
        backend.name(),
    );
    Ok(())
}
