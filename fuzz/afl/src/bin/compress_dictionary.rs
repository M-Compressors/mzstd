//! Thin AFL adapter around [`mzstd_afl::targets::compress_dictionary`].

fn main() {
    afl::fuzz!(|data: &[u8]| {
        mzstd_afl::targets::compress_dictionary(data);
    });
}
