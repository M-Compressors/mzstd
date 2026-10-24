#!/usr/bin/env bash
# Runs Miri over the code that reaches mzstd's only `unsafe`: the SIMD
# intrinsics behind fearless_simd's vector types and kernels, and
# borrowed-buf's bookkeeping for uninitialized output. The crate itself
# forbids `unsafe`, so the rest of it is left to the ordinary suites.
#
# Usage: [MIRI_TARGET_CPU=<cpu>] scripts/miri.sh
#
# Miri implements x86's SIMD intrinsics but not all of NEON's, so the
# tests are interpreted for x86-64 on any host. Miri reports the
# compile-time target features instead of executing CPUID, so the target
# CPU chooses the backends `Backend::available()` yields: the default
# x86-64-v3 gives scalar, SSE2, SSE4.2 and AVX2, and CI passes `native`,
# which adds AVX-512 on runners that have every extension fearless_simd
# requires for it. `native` is only meaningful on an x86-64 host.
#
# Tests are named exactly and every run must pass exactly as many as it
# names: `cargo miri test` succeeds when a renamed test leaves a filter
# matching nothing.

set -euo pipefail

cd "$(dirname -- "$0")/.."
export RUSTFLAGS="${RUSTFLAGS:-} -Ctarget-cpu=${MIRI_TARGET_CPU:-x86-64-v3}"
log=$(mktemp)
trap 'rm -f "$log"' EXIT

# run <cargo target selector> <exact test name>...
run() {
    local selector=$1
    shift
    cargo +nightly miri test --locked --target x86_64-unknown-linux-gnu "$selector" \
        -- --exact "$@" 2>&1 | tee "$log"
    if ! grep -q "test result: ok. $# passed" "$log"; then
        echo "expected $# passing tests in $selector" >&2
        exit 1
    fi
}

# Vector kernels: the match-length scans of the optimal parser and LDM, the
# row match finder's tag masks, and the decoder's shuffle wildcopy, alone
# and inside sequence execution at the edges of fixed and growing outputs.
run --lib \
    compressor::core::opt_count::tests::windows_agree_with_a_byte_scan_on_every_backend \
    compressor::core::opt_count::tests::segment_counts_stop_at_the_limit \
    compressor::core::opt_count::tests::ordered_counts_report_the_first_differing_bytes \
    compressor::core::opt_count::tests::two_segment_counts_continue_at_the_prefix_start \
    compressor::core::lazy::row::tests::masks_equal_the_scalar_reference_on_every_backend \
    compressor::core::lazy::row::tests::a_single_matching_slot_is_found_from_every_head \
    decompressor::core::copy::tests::wildcopied_matches_equal_byte_copies_for_every_offset_and_mode \
    decompressor::core::sequences::tests::long_sequences_execute_alike_however_the_output_grows \
    decompressor::core::sequences::tests::outputs_that_stop_growing_short_of_the_overlength_execute_exactly

# BorrowedBuf: the encoder's zeroed-ahead destination and the decoder's
# one-shot output, zeroed on demand within a block and a frame.
run --lib \
    compressor::core::output::tests::uninit_output_zeroes_only_up_to_the_hint \
    compressor::core::output::tests::uninit_output_keeps_written_bytes_when_it_grows \
    compressor::core::output::tests::uninit_output_stops_at_its_capacity \
    compressor::core::output::tests::destinations_copy_and_expose_their_tails \
    decompressor::core::stream::tests::uninit_output_zeroes_on_demand_within_the_block_and_frame \
    decompressor::core::stream::tests::uninit_output_places_raw_and_rle_blocks_over_and_past_the_zeroed_prefix \
    decompressor::core::stream::tests::decode_to_uninit_writes_nothing_past_the_output

# The public encoder into uninitialized memory, end to end on the default
# backend; the other integration tests compare against the C reference,
# which Miri cannot call.
run --test=compressor_api \
    compress_to_uninit_writes_the_one_shot_frame \
    compress_to_uninit_reports_a_destination_that_is_too_small
