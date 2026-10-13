//! Thin AFL adapter around [`mzstd_afl::targets::output_capacity`].

fn main() {
    afl::fuzz!(|data: &[u8]| {
        mzstd_afl::targets::output_capacity(data);
    });
}
