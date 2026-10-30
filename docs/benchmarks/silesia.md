# Silesia — 2026-10-30

[Benchmark index](README.md) · [Raw results](silesia-2026-10-30/silesia.json.gz)

All twelve files of the Silesia corpus (211,938,580 bytes), each compressed
or decompressed whole with a context and destination reused across
iterations (the comparison harness's `warm` driver). Three sides are timed:
zstd 1.5.7, the main branch (`0a4fd87`) and the working tree. Every cell
runs five rounds of the three in a seeded random order, each process at
about 0.25 s of iterations calibrated on C, and every process checks its
frame or output against C before timing. Values are medians over the five
rounds of C time / mzstd time; above 1, mzstd is faster.

![Median Silesia speed relative to C](silesia-2026-10-30/silesia.svg)

## Working tree against C

| File | Encode -1 | Encode 1 | Encode 3 | Encode 5 | Encode 7 | Encode 9 | Encode 12 | Decode 1 | Decode 3 | Decode 9 | Decode 19 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| dickens | 0.935 | 0.950 | 1.027 | 0.957 | 0.951 | 0.960 | 1.033 | 0.932 | 0.957 | 0.972 | 0.979 |
| mozilla | 0.941 | 0.948 | 0.874 | 0.967 | 0.948 | 0.937 | 1.093 | 0.956 | 0.951 | 0.984 | 0.966 |
| mr | 0.946 | 0.942 | 0.947 | 0.967 | 0.962 | 0.982 | 1.056 | 0.904 | 0.961 | 0.962 | 0.965 |
| nci | 0.946 | 0.947 | 1.053 | 0.916 | 0.915 | 0.932 | 0.958 | 0.959 | 0.953 | 0.950 | 0.960 |
| ooffice | 0.909 | 0.932 | 0.845 | 0.980 | 0.943 | 0.965 | 1.024 | 0.920 | 0.939 | 0.952 | 0.943 |
| osdb | 0.930 | 0.935 | 0.908 | 0.961 | 0.950 | 0.992 | 1.057 | 1.014 | 1.047 | 1.040 | 1.023 |
| reymont | 0.957 | 0.962 | 1.061 | 0.942 | 0.943 | 0.955 | 1.022 | 0.981 | 0.993 | 1.032 | 1.071 |
| samba | 0.937 | 0.945 | 0.984 | 0.947 | 0.957 | 0.941 | 1.009 | 0.999 | 0.991 | 1.003 | 1.067 |
| sao | 0.898 | 0.917 | 0.854 | 1.002 | 0.987 | 1.030 | 1.079 | 0.941 | 0.910 | 0.931 | 0.911 |
| webster | 0.943 | 0.945 | 1.013 | 0.942 | 0.947 | 1.002 | 1.040 | 1.005 | 0.999 | 1.044 | 1.058 |
| x-ray | 1.041 | 0.971 | 0.835 | 0.987 | 0.972 | 0.988 | 1.105 | 0.848 | 0.924 | 0.958 | 0.953 |
| xml | 0.930 | 0.931 | 0.993 | 0.930 | 0.931 | 0.925 | 1.025 | 1.009 | 1.012 | 0.998 | 0.985 |
| median | 0.939 | 0.945 | 0.966 | 0.959 | 0.949 | 0.962 | 1.036 | 0.958 | 0.959 | 0.978 | 0.973 |

- **Compression** trails at most levels on these large inputs. The `fast`
  levels -1 and 1 run at 0.94× (text files 0.93–0.96×): a match costs
  about 127 instructions against the reference's 91, of which about 25
  move values the loop cannot keep in registers and 12 are bounds tests
  ([encoder core](../../architecture/encoder-core.md#known-gaps)). The row
  levels 5 to 9 run at 0.95–0.96×, dfast (level 3) at 0.97× with
  ooffice, sao, x-ray and mozilla at 0.84–0.87×, and btlazy2 (level 12) at 1.04×.
- **Decompression** runs at 0.96–0.98×. It trails most on literal-heavy
  binaries (x-ray level 1 0.85×, mr 0.90×, ooffice 0.92×), where both
  decoders spend most of their time in the four-stream Huffman loop, and is
  at or above parity on osdb, webster and xml.

## Working tree against main

| File | Encode -1 | Encode 1 | Encode 3 | Encode 5 | Encode 7 | Encode 9 | Encode 12 | Decode 1 | Decode 3 | Decode 9 | Decode 19 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| dickens | 1.023 | 1.040 | 1.019 | 1.014 | 1.015 | 1.028 | 1.032 | 1.155 | 1.113 | 1.100 | 1.079 |
| mozilla | 1.001 | 1.027 | 0.988 | 1.026 | 1.014 | 1.016 | 1.041 | 1.078 | 1.045 | 1.059 | 1.011 |
| mr | 1.003 | 1.026 | 0.992 | 1.022 | 1.017 | 1.027 | 1.122 | 1.181 | 1.102 | 1.101 | 1.119 |
| nci | 1.013 | 1.018 | 1.002 | 1.011 | 1.011 | 1.020 | 1.023 | 1.050 | 1.042 | 1.042 | 1.030 |
| ooffice | 0.993 | 1.024 | 0.964 | 1.005 | 1.003 | 1.028 | 1.222 | 1.152 | 1.063 | 1.073 | 1.062 |
| osdb | 1.017 | 1.025 | 0.976 | 1.005 | 0.997 | 1.030 | 1.122 | 1.067 | 1.087 | 1.096 | 1.073 |
| reymont | 1.022 | 1.041 | 1.014 | 1.015 | 1.025 | 1.017 | 1.028 | 1.113 | 1.097 | 1.088 | 1.075 |
| samba | 1.009 | 1.019 | 1.003 | 1.010 | 1.015 | 1.025 | 1.077 | 1.089 | 1.063 | 1.055 | 1.120 |
| sao | 1.002 | 1.029 | 0.970 | 0.968 | 0.993 | 1.005 | 1.260 | 1.062 | 1.064 | 1.064 | 1.103 |
| webster | 1.019 | 1.035 | 1.018 | 1.010 | 1.022 | 1.015 | 0.990 | 1.121 | 1.108 | 1.104 | 1.071 |
| x-ray | 0.960 | 1.020 | 0.973 | 1.001 | 1.000 | 1.029 | 1.164 | 1.325 | 1.221 | 1.105 | 1.086 |
| xml | 1.003 | 1.014 | 1.013 | 1.039 | 1.026 | 1.029 | 1.065 | 1.075 | 1.061 | 1.058 | 1.058 |
| median | 1.006 | 1.025 | 0.997 | 1.011 | 1.015 | 1.026 | 1.071 | 1.101 | 1.075 | 1.080 | 1.074 |

Main time / working-tree time, from the same rounds. The decoder is 7–10%
faster (x-ray level 1 33%), from the decoder changes in the
[changelog](../../CHANGELOG.md)'s Unreleased section: static FSE tables,
the Huffman stream schedule and fixed-size pair loop, and AVX2 block loops
on AVX-512 hosts. Compression is 1–2.6% faster at levels 1 and 5 to 9 and
7% at level 12. Level -1 is unchanged in the median (x-ray 4% slower) and
dfast level 3 too (ooffice, sao and x-ray 2.7–3.6% slower). The `fast`
levels' gain includes the sequence store's kept full-block destinations:
before them, every dense full block zeroed 384 KiB, and level -1 ran 0.997
against main with nci 4.5% slower.

## Main against C

| File | Encode -1 | Encode 1 | Encode 3 | Encode 5 | Encode 7 | Encode 9 | Encode 12 | Decode 1 | Decode 3 | Decode 9 | Decode 19 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| dickens | 0.914 | 0.909 | 1.008 | 0.943 | 0.935 | 0.941 | 1.003 | 0.810 | 0.860 | 0.884 | 0.886 |
| mozilla | 0.938 | 0.923 | 0.888 | 0.943 | 0.938 | 0.929 | 1.070 | 0.886 | 0.911 | 0.927 | 0.934 |
| mr | 0.942 | 0.922 | 0.953 | 0.945 | 0.945 | 0.954 | 0.932 | 0.756 | 0.872 | 0.877 | 0.862 |
| nci | 0.932 | 0.935 | 1.018 | 0.894 | 0.901 | 0.914 | 0.947 | 0.917 | 0.917 | 0.912 | 0.920 |
| ooffice | 0.917 | 0.912 | 0.879 | 0.972 | 0.942 | 0.948 | 0.865 | 0.799 | 0.882 | 0.886 | 0.888 |
| osdb | 0.917 | 0.906 | 0.935 | 0.956 | 0.955 | 0.964 | 0.968 | 0.945 | 0.963 | 0.949 | 0.953 |
| reymont | 0.936 | 0.927 | 1.043 | 0.930 | 0.920 | 0.938 | 1.006 | 0.881 | 0.904 | 0.946 | 0.988 |
| samba | 0.925 | 0.924 | 0.979 | 0.933 | 0.931 | 0.925 | 0.985 | 0.915 | 0.931 | 0.951 | 0.946 |
| sao | 0.896 | 0.892 | 0.879 | 1.037 | 0.996 | 1.021 | 0.856 | 0.886 | 0.856 | 0.876 | 0.844 |
| webster | 0.922 | 0.914 | 1.001 | 0.930 | 0.930 | 0.981 | 1.050 | 0.901 | 0.905 | 0.944 | 0.995 |
| x-ray | 1.080 | 0.957 | 0.858 | 0.992 | 0.963 | 0.963 | 0.914 | 0.640 | 0.753 | 0.869 | 0.877 |
| xml | 0.923 | 0.919 | 0.990 | 0.896 | 0.904 | 0.899 | 0.965 | 0.930 | 0.951 | 0.944 | 0.932 |
| median | 0.924 | 0.921 | 0.966 | 0.943 | 0.936 | 0.945 | 0.967 | 0.886 | 0.904 | 0.919 | 0.926 |

## Method

AMD Ryzen 9 7950X, Linux KVM guest with two dedicated vCPUs: compression
cells on vCPU 0 and decompression cells on vCPU 1 at the same time, so the
two lanes share the last-level cache and memory bandwidth; the three sides
of a cell always run in the same lane. Rust 1.99.1 release builds with
runtime AVX-512 dispatch, zstd 1.5.7 built by `zstd-ffi` (GCC 15.2 `-O3`,
no Huffman assembly). Level 19 and above are not swept for compression:
each process's preflight alone would take minutes per file.

    python3 benchmarks/comparison/silesia.py run --current WARM_CURRENT --main WARM_MAIN \
      --corpus SILESIA --output encode.json --cpu 0 \
      --plan encode-1,encode1,encode3,encode5,encode7,encode9,encode12
    python3 benchmarks/comparison/silesia.py run --current WARM_CURRENT --main WARM_MAIN \
      --corpus SILESIA --output decode.json --cpu 1 --plan decode1,decode3,decode9,decode19

The [results](silesia-2026-10-30/silesia.json.gz) keep every round. See the
[benchmark guide](../benchmarking.md#silesia).
