/* Force-included ahead of every translation unit of the oracle (see
 * build.rs). Fuzzing toolchains such as OSS-Fuzz and ClusterFuzzLite put
 * -DFUZZING_BUILD_MODE_UNSAFE_FOR_PRODUCTION into CFLAGS, and zstd then
 * stops being the release library mzstd is byte-identical to: the
 * superblock encoder drops its decoder-compatibility fallbacks to raw
 * blocks, overflow correction runs on every block, and the decoder skips
 * the dictionary ID check. `cc` appends CFLAGS after every flag build.rs
 * sets, so an -U there could not win; a forced include runs after all
 * command-line definitions. */
#undef FUZZING_BUILD_MODE_UNSAFE_FOR_PRODUCTION
