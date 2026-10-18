#!/bin/sh
# Builds the AFL seed corpora from zstd's own test data.
#
# The vendored submodule at zstd-ffi/vendor/zstd ships the payloads upstream
# keeps as compression regressions (the block splitter corruption case, long
# literal and match lengths, an HTTP request, a Huffman edge case) and the
# frames it keeps as decoder fixtures. Reusing them keeps the seeds identical
# to what upstream considers interesting.
#
# Two corpora are produced:
#
#   seeds/params   each payload behind a parameter header (see
#                  src/lib.rs::decode_config), for the encoder targets
#   seeds/frames   upstream's compressed fixtures, valid and invalid, for the
#                  decoder targets
#
# Usage: fuzz/afl/prepare-seeds.sh

set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
tests="$here/../../zstd-ffi/vendor/zstd/tests"
params="$here/seeds/params"
frames="$here/seeds/frames"

# Large seeds slow every fuzzing iteration far more than they add coverage;
# the targets also cap their payload at 256 KiB.
max_bytes=65536

if [ ! -d "$tests/golden-compression" ]; then
    echo "missing $tests" >&2
    echo "run: git submodule update --init --recursive" >&2
    exit 1
fi

rm -rf "$params" "$frames"
mkdir -p "$params" "$frames"

# Header bytes, as decode_config reads them: byte 0 the level (value % 40;
# 0..=32 map to -10..=22), byte 1 the flags (1 checksum, 2 no content size,
# 4/8 LDM mode, 0x10 window, 0x20 strategy, 0x40 search, 0x80 size hint),
# then one byte per enabled override.
headers="13:0 11:1 0:2 20:3 23:4 29:8:40 32:16:12 31:32:136 15:64:200 \
26:112:9:7:200 35:0 33:1"

for path in "$tests"/golden-compression/* "$here/../../zstd-ffi/vendor/zstd/lib/zstd.h"; do
    [ -f "$path" ] || continue
    name=$(basename "$path")
    for header in $headers; do
        bytes=""
        for field in $(echo "$header" | tr ':' ' '); do
            bytes="$bytes$(printf '\\%03o' "$field")"
        done
        target="$params/$(echo "$header" | tr ':' '-')-$name"
        printf "$bytes" > "$target"
        dd if="$path" bs="$max_bytes" count=1 2>/dev/null >> "$target"
    done
done

# The comparison benchmark's text (its first 64 KiB) at one level per
# strategy, and the benchmark inputs committed as regressions, so campaigns
# start from what the published benchmarks time.
alice="$here/../../testdata/alice29.txt"
for level in -5 1 3 5 6 9 13 16 19 22; do
    printf "$(printf '\\%03o' $((level + 10)))\\000" > "$params/bench-alice29-l$level"
    dd if="$alice" bs="$max_bytes" count=1 2>/dev/null >> "$params/bench-alice29-l$level"
done
cp "$here"/regressions/differential_c/bench-*.bin "$params/"

for path in "$tests"/golden-decompression/* "$tests"/golden-decompression-errors/*; do
    [ -f "$path" ] || continue
    dd if="$path" of="$frames/$(basename "$path")" bs="$max_bytes" count=1 2>/dev/null
done

echo "prepared $(ls "$params" | wc -l | tr -d ' ') parameter seeds in $params"
echo "prepared $(ls "$frames" | wc -l | tr -d ' ') frame seeds in $frames"
