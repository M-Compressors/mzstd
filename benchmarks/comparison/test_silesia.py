#!/usr/bin/env python3
"""Unit tests for the Silesia summaries (no matplotlib or corpus needed)."""

import gzip
import json
import tempfile
import unittest
from pathlib import Path

from silesia import load, medians, parse_plan, summarize, table


def results():
    """Two files and two cells; mzstd main runs at 0.8× C, the working tree at 1.25×."""
    data = {}
    for codec, level in [("encode", -1), ("decode", 3)]:
        for name, c in [("dickens", 100.0), ("x-ray", 200.0)]:
            data[f"{codec}/{name}/l{level}"] = summarize([{"c-zstd": c, "main": c / .8, "current": c / 1.25}] * 5)
    return data


class Silesia(unittest.TestCase):
    def test_plans_name_codec_and_level(self):
        self.assertEqual(parse_plan("encode-1,decode19"), [("encode", -1), ("decode", 19)])
        for plan in ["encode", "compress1", "decodex"]:
            with self.assertRaises(ValueError):
                parse_plan(plan)

    def test_rounds_reduce_to_median_ratios(self):
        rounds = [{"c-zstd": 100.0, "main": 125.0, "current": 80.0},
                  {"c-zstd": 100.0, "main": 100.0, "current": 100.0},
                  {"c-zstd": 110.0, "main": 125.0, "current": 88.0}]
        value = summarize(rounds)
        self.assertEqual(value["c-zstd_ns"], 100.0)
        self.assertAlmostEqual(value["current_vs_c"], 1.25)
        self.assertAlmostEqual(value["main_vs_c"], .88)
        self.assertAlmostEqual(value["current_vs_main"], 125 / 88)

    def test_cells_keep_plan_order_and_take_medians_across_files(self):
        summary = medians(results())
        self.assertEqual(list(summary), [("encode", -1), ("decode", 3)])
        self.assertAlmostEqual(summary[("decode", 3)]["main_vs_c"], .8)
        self.assertAlmostEqual(summary[("encode", -1)]["current_vs_main"], 1.5625)

    def test_tables_list_every_file_and_the_median(self):
        lines = table(results(), "current_vs_c")
        self.assertEqual(lines[0], "| File | Encode -1 | Decode 3 |")
        self.assertEqual(lines[2], "| dickens | 1.250 | 1.250 |")
        self.assertEqual(lines[-1], "| median | 1.250 | 1.250 |")
        self.assertEqual(len(lines), 5)


    def test_results_load_from_plain_or_gzipped_json(self):
        with tempfile.TemporaryDirectory() as directory:
            plain, packed = Path(directory) / "r.json", Path(directory) / "r.json.gz"
            plain.write_text(json.dumps(results()))
            packed.write_bytes(gzip.compress(plain.read_bytes()))
            self.assertEqual(load(plain), load(packed))


if __name__ == "__main__":
    unittest.main()
