//! Replays every committed fuzz input through its target body.
//!
//! AFL finds an input, `cargo afl tmin` shrinks it, the minimised bytes land
//! in `regressions/<target>/`, and from then on `cargo afl test` re-checks
//! them without an instrumented binary.

use std::fs;
use std::path::{Path, PathBuf};

fn inputs_for(target: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("regressions")
        .join(target);
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        .map(|entry| entry.expect("unreadable directory entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "bin"))
        .collect();
    paths.sort();
    paths
}

fn replay(target: &str, body: fn(&[u8])) {
    let inputs = inputs_for(target);
    assert!(!inputs.is_empty(), "no regression inputs for {target}");
    for path in inputs {
        let data = fs::read(&path).expect("readable regression input");
        body(&data);
    }
}

#[test]
fn differential_c() {
    replay("differential_c", mzstd_afl::targets::differential_c);
}

#[test]
fn output_capacity() {
    replay("output_capacity", mzstd_afl::targets::output_capacity);
}

#[test]
fn encoder_session() {
    replay("encoder_session", mzstd_afl::targets::encoder_session);
}

#[test]
fn compressor_lifecycle() {
    replay(
        "compressor_lifecycle",
        mzstd_afl::targets::compressor_lifecycle,
    );
}

#[test]
fn parallel() {
    replay("parallel", mzstd_afl::targets::parallel);
}

#[test]
fn parameter_parsing() {
    replay("parameter_parsing", mzstd_afl::targets::parameter_parsing);
}

#[test]
fn committed_inputs_past_the_boundaries_decode_a_configuration() {
    for path in inputs_for("differential_c") {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if name.starts_with("boundary-") {
            continue;
        }
        let data = fs::read(&path).expect("readable regression input");
        let decoded = mzstd_afl::decode_config(&data)
            .unwrap_or_else(|| panic!("{name} decodes no configuration"));
        assert!(!decoded.rest.is_empty(), "{name} carries no payload");
    }
}

#[test]
fn decompress() {
    replay("decompress", mzstd_afl::targets::decompress);
}

#[test]
fn decode_streaming() {
    replay("decode_streaming", mzstd_afl::targets::decode_streaming);
}

#[test]
fn decode_roundtrip() {
    replay("decode_roundtrip", mzstd_afl::targets::decode_roundtrip);
}

#[test]
fn decode_dictionary() {
    replay("decode_dictionary", mzstd_afl::targets::decode_dictionary);
}

#[test]
fn compress_dictionary() {
    replay(
        "compress_dictionary",
        mzstd_afl::targets::compress_dictionary,
    );
}
