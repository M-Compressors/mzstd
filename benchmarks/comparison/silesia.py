#!/usr/bin/env python3
"""Time zstd C, mzstd on main and the working tree on whole Silesia files.

`run` drives two saved `warm` binaries (reused contexts, `file:PATH`
inputs): the working tree's binary times both C and mzstd, the main
branch's binary times its mzstd. Each cell takes five rounds in a seeded
random order of the three sides, with iteration counts calibrated on C;
every process checks its bytes against C before timing. `report` renders
the medians across files as an SVG (requires matplotlib==3.10.8).
"""

import argparse
import gzip
import json
import os
import random
import re
import statistics
import subprocess
from pathlib import Path

SIDES = ("c-zstd", "main", "current")
ROUNDS = 5


def parse_plan(plan):
    """`encode-1,decode3` -> [("encode", -1), ("decode", 3)]."""
    cells = []
    for item in plan.split(","):
        codec, level = item[:6], item[6:]
        if codec not in ("encode", "decode") or not level.lstrip("-").isdigit():
            raise ValueError(f"invalid plan item: {item}")
        cells.append((codec, int(level)))
    return cells


def summarize(rounds):
    """Medians over rounds: each side's time and both mzstd sides' C time / own time."""
    return {
        **{f"{side}_ns": statistics.median(r[side] for r in rounds) for side in SIDES},
        "current_vs_c": statistics.median(r["c-zstd"] / r["current"] for r in rounds),
        "main_vs_c": statistics.median(r["c-zstd"] / r["main"] for r in rounds),
        "current_vs_main": statistics.median(r["main"] / r["current"] for r in rounds),
    }


def medians(results):
    """Per (codec, level), the median across files of each ratio, in plan order."""
    cells = {}
    for key, value in results.items():
        codec, _, level = key.split("/")
        cells.setdefault((codec, int(level[1:])), []).append(value)
    return {cell: {ratio: statistics.median(v[ratio] for v in values)
                   for ratio in ("main_vs_c", "current_vs_c", "current_vs_main")}
            for cell, values in cells.items()}


def table(results, ratio):
    """A Markdown table of one ratio: a row per file plus the median, a column per cell."""
    cells = list(medians(results))
    files = sorted({key.split("/")[1] for key in results})
    lines = ["| File | " + " | ".join(f"{codec.capitalize()} {level}" for codec, level in cells) + " |",
             "| --- |" + " ---: |" * len(cells)]
    for name in files:
        lines.append(f"| {name} | " + " | ".join(
            f"{results[f'{codec}/{name}/l{level}'][ratio]:.3f}" for codec, level in cells) + " |")
    summary = medians(results)
    lines.append("| median | " + " | ".join(f"{summary[cell][ratio]:.3f}" for cell in cells) + " |")
    return lines


def chart(results, path, subtitle):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    from level_docs import SERIES, MAIN, save_svg

    plt.rcParams.update({"svg.fonttype": "none", "svg.hashsalt": "mzstd-silesia", "font.size": 11,
                         "axes.spines.top": False, "axes.spines.right": False, "figure.facecolor": "white"})
    summary = medians(results)
    fig, axes = plt.subplots(1, 2, figsize=(15, 5), width_ratios=[3, 2])
    for ax, codec in zip(axes, ("encode", "decode")):
        cells = [cell for cell in summary if cell[0] == codec]
        for index, (series, ratio) in enumerate([(MAIN, "main_vs_c"), ("mzstd", "current_vs_c")]):
            name, color = SERIES[series]
            positions = [i + (index - .5) * .38 for i in range(len(cells))]
            bars = ax.bar(positions, [summary[cell][ratio] for cell in cells], width=.36, color=color, label=name)
            ax.bar_label(bars, labels=[f"{summary[cell][ratio]:.3f}" for cell in cells], padding=3, fontsize=8)
        ax.set_xticks(range(len(cells)), [str(level) for _, level in cells])
        ax.set_xlabel("Level" if codec == "encode" else "Source level (zstd C frames)")
        ax.set_ylabel(f"Median {codec} speed / C · higher is better")
        ax.set_ylim(0, 1.2)
        ax.axhline(1, color="#64748b", linestyle="--", linewidth=1)
        ax.grid(axis="y", alpha=.18)
        ax.set_axisbelow(True)
        ax.set_title("Compression" if codec == "encode" else "Decompression")
    handles, labels = axes[0].get_legend_handles_labels()
    fig.legend(handles, labels, loc="upper center", bbox_to_anchor=(.5, .93), ncol=2, frameon=False)
    files = len({key.split("/")[1] for key in results})
    fig.suptitle(f"Silesia · median across {files} whole files · reused contexts", fontsize=17)
    fig.text(.5, .02, subtitle + "\nC time / mzstd time per file, interleaved in one process order; dashed line: C = 1×",
             ha="center", fontsize=10)
    fig.tight_layout(rect=(0, .07, 1, .88))
    save_svg(fig, path)
    plt.close(fig)


def load(path):
    """Results from plain or gzipped JSON."""
    data = path.read_bytes()
    return json.loads(gzip.decompress(data) if data[:2] == b"\x1f\x8b" else data)


def run(args):
    files = sorted(os.listdir(args.corpus))
    results = json.loads(args.output.read_text()) if args.output.exists() else {}
    binaries = {"c-zstd": (args.current, "c-zstd"), "main": (args.main, "mzstd"),
                "current": (args.current, "mzstd")}

    def time(side, codec, name, level, iterations):
        binary, implementation = binaries[side]
        output = subprocess.check_output(["taskset", "-c", str(args.cpu), str(binary), codec, implementation,
                                          f"file:{args.corpus / name}", str(level), str(iterations)], text=True)
        return float(re.search(r"([0-9.]+) ns/iteration", output)[1])

    order = random.Random(args.seed)
    for codec, level in parse_plan(args.plan):
        for name in files:
            key = f"{codec}/{name}/l{level}"
            if key in results:
                continue
            iterations = max(1, round(args.seconds * 1e9 / time("c-zstd", codec, name, level, 1)))
            rounds = []
            for _ in range(ROUNDS):
                sides = list(SIDES)
                order.shuffle(sides)
                rounds.append({side: time(side, codec, name, level, iterations) for side in sides})
            results[key] = {"bytes": (args.corpus / name).stat().st_size, "iterations": iterations,
                            **summarize(rounds), "rounds": rounds}
            args.output.write_text(json.dumps(results, indent=1) + "\n")
            print(f"{key:26s} current/C {results[key]['current_vs_c']:.3f} main/C {results[key]['main_vs_c']:.3f}",
                  flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    sweep = commands.add_parser("run", help="time the three sides")
    sweep.add_argument("--current", type=Path, required=True, help="the working tree's warm binary")
    sweep.add_argument("--main", type=Path, required=True, help="the main branch's warm binary")
    sweep.add_argument("--corpus", type=Path, required=True, help="directory of Silesia files")
    sweep.add_argument("--output", type=Path, required=True, help="JSON results, resumed if present")
    sweep.add_argument("--cpu", type=int, default=1)
    sweep.add_argument("--plan", default="encode-1,encode1,encode3,encode5,encode7,encode9,encode12,"
                                         "decode1,decode3,decode9,decode19")
    sweep.add_argument("--seconds", type=float, default=.25, help="target time per process")
    sweep.add_argument("--seed", type=int, default=20261030)
    report = commands.add_parser("report", help="render the median chart")
    report.add_argument("--results", type=Path, required=True)
    report.add_argument("--output", type=Path, required=True)
    report.add_argument("--subtitle", default="Silesia corpus")
    args = parser.parse_args()
    if args.command == "run":
        run(args)
    else:
        chart(load(args.results), args.output, args.subtitle)


if __name__ == "__main__":
    main()
