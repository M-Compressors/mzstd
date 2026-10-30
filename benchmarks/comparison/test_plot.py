#!/usr/bin/env python3
"""Chart tests; skipped without matplotlib."""

import tempfile
import unittest
from pathlib import Path

try:
    import matplotlib
except ImportError:  # pragma: no cover - plotting is optional for the other tests
    matplotlib = None


@unittest.skipIf(matplotlib is None, "matplotlib is not installed")
class Charts(unittest.TestCase):
    def test_every_chart_renders_from_a_complete_matrix(self):
        from decoder_docs import level_chart, overview_chart
        from level_docs import cases, chart, validate_rows
        from plot import median_chart, render
        from test_level_docs import matrix

        rows = validate_rows(matrix())
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            render(rows, out, "unit")
            median_chart(rows, 3, out / "l3-summary.svg")
            chart(cases(rows, 3, "alice29"), 3, "alice29", out / "l3-alice29.svg")
            chart(cases(rows, 3, "empty"), 3, "empty", out / "l3-empty.svg")
            overview_chart(rows, out / "dec-overview.svg", "unit")
            level_chart(rows, 3, out / "dec-l3.svg")
            main = validate_rows(matrix(c=900.0))
            render(rows, out / "main", "unit", main)
            median_chart(rows, 3, out / "main" / "l3-summary.svg", main)
            overview_chart(rows, out / "main" / "dec-overview.svg", "unit", main)
            self.assertIn("mzstd main", (out / "main" / "overview.svg").read_text())
            self.assertIn("mzstd main", (out / "main" / "dec-overview.svg").read_text())
            names = sorted(p.name for p in out.iterdir() if p.is_file())
            self.assertEqual(names, ["dec-l3.svg", "dec-overview.svg", "l3-alice29.svg", "l3-empty.svg",
                                     "l3-summary.svg", "overview.svg", "size.svg", "throughput.svg"])
            text = (out / "overview.svg").read_text()
            self.assertNotIn("<dc:date>", text)
            self.assertFalse(any(line != line.rstrip() for line in text.splitlines()))


if __name__ == "__main__":
    unittest.main()
