#!/bin/sh
# Runs every target for a fixed time, one AFL instance per target, in
# parallel, and reports crashes and hangs.
#
# Usage: fuzz/afl/campaign.sh [seconds] [target...]
#
# Targets read the prepared seeds (run prepare-seeds.sh first) when they exist
# and their committed regressions otherwise. Findings land in
# findings/<target>.

set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
cd "$here"
seconds=${1:-300}
[ "$#" -gt 0 ] && shift
targets=${*:-"differential_c output_capacity encoder_session compressor_lifecycle parallel parameter_parsing decompress decode_streaming decode_roundtrip"}

cargo afl build --release
mkdir -p findings
for target in $targets; do
    # Encoder targets and decode_roundtrip read a parameter header; decompress
    # reads a frame; decode_streaming reads two piece sizes and a frame, which
    # only its committed regressions provide.
    seeds="regressions/$target"
    case "$target" in
        decompress) [ -d seeds/frames ] && seeds=seeds/frames ;;
        decode_streaming) ;;
        *) [ -d seeds/params ] && seeds=seeds/params ;;
    esac
    rm -rf "findings/$target"
    AFL_SKIP_CPUFREQ=1 AFL_NO_UI=1 AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES=1 \
        cargo afl fuzz -V "$seconds" -t 20000 -i "$seeds" -o "findings/$target" \
        "target/release/$target" > "findings/$target.log" 2>&1 &
done
wait

status=0
for target in $targets; do
    crashes=$(find "findings/$target/default/crashes" -type f -name 'id:*' 2>/dev/null | wc -l | tr -d ' ')
    hangs=$(find "findings/$target/default/hangs" -type f -name 'id:*' 2>/dev/null | wc -l | tr -d ' ')
    summary=$(grep -a 'Statistics' "findings/$target.log" | tail -1 | sed 's/.*Statistics: //; s/\x1b\[[0-9;]*m//g')
    echo "$target: $crashes crashes, $hangs hangs; $summary"
    [ "$crashes" -eq 0 ] || status=1
done
exit "$status"
