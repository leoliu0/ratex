# TeXres vs TeX Live: benchmark results

Run on 2026-10-07T13:04:16+11:00; 7 timed runs per cell, tools alternating.

## Environment

| | |
| --- | --- |
| CPU | AMD Ryzen Threadripper PRO 5995WX 64-Cores (1 socket, 64 cores, 2 threads/core, 128 logical) |
| RAM | 503.5 GiB |
| OS | Arch Linux, kernel 7.2.8-arch1-2; CPU governor `performance` |
| TeXres | texres 0.7.2 (Rust TeX engine) (sha256 `e88117290f733cee…`) |
| TeX Live | pdfTeX 3.141592653-2.6-1.40.29 (TeX Live 2026/Arch Linux); XeTeX 3.141592653-2.6-0.999998 (TeX Live 2026/Arch Linux); This is LuaHBTeX, Version 1.24.0 (TeX Live 2026/Arch Linux); Latexmk, John Collins, 15 June 2025. Version 4.87; biber version: 2.22 |

## Equivalence of outputs

| Document | Engine | Pages (TeXres / TL) | `pdftotext -layout` text | Status |
| --- | --- | ---: | --- | --- |
| article_math | pdflatex | 15 / 15 | equal | timed |
| article_biblatex | pdflatex | 12 / 12 | equal | timed |
| article_natbib | pdflatex | 12 / 12 | equal | timed |
| beamer_deck | pdflatex | 82 / 82 | equal | timed |
| tikz_pgfplots | pdflatex | 9 / 9 | equal | timed |
| xelatex_fontspec | xelatex | 8 / 8 | equal | timed |
| lualatex_fontspec | lualatex | 7 / 7 | equal | timed |
| long_thesis | pdflatex | 113 / 113 | equal | timed |
| toc_wrap_canary | pdflatex | 1 / 1 | differs on page(s) 1 | **excluded** |

Excluded `toc_wrap_canary`: first text difference, TL `…ent,whilethecomplexframeworkrelatestheaverageregression...............…` vs TeXres `…ent,whilethecomplexframeworkre-latestheaverageregression..............…`

## Summary: median wall time in seconds, TeXres / TeX Live

| Document | Pages | (a) cold build | (b) no-change rebuild | (c) one-line-edit rebuild | (d) single engine pass |
| --- | ---: | ---: | ---: | ---: | ---: |
| article_math | 15 | 0.994 / 1.10 (1.11× faster) | 0.008 / 0.076 (9.97× faster) | 0.339 / 0.407 (1.20× faster) | 0.320 / 0.302 (1.06× slower) |
| article_biblatex | 12 | 4.79 / 5.77 (1.20× faster) | 0.012 / 0.075 (6.18× faster) | 1.34 / 1.38 (1.04× faster) | 1.32 / 1.27 (1.04× slower) |
| article_natbib | 12 | 1.04 / 1.76 (1.70× faster) | 0.008 / 0.075 (9.60× faster) | 0.267 / 0.447 (1.67× faster) | 0.260 / 0.267 (1.03× faster) |
| beamer_deck | 82 | 7.88 / 4.45 (1.77× slower) | 0.010 / 0.080 (7.87× faster) | 4.07 / 2.30 (1.77× slower) | 4.05 / 2.15 (1.89× slower) |
| tikz_pgfplots | 9 | 24.74 / 14.88 (1.66× slower) | 0.009 / 0.079 (8.85× faster) | 8.25 / 5.01 (1.64× slower) | 8.26 / 4.93 (1.68× slower) |
| xelatex_fontspec | 8 | 1.96 / 2.15 (1.10× faster) | 0.007 / 0.073 (9.79× faster) | 0.665 / 0.858 (1.29× faster) | 0.645 / 0.701 (1.09× faster) |
| lualatex_fontspec | 7 | 52.50 / 4.23 (12.40× slower) | 1.71 / 0.083 (20.53× slower) | 1.72 / 1.47 (1.17× slower) | 48.96 / 1.28 (38.10× slower) |
| long_thesis | 113 | 3.04 / 3.57 (1.17× faster) | 0.011 / 0.076 (6.93× faster) | 0.773 / 0.792 (1.03× faster) | 0.760 / 0.596 (1.27× slower) |

## (a) cold build: wall time, seconds, median (min–max)

| Document | Pages | TeXres | TeX Live | TeX Live ÷ TeXres | CPU s: TeXres | CPU s: TeX Live |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| article_math | 15 | 0.994 (0.978–1.02) | 1.10 (1.08–1.14) | 1.11× | 1.04 | 1.09 |
| article_biblatex | 12 | 4.79 (4.74–4.84) | 5.77 (5.68–5.87) | 1.20× | 4.77 | 5.70 |
| article_natbib | 12 | 1.04 (1.04–1.05) | 1.76 (1.74–1.81) | 1.70× | 1.06 | 1.75 |
| beamer_deck | 82 | 7.88 (7.84–7.95) | 4.45 (4.42–4.58) | 0.56× | 7.84 | 4.40 |
| tikz_pgfplots | 9 | 24.74 (24.61–25.10) | 14.88 (14.80–15.07) | 0.60× | 24.54 | 14.74 |
| xelatex_fontspec | 8 | 1.96 (1.95–2.00) | 2.15 (2.10–2.20) | 1.10× | 1.94 | 2.13 |
| lualatex_fontspec | 7 | 52.50 (51.30–53.29) | 4.23 (4.16–4.30) | 0.08× | 52.01 | 4.19 |
| long_thesis | 113 | 3.04 (3.01–3.14) | 3.57 (3.53–3.67) | 1.17× | 3.11 | 3.53 |

## (b) no-change rebuild: wall time, seconds, median (min–max)

| Document | Pages | TeXres | TeX Live | TeX Live ÷ TeXres | CPU s: TeXres | CPU s: TeX Live |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| article_math | 15 | 0.008 (0.007–0.008) | 0.076 (0.073–0.078) | 9.97× | 0.008 | 0.074 |
| article_biblatex | 12 | 0.012 (0.012–0.012) | 0.075 (0.074–0.077) | 6.18× | 0.012 | 0.074 |
| article_natbib | 12 | 0.008 (0.007–0.008) | 0.075 (0.072–0.076) | 9.60× | 0.008 | 0.074 |
| beamer_deck | 82 | 0.010 (0.010–0.011) | 0.080 (0.073–0.086) | 7.87× | 0.011 | 0.079 |
| tikz_pgfplots | 9 | 0.009 (0.009–0.009) | 0.079 (0.074–0.082) | 8.85× | 0.009 | 0.079 |
| xelatex_fontspec | 8 | 0.007 (0.007–0.008) | 0.073 (0.069–0.074) | 9.79× | 0.008 | 0.072 |
| lualatex_fontspec | 7 | 1.71 (1.70–1.72) | 0.083 (0.080–0.085) | 0.05× | 1.69 | 0.082 |
| long_thesis | 113 | 0.011 (0.011–0.011) | 0.076 (0.074–0.078) | 6.93× | 0.011 | 0.075 |

## (c) one-line-edit rebuild: wall time, seconds, median (min–max)

| Document | Pages | TeXres | TeX Live | TeX Live ÷ TeXres | CPU s: TeXres | CPU s: TeX Live |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| article_math | 15 | 0.339 (0.327–0.347) | 0.407 (0.404–0.417) | 1.20× | 0.351 | 0.403 |
| article_biblatex | 12 | 1.34 (1.33–1.37) | 1.38 (1.37–1.41) | 1.04× | 1.33 | 1.37 |
| article_natbib | 12 | 0.267 (0.262–0.277) | 0.447 (0.430–0.459) | 1.67× | 0.272 | 0.443 |
| beamer_deck | 82 | 4.07 (4.02–4.17) | 2.30 (2.28–2.33) | 0.57× | 4.05 | 2.28 |
| tikz_pgfplots | 9 | 8.25 (8.12–8.34) | 5.01 (4.94–5.05) | 0.61× | 8.17 | 4.97 |
| xelatex_fontspec | 8 | 0.665 (0.644–0.687) | 0.858 (0.838–0.873) | 1.29× | 0.659 | 0.850 |
| lualatex_fontspec | 7 | 1.72 (1.70–1.75) | 1.47 (1.45–1.49) | 0.85× | 1.71 | 1.45 |
| long_thesis | 113 | 0.773 (0.765–0.793) | 0.792 (0.770–0.814) | 1.03× | 0.792 | 0.785 |

## (d) single engine pass: wall time, seconds, median (min–max)

| Document | Pages | TeXres | TeX Live | TeX Live ÷ TeXres | CPU s: TeXres | CPU s: TeX Live |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| article_math | 15 | 0.320 (0.315–0.343) | 0.302 (0.296–0.320) | 0.94× | 0.334 | 0.299 |
| article_biblatex | 12 | 1.32 (1.29–1.34) | 1.27 (1.24–1.30) | 0.96× | 1.32 | 1.26 |
| article_natbib | 12 | 0.260 (0.250–0.265) | 0.267 (0.261–0.277) | 1.03× | 0.264 | 0.264 |
| beamer_deck | 82 | 4.05 (4.01–4.10) | 2.15 (2.11–2.18) | 0.53× | 4.03 | 2.13 |
| tikz_pgfplots | 9 | 8.26 (8.15–8.35) | 4.93 (4.86–5.15) | 0.60× | 8.19 | 4.88 |
| xelatex_fontspec | 8 | 0.645 (0.639–0.657) | 0.701 (0.682–0.711) | 1.09× | 0.638 | 0.716 |
| lualatex_fontspec | 7 | 48.96 (47.98–50.20) | 1.28 (1.26–1.29) | 0.03× | 48.51 | 1.27 |
| long_thesis | 113 | 0.760 (0.749–0.770) | 0.596 (0.586–0.611) | 0.78× | 0.779 | 0.590 |

`TeX Live ÷ TeXres` above 1 means TeXres was faster; below 1 means TeXres was slower.

