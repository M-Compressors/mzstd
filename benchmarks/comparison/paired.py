#!/usr/bin/env python3
"""Collect interleaved API timings from two saved profile binaries.

Each binary validates its selected case before timing. Iteration counts come
from a comparison CSV, so both sides of a pair do identical amounts of work.
The output contains raw pairs, not a statistical significance claim.
"""

import argparse
import csv
import json
from pathlib import Path
import random
import re
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--csv", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--codec", choices=["encode", "decode"], default="encode")
    parser.add_argument("--boundary", choices=["cold", "warm"], default="cold")
    parser.add_argument("--baseline-implementation", choices=["mzstd", "c-zstd"], default="mzstd")
    parser.add_argument("--candidate-implementation", choices=["mzstd", "c-zstd"], default="mzstd")
    parser.add_argument("--filter", default=".")
    parser.add_argument("--cpu", type=int, default=2)
    parser.add_argument("--pairs", type=int, default=10)
    parser.add_argument("--seconds", type=float, default=0.15)
    parser.add_argument("--seed", type=int, default=20261014)
    args = parser.parse_args()
    if args.pairs < 1 or not 0 < args.seconds < float("inf"):
        parser.error("pairs and seconds must be positive and finite")
    if args.output.exists():
        parser.error("use a new output path to preserve existing measurements")
    binaries = [str(path.resolve(strict=True)) for path in (args.baseline, args.candidate)]
    implementations = [args.baseline_implementation, args.candidate_implementation]
    pattern = re.compile(args.filter)
    with args.csv.open(newline="") as source:
        rows = [
            row for row in csv.DictReader(source)
            if row["implementation"] == "mzstd"
            and pattern.search(f'{row["corpus"]}/l{row["level"]}')
        ]
    if not rows:
        parser.error("no matching cases")
    rng = random.Random(args.seed)
    data = {
        "schema_version": 1, "metric": "time", "unit": "ns",
        "environment": {
            "binaries": binaries, "cpu": args.cpu, "codec": args.codec,
            "implementations": implementations,
            "seconds_requested": args.seconds, "seed": args.seed,
            "calibration_csv": str(args.csv.resolve()),
            "boundary": (
                "cold API including construction, allocation and disposal"
                if args.boundary == "cold"
                else "warm API with reusable contexts and destinations"
            ),
        },
        "cases": [],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    for row in rows:
        mean = float(row["mean_ns"])
        if not 0 < mean < float("inf"):
            raise ValueError("calibration times must be positive and finite")
        iterations = max(5, round(args.seconds * 1e9 / mean))
        case = {
            "name": f'{row["corpus"]}/l{row["level"]}',
            "iterations": iterations, "pairs": [],
        }
        for _ in range(args.pairs):
            order = [0, 1]
            rng.shuffle(order)
            times = [None, None]
            for side in order:
                output = subprocess.check_output([
                    "taskset", "-c", str(args.cpu), binaries[side], args.codec,
                    implementations[side], row["corpus"], row["level"], str(iterations),
                ], text=True)
                match = re.search(r"([0-9.]+) ns/iteration", output)
                if match is None:
                    raise ValueError(f"missing timing in {output!r}")
                sizes = re.search(r"(\d+) input bytes, (\d+) frame bytes", output)
                if sizes is None:
                    raise ValueError(f"missing sizes in {output!r}")
                for key, value in zip(("input_bytes", "frame_bytes"), sizes.groups()):
                    size = int(value)
                    if case.setdefault(key, size) != size:
                        raise ValueError(f"{case['name']}: {key} differs between runs")
                times[side] = float(match[1])
            case["pairs"].append({
                "baseline": times[0], "candidate": times[1],
                "order": "AB" if order[0] == 0 else "BA",
            })
        data["cases"].append(case)
        args.output.write_text(json.dumps(data, indent=2) + "\n")
        print(f'{case["name"]}: {len(case["pairs"])} pairs saved', flush=True)


if __name__ == "__main__":
    main()
