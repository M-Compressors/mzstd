//! Thin AFL adapter around [`mzstd_afl::targets::compressor_lifecycle`].

fn main() {
    afl::fuzz!(|data: &[u8]| {
        mzstd_afl::targets::compressor_lifecycle(data);
    });
}
