#!/usr/bin/env python3
"""Export one complete, named Criterion run with sizes and confidence bounds."""

import argparse
import csv
import json
import math
from pathlib import Path

from level_docs import CORPORA, ENCODERS, LEVELS, validate_rows


def collect(root, baseline, *, decoding=False):
    """Join the preflight size manifest with Criterion's mean estimates."""
    if not baseline or baseline in {".", "..", "new", "base"} or any(c in baseline for c in "/\\"):
        raise ValueError("use a unique named baseline")
    with (root / "sizes.csv").open() as source:
        sizes = list(csv.DictReader(source))
    suite = "decoders" if decoding else "encoders"
    expected = {}
    for row in sizes:
        key = f'{suite}/cold/{row["corpus"]}/l{row["level"]}/{row["implementation"]}'
        if key in expected:
            raise ValueError(f"duplicate size row: {key}")
        expected[key] = row
    measured = {}
    for path in root.rglob(f"{baseline}/benchmark.json"):
        metadata = json.loads(path.read_text())
        key = metadata["full_id"]
        if key not in expected or key in measured:
            raise ValueError(f"unexpected or duplicate measurement: {key}")
        if metadata["throughput"] != {"Bytes": int(expected[key]["input_bytes"])}:
            raise ValueError(f"input size mismatch: {key}")
        estimate = json.loads((path.parent / "estimates.json").read_text())["mean"]
        timing = [estimate["confidence_interval"]["lower_bound"], estimate["point_estimate"],
                  estimate["confidence_interval"]["upper_bound"]]
        if any(not math.isfinite(value) or value <= 0 for value in timing) or timing != sorted(timing):
            raise ValueError(f"invalid timing bounds: {key}")
        measured[key] = estimate
    count = len(CORPORA) * len(LEVELS) * len(ENCODERS)
    if measured.keys() != expected.keys() or len(expected) != count:
        raise ValueError(f"incomplete matrix: {len(measured)} measurements, {len(expected)} sizes; expected {count}")
    rows = []
    for key, size in expected.items():
        estimate = measured[key]
        ns = estimate["point_estimate"]
        reference = measured[key.rsplit("/", 1)[0] + "/c-zstd"]["point_estimate"]
        length = int(size["input_bytes"])
        rows.append({
            **size,
            "mean_ns": ns,
            "mean_lower_ns": estimate["confidence_interval"]["lower_bound"],
            "mean_upper_ns": estimate["confidence_interval"]["upper_bound"],
            "mib_per_second": length * 1e9 / ns / 1048576,
            "speed_relative_to_c": reference / ns,
            "compressed_fraction": int(size["compressed_bytes"]) / length if length else "",
        })
    validate_rows([dict(row) for row in rows], decoding=decoding)
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path)
    parser.add_argument("--decoding", action="store_true", help="export the decoder suite")
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--csv", type=Path, required=True)
    args = parser.parse_args()
    try:
        default = "target/criterion-decoders" if args.decoding else "target/criterion"
        rows = collect(args.root or Path(__file__).parent / default, args.baseline, decoding=args.decoding)
    except (OSError, ValueError, KeyError) as error:
        parser.error(str(error))
    with args.csv.open("w", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=rows[0], lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)
    print(f"Exported {len(rows)} validated size/timing rows to {args.csv}")


if __name__ == "__main__":
    main()
