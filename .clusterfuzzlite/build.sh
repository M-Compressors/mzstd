#!/bin/bash -eu
# Builds the libFuzzer targets in fuzz/libfuzzer for ClusterFuzzLite.
#
# Each target is a three line adapter around a body in the AFL package's
# library, so a finding here replays through `cargo afl test` once its input is
# committed under fuzz/afl/regressions/<target>/.
#
# The stable targets are built without `framed`, so the encoder and decoder
# they fuzz are the default surface a dependant gets. The framed targets come
# from a second build in its own target directory; its copies of the stable
# targets are not shipped.

cd "$SRC/mzstd"

# The C oracle is the pinned vendored zstd, a submodule ClusterFuzzLite's own
# checkout leaves empty. The pin comes from the checked out commit.
if [ ! -f zstd-ffi/vendor/zstd/lib/zstd.h ]; then
    git config --global --add safe.directory '*'
    git submodule update --init --depth 1 zstd-ffi/vendor/zstd
fi

# Debug assertions also turn on overflow checks, which AFL's builds have too.
build() {
    cargo fuzz build -O --debug-assertions --fuzz-dir fuzz/libfuzzer "$@"
}

host=$(rustc -vV | sed -n 's/^host: //p')

ship() {
    local release="$1/$host/release"
    for source in fuzz/libfuzzer/fuzz_targets/*.rs; do
        local target
        target=$(basename "$source" .rs)
        if [ -x "$release/$target" ] && [ ! -e "$OUT/$target" ]; then
            cp "$release/$target" "$OUT/"
        fi
    done
}

build --target-dir "$WORK/stable"
ship "$WORK/stable"
build --target-dir "$WORK/framed" --features framed
ship "$WORK/framed"

# Seed corpora: the generated AFL corpus each target's input layout reads, as
# campaign.sh maps them, plus its committed regressions. prepare-seeds.sh
# already cuts payloads to 64 KiB; regressions stay within the targets'
# 256 KiB payload cap.
fuzz/afl/prepare-seeds.sh

seeds_for() {
    case "$1" in
        decompress) echo fuzz/afl/seeds/frames ;;
        # These read their own layouts, which only their regressions provide.
        decode_streaming | decode_dictionary | compress_dictionary | framed_*) ;;
        *) echo fuzz/afl/seeds/params ;;
    esac
}

for binary in "$OUT"/*; do
    [ -x "$binary" ] || continue
    target=$(basename "$binary")
    corpus="$WORK/corpus/$target"
    rm -rf "$corpus"
    mkdir -p "$corpus"
    # Prefixes keep a seed and a regression with the same name apart.
    for directory in $(seeds_for "$target") "fuzz/afl/regressions/$target"; do
        [ -d "$directory" ] || continue
        prefix=$(basename "$(dirname "$directory")")
        find "$directory" -maxdepth 1 -type f -print0 |
            while IFS= read -r -d '' path; do
                cp "$path" "$corpus/$prefix-$(basename "$path")"
            done
    done
    if [ -n "$(ls -A "$corpus")" ]; then
        (cd "$corpus" && zip -q "$OUT/${target}_seed_corpus.zip" ./*)
    fi
done
