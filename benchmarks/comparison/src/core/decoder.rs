use super::{Error, LEVELS, Result, c_compress, c_decompress, corpora, criterion};
use criterion::{BenchmarkId, Throughput};
use mzstd::Decompressor;
use std::hint::black_box;
use std::io::Write;
use std::path::PathBuf;

/// Decoders in report order; the C library is the reference.
const DECODERS: [&str; 2] = ["c-zstd", "mzstd"];

/// Restores `frame` through one decoder's cold one-shot API.
fn decompress(decoder: &str, frame: &[u8], output_bytes: usize) -> Result<Vec<u8>> {
    match decoder {
        // C's one-shot API needs the output capacity up front; reading it
        // from the frame header is part of the measured call.
        "c-zstd" => {
            // SAFETY: a pure header read within `frame`.
            let declared =
                unsafe { zstd_ffi::ZSTD_getFrameContentSize(frame.as_ptr().cast(), frame.len()) };
            if declared >= zstd_ffi::ZSTD_CONTENTSIZE_ERROR {
                return Err(Error::CDecode.into());
            }
            let capacity = usize::try_from(declared).map_err(|_| Error::CDecode)?;
            debug_assert_eq!(capacity, output_bytes);
            c_decompress(frame, capacity)
        }
        "mzstd" => Ok(Decompressor::default().decompress(frame)?),
        _ => Err(Error::Unknown(decoder.to_owned()).into()),
    }
}

pub(crate) fn run() -> Result<()> {
    let directory = "target/criterion-decoders";
    let mut criterion = criterion(directory)?;
    let mut streams = Vec::new();
    let mut sizes = Vec::new();
    writeln!(
        sizes,
        "corpus,input_bytes,level,implementation,compressed_bytes"
    )?;
    // Every decoder must restore every C-written frame before timing starts.
    for (name, input) in corpora() {
        for level in LEVELS {
            let frame = c_compress(&input, level)?;
            for decoder in DECODERS {
                if decompress(decoder, &frame, input.len())? != input {
                    return Err(Error::RoundTrip.into());
                }
                writeln!(
                    sizes,
                    "{name},{},{level},{decoder},{}",
                    input.len(),
                    frame.len()
                )?;
            }
            streams.push((name, input.len(), level, frame));
        }
    }
    std::fs::write(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(directory)
            .join("sizes.csv"),
        sizes,
    )?;
    for (name, output_bytes, level, frame) in &streams {
        let mut group = criterion.benchmark_group(format!("decoders/cold/{name}/l{level}"));
        // Throughput counts restored bytes, not frame bytes.
        group.throughput(Throughput::Bytes(*output_bytes as u64));
        for decoder in DECODERS {
            let mut failure = None;
            group.bench_function(BenchmarkId::from_parameter(decoder), |b| {
                b.iter(
                    || match decompress(decoder, black_box(frame), *output_bytes) {
                        Ok(output) => {
                            black_box(output);
                        }
                        Err(error) => failure = Some(error),
                    },
                );
            });
            if let Some(error) = failure {
                return Err(error);
            }
        }
        group.finish();
    }
    criterion.final_summary();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_decoder_restores_empty_tiny_and_buffer_boundaries() -> Result<()> {
        for level in [-5, 1, 19] {
            for length in [0, 1, 15, 4095, 4096, 4097, 131_073] {
                let input: Vec<u8> = (0..length).map(|i| (i % 251) as u8).collect();
                let frame = c_compress(&input, level)?;
                for decoder in DECODERS {
                    assert_eq!(decompress(decoder, &frame, input.len())?, input);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn decoders_reject_corrupt_frames_and_unknown_names() {
        for decoder in DECODERS {
            assert!(decompress(decoder, b"not a frame", 0).is_err());
        }
        assert!(decompress("unknown", &[], 0).is_err());
    }
}
