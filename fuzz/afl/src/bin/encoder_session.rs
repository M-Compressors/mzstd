//! Thin AFL adapter around [`mzstd_afl::targets::encoder_session`].

fn main() {
    afl::fuzz!(|data: &[u8]| {
        mzstd_afl::targets::encoder_session(data);
    });
}
