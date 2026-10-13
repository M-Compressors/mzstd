//! Thin AFL adapter around [`mzstd_afl::targets::differential_c`].

fn main() {
    afl::fuzz!(|data: &[u8]| {
        mzstd_afl::targets::differential_c(data);
    });
}
