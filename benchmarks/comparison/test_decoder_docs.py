#!/usr/bin/env python3
"""Unit tests for the decoder pages (no matplotlib needed)."""

import unittest

from decoder_docs import level_page
from level_docs import CORPORA, LEVELS, validate_rows
from test_level_docs import ENVIRONMENT, matrix


class DecoderPages(unittest.TestCase):
    def test_level_pages_report_medians_and_every_dataset(self):
        rows = validate_rows(matrix(), decoding=True)
        page = level_page(rows, 3, ENVIRONMENT, ["d.csv", "e.json", "r.md"])
        self.assertIn("# Decompression of level 3 frames", page)
        self.assertIn("| mzstd | 1.250× |", page)
        self.assertIn("charts/l3.svg", page)
        for corpus in CORPORA:
            self.assertIn(f"## {corpus}", page)
        self.assertIn("compressed / original: undefined", page)

    def test_frames_must_share_one_size_per_level(self):
        rows = matrix()
        rows[1]["compressed_bytes"] = "99"
        with self.assertRaises(ValueError):
            validate_rows(rows, decoding=True)
        self.assertEqual(len(LEVELS), 24)


if __name__ == "__main__":
    unittest.main()
