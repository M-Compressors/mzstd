//! Thin AFL adapter around [`mzstd_afl::targets::decode_dictionary`].

fn main() {
    afl::fuzz!(|data: &[u8]| {
        mzstd_afl::targets::decode_dictionary(data);
    });
}
