//! Standalone end-to-end comparison of mzstd with the zstd 1.5.7 C library.
//!
//! Both implementations run through their cold one-shot APIs on identical
//! bytes and levels; every case is validated before any timing starts.

mod core;

/// Runs the cold compression comparison with Criterion's command-line
/// filtering and sampling.
///
/// # Errors
///
/// Returns an error if either encoder fails, if mzstd's frame differs from
/// zstd's, if a frame does not round-trip, or if the size manifest cannot be
/// written.
pub fn run_encoders() -> Result<(), Box<dyn std::error::Error>> {
    core::run()
}

/// Runs the cold decompression comparison on identical C-written frames.
///
/// # Errors
///
/// Returns an error if frame preparation, decoding, validation or the size
/// manifest fails.
pub fn run_decoders() -> Result<(), Box<dyn std::error::Error>> {
    core::decoder::run()
}

/// Profiles one comparison case, using the same corpus and cold APIs as
/// the Criterion suite. Arguments: codec, implementation, corpus, level,
/// iteration count.
///
/// # Errors
///
/// Returns an error for invalid arguments, codec failures or a failed
/// preflight comparison.
pub fn run_profile() -> Result<(), Box<dyn std::error::Error>> {
    core::profile::run()
}
