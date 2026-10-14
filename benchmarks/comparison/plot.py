#!/usr/bin/env python3
"""Render vertical SVG bars from a complete compression CSV; requires matplotlib==3.10.8."""

import argparse
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt

from level_docs import CORPORA, ENCODERS, LEVELS, bars, level_order, load_rows, medians, save_svg


def style():
    plt.rcParams.update({
        "font.size": 11, "axes.spines.top": False, "axes.spines.right": False,
        "svg.fonttype": "none", "svg.hashsalt": "mzstd-comparison",
        "figure.facecolor": "white",
    })


def median_chart(rows, level, path):
    """Compare per-dataset relative speed and size without mixing units or weights."""
    style()
    values = medians(rows, level)
    names = [ENCODERS[e][0] for e in values]
    colors = [ENCODERS[e][1] for e in values]
    fig, axes = plt.subplots(1, 2, figsize=(13, 4.8))
    for ax, metric, label in zip(axes, ["speed", "size"],
                               ["Median speed / C · higher is better", "Median output / C · lower is better"]):
        bars(ax, names, [v[metric] for v in values.values()], colors, label, ratio=True)
    fig.suptitle(f"Level {level} · median across all {len(CORPORA)} datasets", fontsize=17)
    fig.text(.5, .02, "Equal dataset weight, including empty input · per-dataset ratios to zstd C · dashed line: C = 1×",
             ha="center", fontsize=10)
    fig.tight_layout(rect=(0, .06, 1, .95))
    save_svg(fig, path)
    plt.close(fig)


def render(rows, output, subtitle):
    """Write median speed, size and paired overview bars; strongest mzstd levels first."""
    output.mkdir(parents=True, exist_ok=True)
    style()
    order = level_order(rows)
    summaries = {level: medians(rows, level) for level in order}
    for kind, metric, label in [("throughput", "speed", "Median speed / C · higher is better"),
                                ("size", "size", "Median output / C · lower is better")]:
        fig, axes = plt.subplots(6, 4, figsize=(16, 22))
        for ax, level in zip(axes.flat, order):
            values = summaries[level]
            bars(ax, [ENCODERS[e][0] for e in values],
                 [v[metric] for v in values.values()], [ENCODERS[e][1] for e in values], "", ratio=True)
            ax.set_title(f"Level {level}")
        fig.suptitle(label + f"\nMedian across all {len(CORPORA)} datasets", fontsize=19)
        fig.text(.5, .012, subtitle + "\nEqual dataset weight · strongest mzstd median speed / C first",
                 ha="center", fontsize=10)
        fig.tight_layout(rect=(0, .03, 1, .96))
        save_svg(fig, output / f"{kind}.svg")
        plt.close(fig)
    fig, axes = plt.subplots(2, 1, figsize=(16, 8))
    for ax, metric, label in zip(axes, ["speed", "size"],
                               ["Median speed / C · higher is better", "Median output / C · lower is better"]):
        for index, (encoder, (name, color)) in enumerate(ENCODERS.items()):
            positions = [i + (index - .5) * .38 for i in range(len(order))]
            ax.bar(positions, [summaries[level][encoder][metric] for level in order],
                   width=.36, color=color, label=name)
        ax.set_xticks(range(len(order)), [str(level) for level in order])
        ax.set_ylim(bottom=0)
        ax.set_ylabel(label)
        ax.axhline(1, color="#64748b", linestyle="--", linewidth=1)
        ax.grid(axis="y", alpha=.18)
        ax.set_axisbelow(True)
    axes[1].set_xlabel("Level")
    fig.suptitle(f"Speed and size · median across all {len(CORPORA)} datasets", fontsize=19)
    handles, labels = axes[0].get_legend_handles_labels()
    fig.legend(handles, labels, loc="upper center", bbox_to_anchor=(.5, .94), ncol=len(ENCODERS), frameon=False)
    fig.text(.5, .02, subtitle + "\nPer-dataset ratios to zstd C · equal weight · levels ordered by mzstd median speed / C",
             ha="center", fontsize=10)
    fig.tight_layout(rect=(0, .07, 1, .89))
    save_svg(fig, output / "overview.svg")
    plt.close(fig)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--csv", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--subtitle", default="Implementation comparison")
    args = parser.parse_args()
    render(load_rows(args.csv), args.output, args.subtitle)


if __name__ == "__main__":
    main()
