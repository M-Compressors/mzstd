//! Replays the comparison benchmark's matrix through the target bodies.
//!
//! `benchmarks/comparison` times eight corpora at levels -5, -1 and 1 to 22.
//! Each of those inputs, behind the parameter header that selects its level,
//! goes through the encoder and decoder targets, so every case the published
//! benchmarks time is also a regression input. The corpora are generated as
//! the comparison generates them; the 1 MiB ones are cut to
//! [`mzstd_afl::MAX_PAYLOAD`], as every target cuts its payload.

/// The comparison's levels.
const LEVELS: [i32; 24] = [
    -5, -1, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
];

/// The comparison's corpora (`benchmarks/comparison/src/core.rs`).
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

/// `payload` behind the header `decode_config` reads as `level` with no
/// flags.
fn input(level: i32, payload: &[u8]) -> Vec<u8> {
    let selector = u8::try_from(level + 10).expect("a level from -10 to 22");
    let mut data = vec![selector, 0];
    data.extend_from_slice(&payload[..payload.len().min(mzstd_afl::MAX_PAYLOAD)]);
    data
}

fn replay(body: fn(&[u8])) {
    for (name, payload) in corpora() {
        for level in LEVELS {
            let data = input(level, &payload);
            let decoded = mzstd_afl::decode_config(&data)
                .unwrap_or_else(|| panic!("{name} at level {level} decodes no configuration"));
            assert_eq!(
                decoded.config.level(),
                mzstd::Level::try_from(level).expect("a benchmark level")
            );
            body(&data);
        }
    }
}

#[test]
fn benchmark_inputs_compress_like_the_reference() {
    replay(mzstd_afl::targets::differential_c);
}

#[test]
fn benchmark_frames_decode_one_shot_and_in_pieces() {
    replay(mzstd_afl::targets::decode_roundtrip);
}
