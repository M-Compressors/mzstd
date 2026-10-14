#!/usr/bin/env python3
"""Unit tests for the level pages (no matplotlib needed)."""

import unittest

from level_docs import (CORPORA, ENCODERS, LEVELS, cases, dataset_order, level_order, level_page,
                        medians, number, summary, validate_rows)

LENGTHS = {"empty": 0, "tiny-text": 44, "alice29": 152089, "text-1m": 1 << 20,
           "binary-64k": 1 << 16, "random-64k": 1 << 16, "random-1m": 1 << 20, "repeated-1m": 1 << 20}
ENVIRONMENT = {"baseline": "unit", "date": "2026-10-13", "cpu": "test CPU", "affinity": "one core.",
               "rustflags": "defaults", "versions": {"c-zstd": "1.5.7", "mzstd": "0.2.0"},
               "sampling": {"samples": 20, "warmup_seconds": 1, "requested_measurement_seconds": 3}}


def matrix(fast=lambda level, corpus: 800.0):
    """A complete matrix; mzstd's time per case comes from `fast`."""
    rows = []
    for level in LEVELS:
        for corpus in CORPORA:
            for name in ENCODERS:
                ns = 1000.0 if name == "c-zstd" else fast(level, corpus)
                rows.append({"corpus": corpus, "input_bytes": str(LENGTHS[corpus]), "level": str(level),
                             "implementation": name, "compressed_bytes": "100", "mean_ns": str(ns),
                             "mean_lower_ns": str(ns - 1), "mean_upper_ns": str(ns + 1)})
    return rows


class Validation(unittest.TestCase):
    def test_a_complete_matrix_is_normalized(self):
        rows = validate_rows(matrix())
        self.assertEqual(len(rows), len(LEVELS) * len(CORPORA) * len(ENCODERS))
        self.assertIsInstance(rows[0]["level"], int)
        self.assertAlmostEqual(medians(rows, 3)["mzstd"]["speed"], 1.25)
        self.assertAlmostEqual(medians(rows, 3)["c-zstd"]["size"], 1.0)

    def test_missing_duplicate_and_inconsistent_cases_are_rejected(self):
        for corrupt in [lambda r: r.pop(),
                        lambda r: r.append(dict(r[0])),
                        lambda r: r[0].update(mean_ns="nan"),
                        lambda r: r[0].update(mean_lower_ns="2000"),
                        lambda r: r[1].update(compressed_bytes="99"),
                        lambda r: r[1].update(compressed_bytes="0"),
                        lambda r: r[2].update(input_bytes="1"),
                        lambda r: r[0].update(input_bytes="5"),
                        lambda r: r[0].update(level="23")]:
            rows = matrix()
            corrupt(rows)
            with self.assertRaises(ValueError):
                validate_rows(rows)


class Ordering(unittest.TestCase):
    def test_levels_and_datasets_rank_by_mzstd_speed(self):
        rows = validate_rows(matrix(lambda level, corpus: 500.0 if level == 7 or corpus == "alice29" else 900.0))
        self.assertEqual(level_order(rows)[0], 7)
        self.assertEqual(dataset_order(rows, 3)[0], "alice29")
        self.assertEqual([r["implementation"] for r in cases(rows, 3, "empty")], list(ENCODERS))


class Pages(unittest.TestCase):
    def test_pages_link_neighbours_and_list_every_dataset(self):
        rows = validate_rows(matrix())
        page = level_page(rows, 3, ENVIRONMENT, "a.csv", "e.json", "r.md")
        self.assertIn("[← Level 2](l2.md)", page)
        self.assertIn("[Level 4 →](l4.md)", page)
        self.assertIn("charts/l3-summary.svg", page)
        for corpus in CORPORA:
            self.assertIn(f"### {corpus}", page)
            self.assertIn(f"charts/l3-{corpus}.svg", page)
        self.assertNotIn("←", level_page(rows, LEVELS[0], ENVIRONMENT, "a", "e", "r"))
        self.assertNotIn("→", level_page(rows, LEVELS[-1], ENVIRONMENT, "a", "e", "r"))
        self.assertIn("| mzstd | 1.250× | 1.000× | 8 |", summary(rows, 3))

    def test_numbers_avoid_scientific_notation(self):
        self.assertEqual(number(48470.2), "48,470")
        self.assertEqual(number(0.02966), "0.02966")
        self.assertEqual(number(311.8), "311.8")


if __name__ == "__main__":
    unittest.main()
