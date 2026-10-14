#!/usr/bin/env python3
"""Generate level-by-level compression pages and charts from one recorded run.

Requires matplotlib==3.10.8 for SVG generation. Does not run benchmarks or modify
source CSVs, previous reports, or Criterion baselines.
"""

import argparse
import csv
import json
import math
import os
from pathlib import Path
from statistics import median

ENCODERS = {
    "c-zstd": ("zstd C", "#475569"),
    "mzstd": ("mzstd", "#0072B2"),
}
LEVELS = [-5, -1, *range(1, 23)]
CORPORA = {
    "empty": "Empty input; measures API overhead.",
    "tiny-text": "44-byte quick-brown-fox sentence.",
    "alice29": "Alice in Wonderland text from the Canterbury corpus (`testdata/alice29.txt`).",
    "text-1m": "Alice text repeated cyclically to 1 MiB.",
    "binary-64k": "64 KiB of structured binary bytes: ((i / 16) XOR i) modulo 256.",
    "random-64k": "64 KiB prefix of the deterministic pseudorandom corpus.",
    "random-1m": "1 MiB of deterministic xorshift64 pseudorandom bytes.",
    "repeated-1m": "1 MiB of repeated a bytes.",
}


def load_rows(path, *, decoding=False):
    """Reject missing, duplicate, unsupported, or inconsistent measurements."""
    with path.open() as source:
        rows = list(csv.DictReader(source))
    return validate_rows(rows, decoding=decoding)


def validate_rows(rows, *, decoding=False):
    """Validate one codec's complete matrix and normalize numeric columns.

    Both suites require one frame size per dataset and level: the encoders
    write byte-identical frames, and the decoders read one C-written frame.
    """
    expected = {(level, corpus, name) for level in LEVELS for corpus in CORPORA for name in ENCODERS}
    found = set()
    lengths = {}
    frames = {}
    for row in rows:
        for key in ["level", "input_bytes", "compressed_bytes"]:
            row[key] = int(row[key])
        for key in ["mean_ns", "mean_lower_ns", "mean_upper_ns"]:
            row[key] = float(row[key])
            if not math.isfinite(row[key]) or row[key] <= 0:
                raise ValueError(f"invalid timing: {key}")
        key = row["level"], row["corpus"], row["implementation"]
        if key not in expected or key in found:
            raise ValueError(f"duplicate or unsupported case: {key}")
        found.add(key)
        if not row["mean_lower_ns"] <= row["mean_ns"] <= row["mean_upper_ns"]:
            raise ValueError(f"invalid confidence bounds: {key}")
        length = row["input_bytes"]
        if length < 0 or row["compressed_bytes"] <= 0:
            raise ValueError(f"invalid sizes: {key}")
        if (row["corpus"] == "empty") != (length == 0):
            raise ValueError(f"invalid empty corpus: {key}")
        if lengths.setdefault(row["corpus"], length) != length:
            raise ValueError(f"inconsistent input lengths: {key}")
        frame = row["corpus"], row["level"]
        if frames.setdefault(frame, row["compressed_bytes"]) != row["compressed_bytes"]:
            raise ValueError(f"inconsistent frame sizes: {key}")
    if found != expected:
        raise ValueError(f"incomplete matrix: {len(found)} cases; expected {len(expected)}")
    return rows


def cases(rows, level, corpus):
    return sorted((r for r in rows if r["level"] == level and r["corpus"] == corpus),
                  key=lambda r: list(ENCODERS).index(r["implementation"]))


def throughput(row):
    return row["input_bytes"] * 1e9 / row["mean_ns"] / 1048576


def medians(rows, level):
    """Give each dataset equal weight after normalizing to its own C result."""
    reference = {r["corpus"]: r for r in rows
                 if r["level"] == level and r["implementation"] == "c-zstd"}
    result = {}
    for encoder in ENCODERS:
        group = [r for r in rows if r["level"] == level and r["implementation"] == encoder]
        result[encoder] = {
            "speed": median(reference[r["corpus"]]["mean_ns"] / r["mean_ns"] for r in group),
            "size": median(r["compressed_bytes"] / reference[r["corpus"]]["compressed_bytes"] for r in group),
        }
    return result


def dataset_order(rows, level):
    """Rank by mzstd speed / C speed; retain corpus order on ties."""
    scores = {}
    for corpus in CORPORA:
        c, own = cases(rows, level, corpus)
        scores[corpus] = c["mean_ns"] / own["mean_ns"]
    return sorted(CORPORA, key=lambda corpus: -scores[corpus])


def level_order(rows):
    """Put levels with the highest median mzstd speed relative to C first."""
    return sorted(LEVELS, key=lambda level: -medians(rows, level)["mzstd"]["speed"])


def summary(rows, level):
    lines = ["| Implementation | Median speed / C ↑ | Median output / C ↓ | Datasets |",
             "| --- | ---: | ---: | ---: |"]
    for encoder, values in medians(rows, level).items():
        lines.append(f"| {ENCODERS[encoder][0]} | {values['speed']:.3f}× | {values['size']:.3f}× | {len(CORPORA)} |")
    return lines


def number(value):
    """Four significant digits, without scientific notation for large values."""
    return f"{value:,.0f}" if abs(value) >= 1000 else f"{value:,.4g}"


def bars(ax, names, values, colors, label, low=None, high=None, ratio=False):
    """Draw labeled vertical bars with a linear zero baseline and optional bounds."""
    errors = None if low is None else [[v - lo for v, lo in zip(values, low)],
                                      [hi - v for v, hi in zip(values, high)]]
    rectangles = ax.bar(range(len(values)), values, color=colors, width=.62,
                        yerr=errors, capsize=4, error_kw={"elinewidth": 1.2})
    labels = [f"{v:.3f}×" if ratio else number(v) for v in values]
    ax.bar_label(rectangles, labels=labels, padding=7, fontsize=10,
                 bbox={"facecolor": "white", "edgecolor": "none", "alpha": .9, "pad": 1})
    ax.set_xticks(range(len(names)), names, fontsize=10)
    ax.set_ylim(0, max(high if high is not None else values) * 1.25)
    ax.set_ylabel(label)
    ax.ticklabel_format(axis="y", style="plain", useOffset=False)
    ax.grid(axis="y", alpha=.18)
    ax.set_axisbelow(True)
    if ratio:
        ax.axhline(1, color="#64748b", linestyle="--", linewidth=1)


def save_svg(fig, path):
    """Keep generated SVGs deterministic and free of trailing whitespace."""
    fig.savefig(path, metadata={"Date": None})
    path.write_text("\n".join(line.rstrip() for line in path.read_text().splitlines()) + "\n")


def chart(group, level, corpus, path):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from matplotlib.ticker import MaxNLocator

    plt.rcParams.update({"svg.fonttype": "none", "svg.hashsalt": "mzstd-level-docs",
                         "font.size": 11, "axes.spines.top": False, "axes.spines.right": False})
    length = group[0]["input_bytes"]
    latency = length <= 44
    scale = 1 if length == 0 else 1000
    if latency:
        values = [r["mean_ns"] / scale for r in group]
        low = [r["mean_lower_ns"] / scale for r in group]
        high = [r["mean_upper_ns"] / scale for r in group]
        unit = "ns" if length == 0 else "µs"
        label = f"Mean latency ({unit}) · lower is better"
    else:
        values = [throughput(r) for r in group]
        low = [length * 1e9 / r["mean_upper_ns"] / 1048576 for r in group]
        high = [length * 1e9 / r["mean_lower_ns"] / 1048576 for r in group]
        label = "Throughput (MiB/s) · higher is better"
    fig, (speed, size) = plt.subplots(1, 2, figsize=(13, 4.8))
    colors = [ENCODERS[r["implementation"]][1] for r in group]
    names = [ENCODERS[r["implementation"]][0] for r in group]
    bars(speed, names, values, colors, label, low, high)
    sizes = [r["compressed_bytes"] for r in group]
    bars(size, names, sizes, colors, "Compressed bytes · lower is better")
    size.yaxis.set_major_locator(MaxNLocator(nbins=5, integer=True))
    # Preserve exact byte counts even where the axis uses compact ticks.
    for annotation, value in zip(size.texts, sizes):
        annotation.set_text(f"{value:,}")
    fig.suptitle(f"Level {level} · {corpus} · {length:,} input bytes", fontsize=15)
    fig.text(.5, .015, "Linear axes from zero · cold APIs · whiskers: 95% mean confidence bounds",
             ha="center", fontsize=10)
    fig.tight_layout(rect=(0, .045, 1, .95))
    save_svg(fig, path)
    plt.close(fig)


def level_page(rows, level, environment, source_link, environment_link, report_link):
    versions = environment["versions"]
    sampling = environment["sampling"]
    position = LEVELS.index(level)
    navigation = ["[Benchmark index](../README.md)", "[Median overview](README.md)"]
    if position > 0:
        navigation.append(f"[← Level {LEVELS[position - 1]}](l{LEVELS[position - 1]}.md)")
    if position + 1 < len(LEVELS):
        navigation.append(f"[Level {LEVELS[position + 1]} →](l{LEVELS[position + 1]}.md)")
    lines = [f"# Compression level {level}", "", " · ".join(navigation), "",
             f"Recorded run: **{environment['baseline']}** ({environment['date']}).",
             f"[Raw results]({source_link}) · [Environment]({environment_link}) · [Run analysis]({report_link}).", "",
             "## Median across all datasets", "",
             f"Each of the {len(CORPORA)} datasets has equal weight, including empty and tiny input.",
             "Speed / C = C mean latency / implementation mean latency; output / C = implementation bytes / C bytes.",
             f"We take the median of these {len(CORPORA)} per-dataset ratios separately for speed and size.",
             "1× matches C; higher speed and lower output are better. These are medians across datasets,",
             "not median sample latencies, and do not imply statistical significance.", "",
             f"![Median speed and output relative to C at level {level}](charts/l{level}-summary.svg)", "",
             *summary(rows, level), "", "<details>", "<summary>Measurement setup and interpretation</summary>", "",
             f"zstd {versions['c-zstd']} (the C library built by `zstd-ffi`) and mzstd at the recorded",
             "working tree compress identical bytes at this level. Every frame was checked byte-identical",
             "to zstd's and decoded back to its input before any timing.", "",
             "These are cold native API measurements: construction, allocation, compression, and disposal.",
             f"Complete known input; {sampling['samples']} samples, {sampling['warmup_seconds']:g} s warmup, "
             f"at least {sampling['requested_measurement_seconds']:g} s measurement.",
             f"Host: {environment['cpu']}, {environment['affinity']} {environment['rustflags']}.", "",
             "Chart whiskers and table intervals describe 95% confidence bounds on mean timing.",
             "They do not capture all host or allocator variation. Throughput bounds are transformed latency bounds.",
             "Output / input is compressed bytes divided by input bytes; smaller is better, and values above 100% mean expansion.",
             "For empty input, throughput and output / input are undefined (—).",
             f"See the [run analysis]({report_link}#interpretation-limits) for measurement limitations.",
             "", "</details>", "", "## Dataset details", "",
             f"Ordered by mzstd speed / C, highest first; all {len(CORPORA)} datasets are shown.",
             "This order uses recorded mean latency, not statistical significance. Bars start at zero.", "",
             " · ".join(f"[{corpus}](#{corpus})" for corpus in dataset_order(rows, level)), ""]
    for corpus in dataset_order(rows, level):
        group = cases(rows, level, corpus)
        c, own = group
        length = c["input_bytes"]
        scale, unit = (1, "ns") if length == 0 else (1000, "µs")
        lines += [f"### {corpus}", "", f"{CORPORA[corpus]} **Input: {length:,} bytes.**", "",
                  f"mzstd speed / C: **{c['mean_ns'] / own['mean_ns']:.3f}×**; "
                  f"output: {own['compressed_bytes']:,} / {c['compressed_bytes']:,} bytes.", "",
                  f"![Level {level}: {corpus} speed and compressed size](charts/l{level}-{corpus}.svg)", "",
                  "<details>", "<summary>Exact measurements and confidence bounds</summary>", "",
                  f"| Implementation | Mean, {unit} | 95% mean interval, {unit} | MiB/s | Output bytes | Output / input | Speed / C |",
                  "| --- | ---: | ---: | ---: | ---: | ---: | ---: |"]
        for r in group:
            speed = f"{throughput(r):,.2f}" if length else "—"
            fraction = f"{100 * r['compressed_bytes'] / length:.3f}%" if length else "—"
            lines.append(f"| {ENCODERS[r['implementation']][0]} | {r['mean_ns'] / scale:,.4g} | "
                         f"{r['mean_lower_ns'] / scale:,.4g}–{r['mean_upper_ns'] / scale:,.4g} | "
                         f"{speed} | {r['compressed_bytes']:,} | {fraction} | {c['mean_ns'] / r['mean_ns']:.3f}× |")
        lines.extend(["", "</details>", ""])
    return "\n".join(lines)


def main():
    from plot import median_chart

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--csv", type=Path, required=True)
    parser.add_argument("--environment", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    rows = load_rows(args.csv)
    environment = json.loads(args.environment.read_text())
    args.output.mkdir(parents=True, exist_ok=True)
    charts = args.output / "charts"
    charts.mkdir(exist_ok=True)
    links = [Path(os.path.relpath(path.resolve(), args.output.resolve())).as_posix()
             for path in (args.csv, args.environment, args.report)]
    for level in LEVELS:
        median_chart(rows, level, charts / f"l{level}-summary.svg")
        for corpus in CORPORA:
            chart(cases(rows, level, corpus), level, corpus, charts / f"l{level}-{corpus}.svg")
        (args.output / f"l{level}.md").write_text(level_page(rows, level, environment, *links))
    overview = ["# Encoder comparison", "", "[Benchmark index](../README.md)", "",
                f"Recorded run: **{environment['baseline']}** ({environment['date']}).",
                f"[Raw results]({links[0]}) · [Environment]({links[1]}) · [Run analysis]({links[2]}).", "",
                "![Median compression speed and size relative to C](charts/overview.svg)", "",
                f"All {len(CORPORA)} datasets contribute equally to each median, including empty and tiny input.",
                "For each dataset, speed / C = C mean latency / mzstd mean latency;",
                "output / C = mzstd bytes / C bytes. Medians are taken over those ratios.",
                "Levels are ordered by mzstd median speed / C, highest first. Frames are byte-identical,",
                "so output / C is 1× at every level.", "",
                "| Level | mzstd speed / C ↑ | mzstd output / C ↓ | Datasets |",
                "| --- | ---: | ---: | ---: |"]
    for level in level_order(rows):
        values = medians(rows, level)["mzstd"]
        overview.append(f"| [Level {level}](l{level}.md) | {values['speed']:.3f}× | {values['size']:.3f}× | {len(CORPORA)} |")
    for level in level_order(rows):
        overview.extend(["", f"## [Level {level}](l{level}.md)", "",
                         f"![Level {level}: median across all datasets](charts/l{level}-summary.svg)"])
    (args.output / "README.md").write_text("\n".join(overview) + "\n")
    print(f"Generated level overview, {len(LEVELS)} level pages, {len(LEVELS)} median charts and "
          f"{len(LEVELS) * len(CORPORA)} dataset charts from {len(rows)} cases.")


if __name__ == "__main__":
    main()
