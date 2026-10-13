//! Thin AFL adapter around [`mzstd_afl::targets::decompress`].

fn main() {
    afl::fuzz!(|data: &[u8]| {
        mzstd_afl::targets::decompress(data);
    });
}
