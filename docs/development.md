# Development and checks

Read the [contributing guide](../CONTRIBUTING.md) and
[architecture index](../architecture/README.md) before changing the implementation.
The main crate has no dependency on another Zstandard implementation. The C
reference is used only by development, tests, and benchmarks.

## Set up

Requires Rust **1.89+**, a C compiler for the reference oracle, and Git:

```sh
git clone --recurse-submodules https://github.com/M-Compressors/mzstd.git
cd mzstd
```

For an existing clone, initialize `git submodule update --init --recursive`.

## Format, lint, and test

Differential tests run in release mode because debug execution is too slow:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --all-targets --features framed --locked -- -D warnings
cargo test --workspace --release --locked
cargo test --workspace --all-features --release --locked
cargo test --workspace --features framed --release --locked
```

All features enable `no_std`, removing std-only APIs, so the default and framed
std configurations need separate runs. CI also checks each codec separately,
the MSRV, bare-metal compilation, rustdoc, packaging, and index overflow correction.

## Fuzzing, memory, and proofs

AFL and libFuzzer are excluded packages with their own feature checks and
regression commands. Follow the [contributing guide](../CONTRIBUTING.md#fuzzing)
and [fuzzing specification](../architecture/fuzzing.md). Never commit generated
findings or corpus output directories; minimize failures into regression inputs.

Use `scripts/miri.sh` and the documented AddressSanitizer commands after changing
SIMD or uninitialized-output paths. Inspect function coverage with
`cargo llvm-cov`; generated reports stay local. Changes to verified format
primitives require refreshing Charon/Aeneas output and rebuilding Lean proofs.
See [CI memory checks](../architecture/ci.md#memory-checks) and
[formal verification](../architecture/formal-verification.md).

## Documentation and benchmarks

Keep architecture specifications and Mermaid diagrams current, add new
specifications to the index, and record user-visible changes in the changelog.
All guide and architecture pages are published together. Validate the book and
its runnable recipes:

```sh
python3 -m unittest discover -s scripts -p test_docs.py
python3 scripts/docs.py test-examples
python3 scripts/docs.py build
```

Install the pinned tools and preview as described in
[documentation website](website.md). The [benchmarking guide](benchmarking.md)
describes cold comparisons, profiling, raw measurements, and chart generation.
Performance changes need representative measurements of throughput and output
size, with the command, machine, input, and before/after results recorded.
