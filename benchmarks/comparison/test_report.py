#!/usr/bin/env python3
"""Unit tests for the Criterion exporter."""

import csv
import json
import tempfile
import unittest
from pathlib import Path

from report import collect
from test_level_docs import matrix


def write(root, rows, skip=None, suite="encoders"):
    with (root / "sizes.csv").open("w", newline="") as out:
        writer = csv.DictWriter(out, fieldnames=["corpus", "input_bytes", "level", "implementation",
                                                 "compressed_bytes"])
        writer.writeheader()
        for row in rows:
            writer.writerow({k: row[k] for k in writer.fieldnames})
    for row in rows:
        key = f'{suite}/cold/{row["corpus"]}/l{row["level"]}/{row["implementation"]}'
        if key == skip:
            continue
        directory = root / key.replace("/", "_") / "unit"
        directory.mkdir(parents=True)
        (directory / "benchmark.json").write_text(json.dumps(
            {"full_id": key, "throughput": {"Bytes": int(row["input_bytes"])}}))
        ns = float(row["mean_ns"])
        (directory / "estimates.json").write_text(json.dumps({"mean": {
            "point_estimate": ns, "confidence_interval": {"lower_bound": ns - 1, "upper_bound": ns + 1}}}))


class Collect(unittest.TestCase):
    def test_a_complete_run_exports_ratios_for_either_suite(self):
        for suite, decoding in [("encoders", False), ("decoders", True)]:
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                write(root, matrix(), suite=suite)
                rows = collect(root, "unit", decoding=decoding)
                mzstd = next(r for r in rows if r["implementation"] == "mzstd")
                self.assertAlmostEqual(mzstd["speed_relative_to_c"], 1.25)

    def test_incomplete_runs_and_reserved_baselines_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write(root, matrix(), skip="encoders/cold/alice29/l3/mzstd")
            with self.assertRaises(ValueError):
                collect(root, "unit")
            for name in ["new", "base", "a/b", ""]:
                with self.assertRaises(ValueError):
                    collect(root, name)


if __name__ == "__main__":
    unittest.main()
