//! Thin AFL adapter around [`mzstd_afl::targets::parameter_parsing`].

fn main() {
    afl::fuzz!(|data: &[u8]| {
        mzstd_afl::targets::parameter_parsing(data);
    });
}
