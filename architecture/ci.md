# CI

GitHub Actions workflows live in `.github/workflows`. Every job checks out
submodules recursively, because the C reference in
`zstd-ffi/vendor/zstd` is needed to build the test oracle. The
ClusterFuzzLite jobs are the exception: their actions check out the
repository themselves, and `.clusterfuzzlite/build.sh` fetches the
submodule.

```mermaid
graph TD
    Push[push to main / pull request] --> CI[ci.yml]
    CI --> Check["check: fmt, clippy per feature set, rustdoc per feature set, package"]
    CI --> NoStd["no-std: thumbv7em build, alloc-only host tests"]
    CI --> Test["test: Linux x86-64, Linux arm64, macOS, and MSRV 1.89 — release mode"]
    CI --> OCF["overflow-correction: core differential suites with MZSTD_OVERFLOW_CORRECT_FREQUENTLY"]
    CI --> Fuzz["fuzz: fmt/clippy of fuzz/afl, regression replay with and without framed"]
    CI --> LibFuzzer["libfuzzer: fmt/clippy of fuzz/libfuzzer with and without framed"]
    PR[pull request touching code] --> CFPR["ci-cflite-pr.yml: ClusterFuzzLite on the changed code"]
    Daily[daily schedule or manual] --> CFCron["ci-cflite-cron.yml: batch, prune, coverage"]
    CFCron -. coverage report selects targets .-> CFPR
    Manual[workflow_dispatch] --> Cov["ci-coverage.yml: llvm-cov, 100% function coverage"]
    Manual --> Bench["ci-benchmarks.yml: Criterion smoke runs with differential checks"]
    Tag[v* tag or manual] --> Camp["ci-fuzz.yml: bounded AFL campaign per target"]
    Tag --> Miri["ci-miri.yml: Miri over SIMD kernels and BorrowedBuf output"]
    Tag --> ASan["ci-sanitizer.yml: AddressSanitizer over integration and kernel tests"]
    Manual --> Rel["release.yml: trusted publishing from a v* tag"]
```

| Workflow | Trigger | Checks |
| --- | --- | --- |
| `ci.yml` | push to `main`, pull requests | `cargo fmt --check`. Clippy with `-D warnings` for all features, for the default features plus hotpath, and for each codec with `std` and with `no_std`. Rustdoc with `-D warnings` for the same configurations. `cargo package`. A bare-metal build. Release-mode tests on three platforms and on the MSRV, each with the default features, with all features (alloc only) and with `framed` (which adds the std framed adapters and seek reader). Clippy and rustdoc also cover `framed` with each codec. The overflow-correction mode. Fuzz package lint and regression replay. libFuzzer adapter lint. |
| `ci-cflite-pr.yml` | pull requests touching `src/`, `zstd-ffi/`, `fuzz/`, `Cargo.toml`, `.clusterfuzzlite/`, `.dockerignore` or the workflow | ClusterFuzzLite libFuzzer/ASan run on the changed code, 600 s. See [ClusterFuzzLite](#clusterfuzzlite). |
| `ci-cflite-cron.yml` | daily, manual | ClusterFuzzLite batch fuzzing (5.5 h), corpus pruning and a coverage report. |
| `ci-coverage.yml` | manual | `cargo llvm-cov --workspace --release` with `--fail-under-functions 100` over repository-owned sources (zstd-ffi and the vendored C excluded); HTML report as an artifact. |
| `ci-benchmarks.yml` | manual | Short `compress`, `decompress` and `mzstd-ffi` `one_shot` runs. Each benchmark asserts byte identity with zstd before timing. The framed benches (`framed_compress`, `framed_decompress`) are run locally with `--features framed`. |
| `ci-fuzz.yml` | `v*` tags, manual | `prepare-seeds.sh`, then `campaign.sh` for each target, the three framed targets included (600 s by default); findings uploaded as `findings-<target>.tar.gz`, since AFL file names contain `:`, which `upload-artifact` rejects. |
| `ci-miri.yml` | `v*` tags, manual | `scripts/miri.sh` on nightly: exact-named tests for the vector kernels and `BorrowedBuf` output under Miri, interpreted for x86-64 with `MIRI_TARGET_CPU=native`, so the runner's CPU picks the backends. See [memory checks](#memory-checks). |
| `ci-sanitizer.yml` | `v*` tags, manual | Nightly `-Zsanitizer=address` release builds, LeakSanitizer included: every integration test with `framed` (the two that forget an owning guard on purpose without leak detection), then the kernel and uninitialized-output unit modules on the runner's own backends. |
| `release.yml` | manual from a `v*` tag | Tag/version match, then `cargo publish` through crates.io trusted publishing. |

All Cargo invocations that build the workspace pass `--locked`, because
`Cargo.lock` is committed so that benchmark baselines and campaigns are
reproducible.

## ClusterFuzzLite

Both ClusterFuzzLite workflows build `.clusterfuzzlite/Dockerfile` with
`google/clusterfuzzlite/actions/build_fuzzers@v1` and run the binaries that
`build.sh` produces from `fuzz/libfuzzer` (see
[fuzzing](fuzzing.md#libfuzzer-adapters-and-clusterfuzzlite)). The actions do
their own checkout, so the workflows have no checkout step and `build.sh`
fetches the vendored zstd submodule itself. Runs use libFuzzer with
AddressSanitizer, the only engine and sanitizer ClusterFuzzLite supports for
Rust, and the coverage job builds with the coverage sanitizer. All jobs use
read-only permissions; corpora, crashes and coverage reports are kept as
workflow artifacts, ClusterFuzzLite's default storage on GitHub.

```mermaid
sequenceDiagram
    participant PR as pull request
    participant Cron as daily schedule
    participant CF as ClusterFuzzLite actions
    participant Store as workflow artifacts
    PR->>CF: build (address), run code-change, 600 s
    Store-->>CF: latest corpus and coverage report
    CF-->>PR: job fails on a crash, with the reproducer
    Cron->>CF: batch: build (address), fuzz 19800 s, targets in parallel
    CF->>Store: grown corpus, crashes
    Cron->>CF: prune: minimize corpus, 600 s
    CF->>Store: pruned corpus
    Cron->>CF: coverage: build (coverage), report
    CF->>Store: coverage report
```

`ci-cflite-pr.yml` runs only when a pull request touches `src/`, `zstd-ffi/`,
`fuzz/`, `Cargo.toml`, `.clusterfuzzlite/`, `.dockerignore` or the workflow
itself, and a newer push cancels the run in progress. Code-change mode runs
only the targets whose coverage reaches the changed lines once a coverage
report exists, and every target before that. The 600 seconds are split among
the targets it runs.

In `ci-cflite-cron.yml` the jobs are chained: pruning follows batch fuzzing
(ClusterFuzzLite requires pruning whenever batch fuzzing runs), and coverage
follows pruning, so the report reflects the pruned corpus. A failed batch job
still lets pruning and coverage run. Batch fuzzing gets 5.5 hours, shared by
all targets, which run side by side on the runner's cores
(`parallel-fuzzing: true`); the job's six-hour limit leaves the rest for the
image and fuzzer builds and the corpus upload.

`.dockerignore` keeps target directories and local AFL and libFuzzer
artifacts out of the image, but keeps `.git`, which the submodule fetch
needs.

## Memory checks

`mzstd` is `forbid(unsafe_code)` outside tests. The `unsafe` with memory
effects that its code reaches lives in two dependencies: `fearless_simd`,
whose vector types, `kernel!` and `#[simd]` entries call architecture
intrinsics inside a target-feature context, and `borrowed-buf`, which
tracks the zeroed prefix of uninitialized output. The third, `branches`,
only emits prefetch hints, which never fault; on nightly, and so under
Miri, they lower to the compiler intrinsic instead of inline assembly. The
FFI glue in `zstd-ffi` and `mzstd-ffi` is outside the published crate.

```mermaid
flowchart TD
    Unsafe[unsafe reached by mzstd] --> Simd[fearless_simd intrinsics]
    Unsafe --> Buf[borrowed-buf BorrowedBuf]
    Simd --> K1["compressor::core::opt_count: u8x16/32/64 match-length scans"]
    Simd --> K2["compressor::core::lazy::row: tag masks, NEON kernel!"]
    Simd --> K3["decompressor::core::copy: shuffle wildcopy"]
    K3 --> K4["decompressor::core::sequences: execution at fixed and growing output edges"]
    Buf --> B1["compressor::core::output: UninitOutput"]
    Buf --> B2["decompressor::core::stream: decode_to_uninit output"]
    B1 --> B3["tests/compressor_api.rs: compress_to_uninit end to end"]
    K1 & K2 & K3 & K4 & B1 & B2 & B3 --> Miri["scripts/miri.sh: Miri, x86-64: scalar, SSE2, SSE4.2, AVX2, AVX-512 on capable runners"]
    K1 & K2 & K3 & K4 & B1 & B2 --> ASan["ci-sanitizer.yml: native backends under ASan"]
    All[every integration test with framed] --> ASan
```

Miri covers only those paths. Code that is safe Rust inside a `#[simd]`
context, such as the scalar `mem` counts the compiler auto-vectorizes, the
Huffman and sequence bit packers and the match finders around the kernels,
has no `unsafe` of its own and is left to the ordinary suites.

`scripts/miri.sh` interprets the tests for `x86_64-unknown-linux-gnu` on
any host, with `-Ctarget-cpu=$MIRI_TARGET_CPU` (default `x86-64-v3`). Miri
implements the x86 intrinsics the kernels use, AVX-512 included, but not
all of NEON (`vsli` is missing), and it reports compile-time target
features instead of executing CPUID, so the target CPU decides which
backends `Backend::available()` returns. The default gives scalar, SSE2,
SSE4.2 and AVX2 on any machine; CI passes `native`, which adds AVX-512 on
runners with every extension `fearless_simd` requires for it (Ice Lake
and later Xeons, not Cascade Lake or Zen 3), and prints the runner's CPU
model first. A local `MIRI_TARGET_CPU=icelake-server` reproduces that
case on any host. Each run names its tests with
`--exact` and fails unless that many pass, so a renamed test cannot leave
a filter that silently matches nothing. Two sweeps thin out under
`cfg!(miri)` while keeping every vector width, tail and full-length case:
`opt_count`'s window test keeps lengths next to 16-byte boundaries, and
`compress_to_uninit_writes_the_one_shot_frame` compresses 2 KB instead of
30 KB. Native runs are unchanged.

The AddressSanitizer job needs an explicit `--target` so that build
scripts and proc macros stay uninstrumented. It runs every integration
test, the C-reference differential suites included, and then the Miri
modules natively on the runner's own backends, AVX-512 included when the
runner has it. Locally, the same commands with an `aarch64-apple-darwin`
target cover NEON.

On Linux, ASan also runs LeakSanitizer when the process exits, so a test
must not leak either: a borrowed `&'static` input or resolver lives in a
`static OnceLock` rather than behind `Vec::leak` or `Box::leak`. The
exceptions are `forgotten_readers_keep_the_owner_protected_until_recovery`
(`framed_seek`) and `batches_with_leaked_tasks_report_invalid_state`
(`parallel`), which `mem::forget` a seek reader or a task list that owns
memory and borrows the test's input: the leak is the behavior they check.
The integration step skips them and a second step runs them with
`ASAN_OPTIONS=detect_leaks=0`, so ASan still checks their accesses, and a
renamed one would make the first step report its leak. macOS does not run
LeakSanitizer unless `ASAN_OPTIONS=detect_leaks=1` is set.

## Known gaps

- Miri does not interpret NEON and cannot call the C reference, so NEON
  and the differential suites are checked only natively, under
  AddressSanitizer. Whether a tag's Miri run reaches AVX-512 depends on
  the runner GitHub assigns. The C oracle itself is not built
  with ASan.
- Memory checks run on tags and on demand, not on pull requests.
  ClusterFuzzLite's ten-minute pull request run is the only sanitizer run
  there, and it covers only the fuzz targets.
- ClusterFuzzLite builds for x86-64 only, so its runs reach the x86 backends
  the runner supports, not NEON.
- Benchmark numbers from shared runners are smoke tests, not evidence.
