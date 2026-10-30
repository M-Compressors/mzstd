#!/usr/bin/env python3
"""Generate decoder comparison pages and SVGs from one complete recorded run."""

import argparse
import json
import os
from pathlib import Path

from level_docs import (CORPORA, ENCODERS, LEVELS, SERIES, bars, cases, dataset_order, level_order,
                        load_rows, main_note, medians, save_svg, throughput)

DECODERS = ENCODERS


def overview_chart(rows, path, subtitle, main=None):
    import matplotlib.pyplot as plt
    from plot import grouped

    order = level_order(rows)
    fig, ax = plt.subplots(figsize=(16, 5))
    grouped(ax, order, {level: medians(rows, level, main) for level in order}, "speed")
    ax.set_ylim(bottom=0)
    ax.set_ylabel("Median decode speed / C · higher is better")
    ax.set_xlabel("Source encoder level (zstd C)")
    ax.axhline(1, color="#64748b", linestyle="--", linewidth=1)
    ax.grid(axis="y", alpha=.18)
    ax.set_axisbelow(True)
    ax.legend(ncol=len(ax.get_legend_handles_labels()[1]), loc="upper center", bbox_to_anchor=(.5, 1.15), frameon=False)
    fig.suptitle(f"Decompression · median across all {len(CORPORA)} datasets", fontsize=18)
    fig.text(.5, .02, subtitle + "\nIdentical compressed inputs · equal dataset weight",
             ha="center", fontsize=10)
    fig.tight_layout(rect=(0, .09, 1, .91))
    save_svg(fig, path)
    plt.close(fig)


def level_chart(rows, level, path):
    import matplotlib.pyplot as plt

    count = len(CORPORA)
    fig, axes = plt.subplots(count // 2, 2, figsize=(13, count * 2))
    for ax, corpus in zip(axes.flat, dataset_order(rows, level)):
        group = cases(rows, level, corpus)
        length = group[0]["input_bytes"]
        if length <= 44:
            values = [r["mean_ns"] / 1000 for r in group]
            low = [r["mean_lower_ns"] / 1000 for r in group]
            high = [r["mean_upper_ns"] / 1000 for r in group]
            label = "Mean latency (µs) · lower is better"
        else:
            values = [throughput(r) for r in group]
            low = [length * 1e9 / r["mean_upper_ns"] / 1048576 for r in group]
            high = [length * 1e9 / r["mean_lower_ns"] / 1048576 for r in group]
            label = "Decoded MiB/s · higher is better"
        bars(ax, [DECODERS[r["implementation"]][0] for r in group], values,
             [DECODERS[r["implementation"]][1] for r in group], label, low, high)
        ax.set_title(f"{corpus} · {group[0]['compressed_bytes']:,} → {length:,} bytes")
    fig.suptitle(f"Decompression · source level {level} · identical C frames", fontsize=18)
    fig.text(.5, .015, "Cold APIs · linear axes from zero · whiskers: 95% confidence bounds on mean timing",
             ha="center", fontsize=10)
    fig.tight_layout(rect=(0, .04, 1, .97))
    save_svg(fig, path)
    plt.close(fig)


def level_page(rows, level, environment, links, main=None):
    lines = [f"# Decompression of level {level} frames", "",
             "[Decoder overview](README.md) · [Benchmark index](../README.md)", "",
             f"Recorded run: **{environment['baseline']}** ({environment['date']}).",
             f"[CSV]({links[0]}) · [Environment]({links[1]}) · [Methodology and limits]({links[2]}).", "",
             "Level belongs to the C encoder that prepared the frame; decoders have no level setting.",
             "Both decoders restore the same bytes.", "",
             f"## Median speed across {len(CORPORA)} datasets", "",
             "Median of per-dataset C mean time / decoder mean time ratios, with equal weight.",
             "Higher is faster; 1× matches C. No confidence interval",
             "is inferred for this across-dataset median.", "",
             "| Decoder | Median speed / C |", "| --- | ---: |"]
    for decoder, value in medians(rows, level, main).items():
        lines.append(f"| {SERIES[decoder][0]} | {value['speed']:.3f}× |")
    if main:
        lines += ["", main_note(environment)]
    lines += ["", f"![Decompression by dataset at source level {level}](charts/l{level}.svg)", "",
              "Datasets are ranked by mzstd speed relative to C, highest first.",
              "Throughput counts restored bytes; empty input has latency only. Whiskers and intervals",
              "describe sampling uncertainty, not all host variation. Cold APIs include allocation",
              "and disposal. C reads the frame's content size and decodes into a buffer of exactly",
              "that size; mzstd returns its own vector.", ""]
    for corpus in dataset_order(rows, level):
        group = cases(rows, level, corpus)
        length, compressed = group[0]["input_bytes"], group[0]["compressed_bytes"]
        fraction = f"{compressed / length:.3%}" if length else "undefined"
        reference = next(r["mean_ns"] for r in group if r["implementation"] == "c-zstd")
        lines += [f"## {corpus}", "", CORPORA[corpus], "",
                  f"**{compressed:,} compressed → {length:,} restored bytes; compressed / original: {fraction}.**", "",
                  "| Decoder | Mean µs | 95% mean interval µs | Decoded MiB/s | Speed / C |",
                  "| --- | ---: | ---: | ---: | ---: |"]
        for row in group:
            speed = f"{throughput(row):,.2f}" if length else "—"
            lines.append(f"| {DECODERS[row['implementation']][0]} | {row['mean_ns'] / 1000:,.4g} | "
                         f"{row['mean_lower_ns'] / 1000:,.4g}–{row['mean_upper_ns'] / 1000:,.4g} | "
                         f"{speed} | {reference / row['mean_ns']:.3f}× |")
        if main:
            c, own = cases(main, level, corpus)
            lines.append(f"| mzstd main (separate run) | {own['mean_ns'] / 1000:,.4g} | "
                         f"{own['mean_lower_ns'] / 1000:,.4g}–{own['mean_upper_ns'] / 1000:,.4g} | "
                         f"{f'{throughput(own):,.2f}' if length else '—'} | {c['mean_ns'] / own['mean_ns']:.3f}× |")
        lines.append("")
    return "\n".join(lines)


def generate(rows, environment, output, sources, main=None):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    plt.rcParams.update({"svg.fonttype": "none", "svg.hashsalt": "mzstd-decoder-comparison",
                         "font.size": 11, "axes.spines.top": False, "axes.spines.right": False,
                         "figure.facecolor": "white"})
    output.mkdir(parents=True, exist_ok=True)
    charts = output / "charts"
    charts.mkdir(exist_ok=True)
    links = [Path(os.path.relpath(path.resolve(), output.resolve())).as_posix() for path in sources]
    overview_chart(rows, charts / "overview.svg",
                   f"{environment['cpu']} · {environment['date']} · {environment['sampling']['samples']} samples", main)
    lines = ["# Decoder comparison", "", "[Benchmark index](../README.md)", "",
             f"Recorded run: **{environment['baseline']}** ({environment['date']}).",
             f"[CSV]({links[0]}) · [Environment]({links[1]}) · [Methodology and limits]({links[2]}).", "",
             "![Median decompression speed relative to C](charts/overview.svg)", "",
             f"Each value is the median of C mean time / decoder mean time across all {len(CORPORA)} datasets,",
             "equally weighted. Higher is faster; 1× matches C.",
             "Levels are ordered by mzstd median speed / C. Medians do not imply statistical significance.", "",
             "Both decoders restore identical C-written frames. The level is an encoder setting,",
             "not a decoder setting. Compressed sizes and ratios are properties of those shared inputs,",
             "so there is no decoder size ranking.", "",
             *([main_note(environment), ""] if main else []),
             "| Source level | " + " | ".join(SERIES[d][0] for d in medians(rows, LEVELS[0], main)) + " |",
             "| --- |" + " ---: |" * len(medians(rows, LEVELS[0], main))]
    for level in level_order(rows):
        values = medians(rows, level, main)
        lines.append(f"| [{level}](l{level}.md) | " + " | ".join(f"{v['speed']:.3f}×" for v in values.values()) + " |")
    lines += ["", "Open a source level for all datasets, compressed/restored byte counts,",
              "mean latency, confidence bounds and throughput. Measurements include cold construction,",
              "allocation, decoding and disposal. C receives the frame's declared output capacity.",
              "See the linked methodology for reproducible commands and the limits of this single-host comparison.", ""]
    (output / "README.md").write_text("\n".join(lines))
    for level in LEVELS:
        level_chart(rows, level, charts / f"l{level}.svg")
        (output / f"l{level}.md").write_text(level_page(rows, level, environment, links, main))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--csv", type=Path, required=True)
    parser.add_argument("--environment", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--main-csv", type=Path, help="the same matrix measured on the main branch")
    args = parser.parse_args()
    rows = load_rows(args.csv, decoding=True)
    main = load_rows(args.main_csv, decoding=True) if args.main_csv else None
    environment = json.loads(args.environment.read_text())
    generate(rows, environment, args.output, [args.csv, args.environment, args.report], main)
    print(f"Generated decoder overview, {len(LEVELS)} level pages and {len(LEVELS) + 1} SVGs from {len(rows)} measurements.")


if __name__ == "__main__":
    main()
