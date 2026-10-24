//! libFuzzer adapter for [`mzstd_afl::targets::decode_dictionary`],
//! the body the AFL binary of the same name runs.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mzstd_afl::targets::decode_dictionary(data));
