# TeXres vs TeX Live: 100-document benchmark

Run 2026-10-08T01:51:57+11:00; 9 timed runs per cell (median), tools alternating; jobs=1, CPUs/job=None.

Ratio = TeXres wall time / TeX Live wall time (below 1: TeXres faster).

## Environment

| | |
| --- | --- |
| CPU | AMD Ryzen Threadripper PRO 5995WX 64-Cores (128 logical) |
| RAM / OS | 503.5 GiB; Arch Linux, kernel 7.2.8-arch1-2; governor `performance` |
| TeXres | texres 0.7.2 (Rust TeX engine) (sha256 `f614aba31a45611d…`) |
| TeX Live | pdfTeX 3.141592653-2.6-1.40.29 (TeX Live 2026/Arch Linux); XeTeX 3.141592653-2.6-0.999998 (TeX Live 2026/Arch Linux); This is LuaHBTeX, Version 1.24.0 (TeX Live 2026/Arch Linux); Latexmk, John Collins, 15 June 2025. Version 4.87; biber version: 2.22 |
| Load average | start 1.8, end 1.4 |

## Corpus composition

| Group | Documents |
| --- | ---: |
| generated, lua | 1 |
| generated, pdf | 2 |
| github, pdf | 1 |
| **total** | 4 |

## Status

| Status | Documents |
| --- | ---: |
| timed | 4 |

## Summary: documents where TeXres is faster / slower

| Scenario | TeXres faster | TeXres slower | Median ratio | Worst ratio |
| --- | ---: | ---: | ---: | ---: |
| (a) cold build | 4 | 0 | 0.93 | 0.99 (pdf_siunitx_tables) |
| (b) no-change rebuild | 4 | 0 | 0.13 | 0.24 (gh_lkmpg_book) |
| (c) one-line edit rebuild | 4 | 0 | 0.92 | 0.96 (gh_lkmpg_book) |

## All timed documents, slowest first (by worst ratio over scenarios)

| Document | Engine | Pages | (a) TeXres / TL s | ratio | (b) TeXres / TL s | ratio | (c) TeXres / TL s | ratio |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| pdf_siunitx_tables | pdf | 19 | 3.21 / 3.23 | 0.99 | 0.007 / 0.073 | 0.10 | 1.07 / 1.12 | 0.95 |
| gh_lkmpg_book | pdf | 193 | 18.06 / 18.80 | 0.96 | 0.027 / 0.111 | 0.24 | 6.58 / 6.86 | 0.96 |
| lua_beamer | lua | 21 | 2.79 / 3.08 | 0.91 | 0.011 / 0.086 | 0.13 | 1.41 / 1.59 | 0.89 |
| pdf_beamer_metropolis | pdf | 73 | 2.57 / 2.90 | 0.89 | 0.009 / 0.080 | 0.12 | 1.29 / 1.47 | 0.88 |

## Documents where TeXres is slower in at least one scenario

None.

## Documents that failed, differ, or could not be timed

None.

