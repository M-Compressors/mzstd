//! Thin AFL adapter around [`mzstd_afl::framed_targets::framed_decode`].

fn main() {
    afl::fuzz!(|data: &[u8]| {
        mzstd_afl::framed_targets::framed_decode(data);
    });
}
