# Contributing to mzstd

Thank you for helping. Bug reports, differential test cases, fuzz findings,
documentation fixes, and code are all welcome. By participating you agree to
follow the [Code of Conduct](CODE_OF_CONDUCT.md).

Security issues are reported privately; see [SECURITY.md](SECURITY.md).

## Before you start

- For anything beyond a small fix, open an issue first so we can agree on the
  approach before you invest time in it.
- Read [`architecture/README.md`](architecture/README.md). It describes how the
  code is organized and is kept current with every change.
- The encoder's contract is **byte identity with zstd 1.5.7**: one-shot calls
  must match `ZSTD_compress2`, and sessions must match `ZSTD_compressStream2`
  driven with the same chunks, output capacities, and flushes. Changes to
  `compressor::core` port the reference's control flow; a heuristic that
  compresses better but writes different bytes will not be merged.
- Do not add a dependency on an existing zstd implementation.
- Do not edit `zstd-ffi/vendor/` unless the change is an upstream vendor update.

## Setting up

```sh
git clone --recurse-submodules https://github.com/M-Compressors/mzstd.git
cd mzstd
```

If you already cloned without submodules, run
`git submodule update --init --recursive`. The `zstd-ffi` crate builds zstd
1.5.7 from the vendored submodule as the test and benchmark oracle, so a C
compiler is required. The minimum supported Rust version is 1.89.

## Code guidelines

The full rules are in [`CLAUDE.md`](CLAUDE.md); the essentials:

- **Module shape.** A public module owns the ergonomic API; its private
  `core` submodule owns the implementation. Never expose `core` types, SIMD
  types, FFI details, or low-level errors.
- **Small public APIs.** Prefer borrowed inputs and standard traits (`From`,
  `TryFrom`, `Default`, `Display`, `FromStr`, ...) over inherent constructors.
  Document every public item, with `# Examples`, `# Errors`, `# Panics`, and
  `# Safety` sections where they apply.
- **Errors.** Use `thiserror`. Library code returns `Result`; no `unwrap`,
  `expect`, or panics for recoverable failures. Public errors keep the source
  chain and are `#[non_exhaustive]`.
- **No `unsafe`** in the `mzstd` crate.
- **Performance.** Measure before optimizing (the `hotpath` features). SIMD
  goes through `fearless_simd`, keeps a baseline implementation, and selects
  the level outside inner loops; every SIMD level must produce the same output
  as the baseline.
- **Lints.** Fix warnings rather than suppressing them. Do not add
  `#[allow(..)]`; for a demonstrated false positive, use the narrowest
  `#[expect(clippy::lint)]` with a comment explaining why.

## Tests

- Every new or changed function needs tests. The project keeps **100% function
  coverage** of its own Rust code.
- Unit tests sit beside private `core` logic; public API and cross-module
  behavior go in `tests/`; public examples are doc tests.
- Fixing a bug starts with a regression test that fails without the fix.
- Encoder changes need differential tests against the C implementation
  (`zstd_ffi::oracle`); optimized paths need differential tests against the
  baseline.

## Checks to run before opening a pull request

The differential suites take over an hour without optimizations, so tests run
in release mode.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --release --locked
cargo test --workspace --all-features --release --locked
cargo test --workspace --features framed --release --locked
```

All three test runs are needed: `--all-features` turns on `no_std`, which
removes `io`, `parallel`, and the framed `std` adapters.

After changing `compressor::core::window`, `cctx`, or `match_state`, also run:

```sh
MZSTD_OVERFLOW_CORRECT_FREQUENTLY=1 CARGO_TARGET_DIR=target-ocf \
  cargo test --release --lib compressor::core
```

Check function coverage for what you changed:

```sh
CARGO_INCREMENTAL=1 cargo llvm-cov --workspace --release --summary-only \
  --ignore-filename-regex '/zstd-ffi/|/vendor/'
```

### Fuzzing

The AFL++ targets live in [`fuzz/afl/`](fuzz/afl/README.md), outside the
workspace. After changing anything there, or a public API a fuzz target
calls, run from `fuzz/afl/`:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --no-default-features -- -D warnings
cargo clippy --all-targets --no-default-features --features framed -- -D warnings
cargo afl test --release --no-default-features
cargo afl test --release --no-default-features --features framed
```

Turn every minimized crash into a regression input under
`fuzz/afl/regressions/` before fixing it. Never commit AFL findings or output
directories.

### Memory checks

Release tags run Miri and AddressSanitizer; see
[CI](architecture/ci.md#memory-checks). Both need a nightly toolchain, Miri
also its component (`rustup +nightly component add miri`). After changing a
SIMD kernel or uninitialized-output code, run them locally:

```sh
scripts/miri.sh
ASAN_OPTIONS=detect_leaks=1 RUSTFLAGS=-Zsanitizer=address RUSTDOCFLAGS=-Zsanitizer=address \
  cargo +nightly test --release --locked --target "$(rustc -vV | sed -n 's/host: //p')" \
  --features framed --test '*' -- \
  --skip forgotten_readers_keep_the_owner_protected_until_recovery \
  --skip batches_with_leaked_tasks_report_invalid_state
```

LeakSanitizer is on by default on Linux but not on macOS, hence
`detect_leaks=1`. Tests must not leak: keep a `&'static` value in a
`static OnceLock` instead of `Box::leak`. The two skipped tests forget an
owning guard on purpose; CI runs them without leak detection.

`scripts/miri.sh` names its tests exactly. Add a new kernel's or
`BorrowedBuf` user's tests to it, keeping them free of the C reference,
which Miri cannot call.

### Benchmarks

Performance claims need evidence. Criterion benchmarks compare mzstd with the
C implementation on identical inputs and parameters; see
[`docs/benchmarking.md`](docs/benchmarking.md). Report the command, corpus,
machine, and before/after numbers for both speed and compressed size.

## Documentation and changelog

- Update the specifications and Mermaid diagrams in `architecture/`, and its
  `README.md` index, whenever a change adds, removes, or reshapes a module,
  public API, state machine, data flow, or dispatch boundary. Documentation
  drift counts as an incomplete change.
- Add an entry to [`CHANGELOG.md`](CHANGELOG.md) for user-visible changes.

## Pull requests

- Keep each pull request focused on one change.
- Fill in the pull request template, including which checks you ran.
- CI runs formatting, Clippy, docs, `no_std`, tests on Linux (x86-64 and
  Arm) and macOS, the overflow-correction suite, and the fuzz regression
  corpus. It must pass before merging.

## License

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this work by you, as defined in the Apache-2.0
license, shall be dual licensed under the MIT and Apache-2.0 licenses, without
any additional terms or conditions.
