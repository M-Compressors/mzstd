//! Thin AFL adapter around [`mzstd_afl::targets::parallel`].

fn main() {
    afl::fuzz!(|data: &[u8]| {
        mzstd_afl::targets::parallel(data);
    });
}
