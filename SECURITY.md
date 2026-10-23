# Security Policy

## Supported versions

mzstd is pre-1.0. Security fixes land on `main` and ship in a new patch release
of the latest minor version; older minor versions are not patched.

| Version | Supported |
| ------- | --------- |
| 0.2.x   | yes       |
| < 0.2   | no        |

## What counts as a vulnerability

mzstd contains no `unsafe` code in the `mzstd` crate, so memory corruption there
should be impossible; a report that shows otherwise is especially welcome.
Please report privately anything that lets untrusted input cause:

- memory unsafety, including in the `mzstd-ffi` C ABI;
- a panic, abort, or infinite loop in the decoder, the framed reader, or any
  other API documented to accept untrusted input;
- memory or CPU use beyond the limits a caller configured (window, input, and
  output limits, dictionary sizes), such as a decompression bomb that escapes
  those limits;
- decoded output that silently differs from the original data without an
  error, for example when a checksum should have been verified.

An encoder output that differs from zstd 1.5.7 but still decodes correctly is a
correctness bug rather than a vulnerability; open a regular issue with the
"Output differs from zstd" template.

Code under `zstd-ffi/vendor/` is upstream zstd and is used only as a test and
benchmark oracle. Report issues in it to the
[zstd project](https://github.com/facebook/zstd/security).

## Reporting a vulnerability

Do not open a public issue, pull request, or discussion.

Report it through GitHub's private vulnerability reporting:
[open a security advisory][advisory]. Include:

- the affected version or commit and the enabled crate features;
- the API you called and its configuration (level, parameters, limits,
  dictionary);
- a minimal input that reproduces the problem (attach it to the advisory) and
  what you observed;
- your assessment of the impact.

## What to expect

- An acknowledgement within 7 days.
- An initial assessment, and a fix plan if the report is confirmed, within
  30 days.
- A coordinated release: the fix is published together with a GitHub security
  advisory and a [RustSec](https://rustsec.org/) advisory where appropriate,
  and you are credited unless you ask not to be.

Please give us a reasonable time to release a fix before disclosing the issue
publicly.

[advisory]: https://github.com/M-Compressors/mzstd/security/advisories/new
