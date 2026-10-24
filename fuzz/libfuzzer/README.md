# libFuzzer targets

This package runs the AFL targets' bodies under libFuzzer, which is what
[ClusterFuzzLite](https://google.github.io/clusterfuzzlite/) builds for Rust.
It owns no target logic: each file in `fuzz_targets/` is a one line
`fuzz_target!` adapter around the body the AFL binary of the same name runs,
from the `mzstd-afl` library. Inputs, oracles and corpora are described in the
[AFL guide](../afl/README.md).

Like `fuzz/afl`, the package is excluded from the root workspace.

## Targets

The 11 stable targets are every stable AFL target. `--features framed` adds
`framed_decode`, `framed_encode` and `framed_roundtrip`.

## Local runs

cargo-fuzz needs nightly Rust for the sanitizer flags. From the repository root:

```sh
cargo install cargo-fuzz --locked
cargo +nightly fuzz build -O --debug-assertions --fuzz-dir fuzz/libfuzzer
cargo +nightly fuzz run --fuzz-dir fuzz/libfuzzer -O --debug-assertions decompress
```

Add `--features framed` to build and run the framed targets.

To reproduce exactly what ClusterFuzzLite builds, including the seed corpora,
run `.clusterfuzzlite/build.sh` with `SRC`, `OUT` and `WORK` set. `SRC` must
contain the repository as `mzstd`.

## Findings

ClusterFuzzLite attaches the crashing input to the failed workflow run.
Reproduce it with the binary of that target:

```sh
cargo +nightly fuzz run --fuzz-dir fuzz/libfuzzer -O --debug-assertions <target> <crash-file>
```

Then minimize it, commit it under `fuzz/afl/regressions/<target>/` as
`*.bin`, check that `cargo afl test` fails on it, and fix the bug. The
regression replay does not need a libFuzzer build.
