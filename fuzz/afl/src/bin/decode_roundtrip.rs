//! Thin AFL adapter around [`mzstd_afl::targets::decode_roundtrip`].

fn main() {
    afl::fuzz!(|data: &[u8]| {
        mzstd_afl::targets::decode_roundtrip(data);
    });
}
