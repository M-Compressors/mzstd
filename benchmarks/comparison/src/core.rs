use criterion::{BenchmarkId, Criterion, Throughput};
use mzstd::{Compressor, EncoderConfig, Level};
use std::hint::black_box;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;
use zstd_ffi as ffi;

pub(super) mod decoder;
pub(super) mod profile;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Implementations in report order; the C library is the reference.
const ENCODERS: [&str; 2] = ["c-zstd", "mzstd"];

/// Levels timed: two accelerated levels and every regular level.
const LEVELS: [i32; 24] = [
    -5, -1, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
];

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("C compression failed")]
    CEncode,
    #[error("C decompression failed")]
    CDecode,
    #[error("compressed output did not round-trip through the C decoder")]
    RoundTrip,
    #[error("mzstd and zstd wrote different frames for {corpus} at level {level}")]
    NotIdentical { corpus: &'static str, level: i32 },
    #[error("unknown implementation: {0}")]
    Unknown(String),
}

/// Compresses `input` at `level` through one implementation's cold one-shot
/// API: construction, allocation, compression and disposal.
fn compress(encoder: &str, input: &[u8], level: i32) -> Result<Vec<u8>> {
    match encoder {
        "c-zstd" => c_compress(input, level),
        "mzstd" => Ok(Compressor::new(
            EncoderConfig::default().with_level(Level::try_from(level)?),
        )?
        .compress(input)?),
        _ => Err(Error::Unknown(encoder.to_owned()).into()),
    }
}

/// `ZSTD_compress2` on a fresh context into a `ZSTD_compressBound` buffer.
fn c_compress(input: &[u8], level: i32) -> Result<Vec<u8>> {
    // SAFETY: a pure size computation valid for every length.
    let bound = unsafe { ffi::ZSTD_compressBound(input.len()) };
    let mut output = Vec::<u8>::with_capacity(bound);
    // SAFETY: creating a context has no preconditions; it is freed below on
    // every path.
    let cctx = unsafe { ffi::ZSTD_createCCtx() };
    if cctx.is_null() {
        return Err(Error::CEncode.into());
    }
    // SAFETY: `cctx` is live; `output` owns `bound` writable bytes and does
    // not overlap `input`. C writes at most `bound` bytes and returns either
    // the frame size or an error code.
    let (set, written) = unsafe {
        let set = ffi::ZSTD_CCtx_setParameter(cctx, ffi::ZSTD_c_compressionLevel, level);
        let written = ffi::ZSTD_compress2(
            cctx,
            output.as_mut_ptr().cast(),
            bound,
            input.as_ptr().cast(),
            input.len(),
        );
        ffi::ZSTD_freeCCtx(cctx);
        (set, written)
    };
    // SAFETY: pure error-code classification.
    if unsafe { ffi::ZSTD_isError(set) != 0 || ffi::ZSTD_isError(written) != 0 } {
        return Err(Error::CEncode.into());
    }
    // SAFETY: a successful call initialized the first `written <= bound`
    // bytes.
    unsafe { output.set_len(written) };
    Ok(output)
}

/// `ZSTD_decompress` into a buffer of the frame's declared content size.
fn c_decompress(frame: &[u8], capacity: usize) -> Result<Vec<u8>> {
    let mut output = Vec::<u8>::with_capacity(capacity);
    // SAFETY: `output` owns `capacity` writable bytes disjoint from `frame`;
    // C writes at most that many and returns the size or an error code.
    let written = unsafe {
        ffi::ZSTD_decompress(
            output.as_mut_ptr().cast(),
            capacity,
            frame.as_ptr().cast(),
            frame.len(),
        )
    };
    // SAFETY: pure error-code classification.
    if unsafe { ffi::ZSTD_isError(written) } != 0 {
        return Err(Error::CDecode.into());
    }
    // SAFETY: a successful call initialized the first `written <= capacity`
    // bytes.
    unsafe { output.set_len(written) };
    Ok(output)
}

fn validate(input: &[u8], frame: &[u8]) -> Result<()> {
    if c_decompress(frame, input.len())? != input {
        return Err(Error::RoundTrip.into());
    }
    Ok(())
}

/// mbrotli's comparison datasets, with the same generators and seeds.
fn corpora() -> Vec<(&'static str, Vec<u8>)> {
    let text = include_bytes!("../../../testdata/alice29.txt");
    let mut state = 0x243f_6a88_85a3_08d3_u64;
    let random: Vec<u8> = (0..1_048_576)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect();
    vec![
        ("empty", Vec::new()),
        (
            "tiny-text",
            b"The quick brown fox jumps over the lazy dog.".to_vec(),
        ),
        ("alice29", text.to_vec()),
        (
            "text-1m",
            text.iter().copied().cycle().take(1_048_576).collect(),
        ),
        (
            "binary-64k",
            (0..65_536_u32).map(|i| ((i / 16) ^ i) as u8).collect(),
        ),
        ("random-64k", random[..65_536].to_vec()),
        ("random-1m", random),
        ("repeated-1m", vec![b'a'; 1_048_576]),
    ]
}

/// Criterion configured like mbrotli's comparison: 20 samples, 1 s warmup,
/// at least 3 s of measurement, command-line overrides last.
fn criterion(directory: &str) -> Result<Criterion> {
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(directory);
    std::fs::create_dir_all(&output)?;
    Ok(Criterion::default()
        .output_directory(&output)
        .sample_size(20)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3))
        .configure_from_args())
}

pub(super) fn run() -> Result<()> {
    let directory = "target/criterion";
    let mut criterion = criterion(directory)?;
    let corpora = corpora();
    // Preflight the entire matrix before timing anything, filtered runs
    // included: both frames must be identical and decode to the input.
    let mut sizes = Vec::new();
    writeln!(
        sizes,
        "corpus,input_bytes,level,implementation,compressed_bytes"
    )?;
    for (name, input) in &corpora {
        for level in LEVELS {
            let reference = compress("c-zstd", input, level)?;
            validate(input, &reference)?;
            for encoder in ENCODERS {
                let frame = compress(encoder, input, level)?;
                if frame != reference {
                    return Err(Error::NotIdentical {
                        corpus: name,
                        level,
                    }
                    .into());
                }
                writeln!(
                    sizes,
                    "{name},{},{level},{encoder},{}",
                    input.len(),
                    frame.len()
                )?;
            }
        }
    }
    std::fs::write(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(directory)
            .join("sizes.csv"),
        sizes,
    )?;
    for (name, input) in &corpora {
        for level in LEVELS {
            let mut group = criterion.benchmark_group(format!("encoders/cold/{name}/l{level}"));
            group.throughput(Throughput::Bytes(input.len() as u64));
            for encoder in ENCODERS {
                let mut failure = None;
                group.bench_function(BenchmarkId::from_parameter(encoder), |b| {
                    b.iter(|| match compress(encoder, black_box(input), level) {
                        Ok(output) => {
                            black_box(output);
                        }
                        Err(error) => failure = Some(error),
                    });
                });
                if let Some(error) = failure {
                    return Err(error);
                }
            }
            group.finish();
        }
    }
    criterion.final_summary();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_encoders_write_identical_frames_at_buffer_boundaries() -> Result<()> {
        for level in [-5, 1, 3, 19] {
            for length in [0, 1, 15, 16, 17, 4095, 4096, 4097] {
                let input: Vec<u8> = (0..length).map(|i| (i % 251) as u8).collect();
                let reference = compress("c-zstd", &input, level)?;
                validate(&input, &reference)?;
                assert_eq!(compress("mzstd", &input, level)?, reference);
            }
        }
        Ok(())
    }

    #[test]
    fn oracle_rejects_corruption_wrong_content_and_unknown_names() -> Result<()> {
        assert!(validate(b"x", &[]).is_err());
        assert!(validate(b"y", &c_compress(b"x", 3)?).is_err());
        assert!(c_decompress(&c_compress(b"xyz", 3)?, 1).is_err());
        assert!(compress("unknown", b"", 3).is_err());
        assert!(compress("mzstd", b"", 23).is_err());
        Ok(())
    }

    #[test]
    fn corpus_is_deterministic_and_has_all_size_classes() {
        let first = corpora();
        assert_eq!(first, corpora());
        assert_eq!(first.len(), 8);
        assert!(first.iter().any(|(_, input)| input.is_empty()));
        assert!(first.iter().any(|(_, input)| input.len() == 1_048_576));
    }
}
