//! Thin AFL adapter around [`mzstd_afl::targets::decode_streaming`].

fn main() {
    afl::fuzz!(|data: &[u8]| {
        mzstd_afl::targets::decode_streaming(data);
    });
}
