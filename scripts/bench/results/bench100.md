# TeXres vs TeX Live: 100-document benchmark

Run 2026-10-08T01:44:23+11:00; 5 timed runs per cell (median), tools alternating; jobs=16, CPUs/job=4.

Ratio = TeXres wall time / TeX Live wall time (below 1: TeXres faster).

## Environment

| | |
| --- | --- |
| CPU | AMD Ryzen Threadripper PRO 5995WX 64-Cores (128 logical) |
| RAM / OS | 503.5 GiB; Arch Linux, kernel 7.2.8-arch1-2; governor `performance` |
| TeXres | texres 0.7.2 (Rust TeX engine) (sha256 `f614aba31a45611d…`); 69c9ca4, scripts/build_pgo.sh (PGO, fat LTO, as release.yml ships); xe_natbib_bibtex re-measured alone (same binary, same settings) after TeX Live's xelatex exited 1 once during the parallel sweep |
| TeX Live | pdfTeX 3.141592653-2.6-1.40.29 (TeX Live 2026/Arch Linux); XeTeX 3.141592653-2.6-0.999998 (TeX Live 2026/Arch Linux); This is LuaHBTeX, Version 1.24.0 (TeX Live 2026/Arch Linux); Latexmk, John Collins, 15 June 2025. Version 4.87; biber version: 2.22 |
| Load average | start 5.9, end 5.6 |

## Corpus composition

| Group | Documents |
| --- | ---: |
| arxiv, pdf | 24 |
| generated, lua | 15 |
| generated, pdf | 31 |
| generated, xe | 24 |
| github, pdf | 5 |
| github, xe | 1 |
| **total** | 100 |

## Status

| Status | Documents |
| --- | ---: |
| timed | 100 |

## Summary: documents where TeXres is faster / slower

| Scenario | TeXres faster | TeXres slower | Median ratio | Worst ratio |
| --- | ---: | ---: | ---: | ---: |
| (a) cold build | 100 | 0 | 0.60 | 0.98 (pdf_siunitx_tables) |
| (b) no-change rebuild | 100 | 0 | 0.13 | 0.26 (gh_lkmpg_book) |
| (c) one-line edit rebuild | 100 | 0 | 0.57 | 0.98 (gh_lkmpg_book) |

## All timed documents, slowest first (by worst ratio over scenarios)

| Document | Engine | Pages | (a) TeXres / TL s | ratio | (b) TeXres / TL s | ratio | (c) TeXres / TL s | ratio |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| gh_lkmpg_book | pdf | 193 | 20.13 / 20.95 | 0.96 | 0.030 / 0.114 | 0.26 | 6.62 / 6.77 | 0.98 |
| pdf_siunitx_tables | pdf | 19 | 3.54 / 3.63 | 0.98 | 0.008 / 0.076 | 0.11 | 1.19 / 1.27 | 0.94 |
| lua_beamer | lua | 21 | 3.11 / 3.52 | 0.88 | 0.014 / 0.091 | 0.15 | 1.56 / 1.79 | 0.87 |
| pdf_beamer_metropolis | pdf | 73 | 2.82 / 3.24 | 0.87 | 0.011 / 0.082 | 0.13 | 1.41 / 1.66 | 0.85 |
| lua_siunitx | lua | 5 | 2.84 / 3.37 | 0.84 | 0.011 / 0.083 | 0.13 | 0.956 / 1.17 | 0.82 |
| pdf_pgfplots_gallery | pdf | 19 | 9.40 / 11.63 | 0.81 | 0.010 / 0.077 | 0.13 | 2.87 / 3.45 | 0.83 |
| arxiv_recursive_partitioning | pdf | 48 | 2.00 / 2.43 | 0.82 | 0.014 / 0.083 | 0.17 | 0.477 / 0.832 | 0.57 |
| lua_microtype | lua | 21 | 3.16 / 3.87 | 0.82 | 0.011 / 0.083 | 0.13 | 1.05 / 1.34 | 0.78 |
| xe_tikz_pgfplots | xe | 7 | 3.94 / 4.86 | 0.81 | 0.010 / 0.077 | 0.13 | 1.25 / 1.69 | 0.74 |
| arxiv_ieeetran_kozen | pdf | 17 | 1.47 / 1.83 | 0.80 | 0.013 / 0.086 | 0.15 | 0.525 / 0.931 | 0.56 |
| pdf_tikz_diagrams | pdf | 22 | 5.05 / 6.29 | 0.80 | 0.009 / 0.077 | 0.12 | 1.66 / 2.07 | 0.80 |
| lua_tikz_pgfplots | lua | 6 | 3.97 / 4.97 | 0.80 | 0.013 / 0.092 | 0.14 | 1.33 / 1.69 | 0.78 |
| xe_beamer | xe | 21 | 2.00 / 2.51 | 0.80 | 0.011 / 0.082 | 0.13 | 1.01 / 1.38 | 0.73 |
| pdf_scrbook_large | pdf | 255 | 4.10 / 5.30 | 0.77 | 0.013 / 0.078 | 0.16 | 1.38 / 1.78 | 0.77 |
| pdf_beamer_boadilla | pdf | 13 | 2.70 / 3.50 | 0.77 | 0.012 / 0.084 | 0.14 | 1.36 / 1.77 | 0.77 |
| lua_polyglossia_french | lua | 3 | 2.08 / 2.71 | 0.77 | 0.011 / 0.084 | 0.13 | 0.700 / 0.954 | 0.73 |
| gh_mtheme_demo | pdf | 33 | 4.40 / 5.76 | 0.76 | 0.013 / 0.088 | 0.15 | 1.49 / 1.97 | 0.76 |
| xe_siunitx | xe | 5 | 1.63 / 2.14 | 0.76 | 0.008 / 0.074 | 0.11 | 0.545 / 0.852 | 0.64 |
| arxiv_lipics_paper | pdf | 30 | 4.37 / 5.73 | 0.76 | 0.017 / 0.093 | 0.18 | 1.49 / 2.00 | 0.75 |
| lua_book | lua | 40 | 2.02 / 2.67 | 0.76 | 0.011 / 0.085 | 0.13 | 1.02 / 1.37 | 0.74 |
| xe_xecjk_chinese | xe | 20 | 1.47 / 1.97 | 0.75 | 0.009 / 0.075 | 0.11 | 0.494 / 0.805 | 0.61 |
| xe_ctex_article | xe | 24 | 1.72 / 2.31 | 0.75 | 0.008 / 0.077 | 0.11 | 0.580 / 0.929 | 0.62 |
| lua_fontspec_termes | lua | 12 | 3.45 / 4.65 | 0.74 | 0.011 / 0.085 | 0.13 | 1.16 / 1.60 | 0.72 |
| lua_directlua | lua | 3 | 1.76 / 2.37 | 0.74 | 0.010 / 0.082 | 0.12 | 0.595 / 0.842 | 0.71 |
| lua_luacode_env | lua | 3 | 1.77 / 2.40 | 0.74 | 0.010 / 0.083 | 0.13 | 0.602 / 0.848 | 0.71 |
| arxiv_ecta_paper | pdf | 86 | 10.51 / 14.42 | 0.73 | 0.014 / 0.083 | 0.17 | 3.51 / 4.79 | 0.73 |
| lua_fontspec_pagella | lua | 4 | 3.29 / 4.50 | 0.73 | 0.011 / 0.085 | 0.13 | 1.12 / 1.56 | 0.72 |
| arxiv_jhep_6d_scft | pdf | 28 | 3.40 / 4.67 | 0.73 | 0.014 / 0.089 | 0.16 | 1.16 / 1.63 | 0.71 |
| pdf_listings_algorithm2e | pdf | 31 | 1.19 / 1.66 | 0.72 | 0.008 / 0.076 | 0.11 | 0.403 / 0.595 | 0.68 |
| pdf_memoir_book | pdf | 118 | 2.03 / 2.84 | 0.72 | 0.010 / 0.077 | 0.13 | 0.691 / 0.984 | 0.70 |
| lua_biblatex | lua | 5 | 4.44 / 7.46 | 0.60 | 0.015 / 0.086 | 0.17 | 1.13 / 1.58 | 0.71 |
| lua_polyglossia_russian | lua | 3 | 2.12 / 3.00 | 0.71 | 0.010 / 0.082 | 0.12 | 0.709 / 1.04 | 0.68 |
| lua_scrartcl_unicode | lua | 7 | 3.90 / 5.51 | 0.71 | 0.011 / 0.084 | 0.13 | 1.31 / 1.88 | 0.70 |
| lua_lm_fontspec | lua | 5 | 1.67 / 2.38 | 0.70 | 0.010 / 0.081 | 0.12 | 0.566 / 0.839 | 0.68 |
| xe_ctex_book | xe | 27 | 1.05 / 1.50 | 0.70 | 0.009 / 0.076 | 0.11 | 0.531 / 0.872 | 0.61 |
| arxiv_lmcs_eating | pdf | 30 | 2.03 / 2.94 | 0.69 | 0.014 / 0.087 | 0.16 | 1.01 / 1.47 | 0.69 |
| pdf_hyperref_toc | pdf | 68 | 1.40 / 2.06 | 0.68 | 0.009 / 0.077 | 0.12 | 0.481 / 0.734 | 0.65 |
| xe_polyglossia_german | xe | 20 | 1.30 / 1.92 | 0.68 | 0.009 / 0.077 | 0.11 | 0.435 / 0.800 | 0.54 |
| lua_polyglossia_german | lua | 3 | 1.95 / 2.94 | 0.66 | 0.011 / 0.083 | 0.13 | 0.659 / 1.02 | 0.65 |
| xe_polyglossia_polish | xe | 4 | 1.19 / 1.86 | 0.64 | 0.008 / 0.074 | 0.10 | 0.401 / 0.746 | 0.54 |
| arxiv_stat_me_39pp | pdf | 39 | 2.76 / 4.36 | 0.63 | 0.014 / 0.086 | 0.16 | 0.934 / 1.46 | 0.64 |
| arxiv_graded_xy | pdf | 22 | 1.41 / 2.70 | 0.52 | 0.012 / 0.084 | 0.14 | 0.477 / 0.749 | 0.64 |
| xe_float_heavy | xe | 22 | 1.09 / 1.72 | 0.63 | 0.008 / 0.075 | 0.11 | 0.370 / 0.730 | 0.51 |
| pdf_scrartcl_floats | pdf | 50 | 1.70 / 2.71 | 0.63 | 0.008 / 0.076 | 0.11 | 0.576 / 0.941 | 0.61 |
| arxiv_ehmp_econ | pdf | 54 | 2.24 / 3.56 | 0.63 | 0.015 / 0.088 | 0.17 | 0.765 / 1.23 | 0.62 |
| arxiv_vae | pdf | 14 | 1.17 / 1.87 | 0.63 | 0.023 / 0.090 | 0.25 | 0.614 / 0.979 | 0.63 |
| gh_mtheme_demo_xelatex | xe | 33 | 4.79 / 7.65 | 0.63 | 0.014 / 0.087 | 0.16 | 1.64 / 2.75 | 0.60 |
| arxiv_math_nt_amsart | pdf | 32 | 1.32 / 3.10 | 0.42 | 0.010 / 0.079 | 0.13 | 0.435 / 0.698 | 0.62 |
| xe_scrbook_fontspec | xe | 36 | 1.89 / 3.05 | 0.62 | 0.009 / 0.076 | 0.12 | 0.619 / 1.14 | 0.54 |
| arxiv_bert | pdf | 16 | 1.86 / 3.03 | 0.61 | 0.015 / 0.087 | 0.17 | 0.635 / 1.06 | 0.60 |
| pdf_cleveref_hyperref | pdf | 25 | 0.943 / 1.56 | 0.61 | 0.009 / 0.078 | 0.12 | 0.322 / 0.562 | 0.57 |
| pdf_float_heavy | pdf | 41 | 0.759 / 1.27 | 0.60 | 0.008 / 0.075 | 0.11 | 0.258 / 0.466 | 0.55 |
| arxiv_sparse_cb_tikz | pdf | 13 | 1.15 / 1.93 | 0.60 | 0.011 / 0.082 | 0.14 | 0.396 / 0.692 | 0.57 |
| xe_book_memoir | xe | 25 | 0.821 / 1.38 | 0.60 | 0.009 / 0.078 | 0.12 | 0.416 / 0.811 | 0.51 |
| pdf_tcolorbox | pdf | 13 | 0.963 / 1.62 | 0.60 | 0.010 / 0.081 | 0.12 | 0.331 / 0.594 | 0.56 |
| xe_fontspec_schola | xe | 19 | 1.50 / 2.55 | 0.59 | 0.008 / 0.076 | 0.11 | 0.506 / 1.000 | 0.51 |
| xe_polyglossia_russian | xe | 4 | 0.880 / 1.51 | 0.58 | 0.008 / 0.075 | 0.10 | 0.296 / 0.651 | 0.45 |
| xe_biblatex | xe | 6 | 2.99 / 5.39 | 0.55 | 0.012 / 0.077 | 0.16 | 0.794 / 1.37 | 0.58 |
| pdf_biblatex_numeric | pdf | 15 | 3.05 / 6.91 | 0.44 | 0.013 / 0.078 | 0.16 | 0.815 / 1.42 | 0.57 |
| xe_polyglossia_multilingual | xe | 3 | 0.854 / 1.49 | 0.57 | 0.008 / 0.076 | 0.11 | 0.293 / 0.637 | 0.46 |
| arxiv_sex_chr_bio | pdf | 20 | 0.643 / 1.12 | 0.57 | 0.014 / 0.080 | 0.18 | 0.334 / 0.589 | 0.57 |
| pdf_biblatex_ieee | pdf | 9 | 2.88 / 5.79 | 0.50 | 0.013 / 0.078 | 0.16 | 0.800 / 1.40 | 0.57 |
| gh_tufte_book | pdf | 42 | 1.85 / 3.26 | 0.57 | 0.012 / 0.082 | 0.15 | 0.638 / 1.12 | 0.57 |
| xe_fontspec_pagella | xe | 8 | 1.39 / 2.44 | 0.57 | 0.008 / 0.075 | 0.11 | 0.469 / 0.965 | 0.49 |
| pdf_footnote_heavy | pdf | 38 | 0.622 / 1.09 | 0.57 | 0.008 / 0.075 | 0.11 | 0.213 / 0.411 | 0.52 |
| xe_polyglossia_french | xe | 4 | 0.839 / 1.48 | 0.57 | 0.008 / 0.076 | 0.10 | 0.289 / 0.644 | 0.45 |
| xe_polyglossia_greek | xe | 10 | 0.831 / 1.47 | 0.57 | 0.008 / 0.076 | 0.11 | 0.283 / 0.637 | 0.44 |
| xe_polyglossia_spanish | xe | 4 | 0.830 / 1.47 | 0.56 | 0.008 / 0.075 | 0.11 | 0.276 / 0.626 | 0.44 |
| xe_fontspec_bonum | xe | 5 | 1.32 / 2.35 | 0.56 | 0.008 / 0.075 | 0.11 | 0.449 / 0.941 | 0.48 |
| arxiv_resnet | pdf | 12 | 1.15 / 2.06 | 0.56 | 0.012 / 0.082 | 0.15 | 0.396 / 0.731 | 0.54 |
| xe_fontspec_termes | xe | 4 | 1.33 / 2.38 | 0.56 | 0.008 / 0.076 | 0.11 | 0.448 / 0.949 | 0.47 |
| pdf_biblatex_chicago | pdf | 13 | 3.19 / 7.47 | 0.43 | 0.015 / 0.076 | 0.19 | 0.871 / 1.58 | 0.55 |
| pdf_glossaries | pdf | 4 | 0.724 / 1.31 | 0.55 | 0.008 / 0.076 | 0.10 | 0.253 / 0.490 | 0.52 |
| xe_report_tables | xe | 26 | 0.645 / 1.17 | 0.55 | 0.008 / 0.074 | 0.11 | 0.325 / 0.685 | 0.48 |
| pdf_microtype | pdf | 22 | 0.778 / 1.43 | 0.54 | 0.008 / 0.076 | 0.11 | 0.265 / 0.516 | 0.51 |
| pdf_amsart | pdf | 40 | 0.637 / 1.18 | 0.54 | 0.008 / 0.075 | 0.11 | 0.215 / 0.420 | 0.51 |
| arxiv_jcap_cmb | pdf | 14 | 0.772 / 2.08 | 0.37 | 0.010 / 0.083 | 0.13 | 0.291 / 0.540 | 0.54 |
| arxiv_revtex41_neuro | pdf | 17 | 0.836 / 1.57 | 0.53 | 0.015 / 0.083 | 0.18 | 0.429 / 0.802 | 0.53 |
| arxiv_dml_econometrics_journal | pdf | 71 | 1.16 / 3.09 | 0.37 | 0.011 / 0.079 | 0.14 | 0.381 / 0.715 | 0.53 |
| gh_phd_thesis_template | pdf | 39 | 2.21 / 4.21 | 0.53 | 0.016 / 0.084 | 0.19 | 0.725 / 1.43 | 0.51 |
| pdf_standalone_tikz | pdf | 1 | 0.275 / 0.541 | 0.51 | 0.009 / 0.079 | 0.12 | 0.275 / 0.527 | 0.52 |
| pdf_biblatex_authortitle | pdf | 5 | 1.58 / 3.64 | 0.43 | 0.011 / 0.076 | 0.15 | 0.422 / 0.811 | 0.52 |
| arxiv_econ_gn_bibtex | pdf | 34 | 1.31 / 5.19 | 0.25 | 0.020 / 0.084 | 0.24 | 0.431 / 0.833 | 0.52 |
| arxiv_svjour3_paper | pdf | 38 | 1.01 / 1.99 | 0.51 | 0.010 / 0.080 | 0.13 | 0.347 / 0.705 | 0.49 |
| xe_lm_fontspec | xe | 4 | 0.641 / 1.27 | 0.50 | 0.008 / 0.075 | 0.10 | 0.216 / 0.581 | 0.37 |
| gh_tufte_handout | pdf | 5 | 1.18 / 2.39 | 0.49 | 0.011 / 0.082 | 0.13 | 0.403 / 0.819 | 0.49 |
| pdf_report_longtable | pdf | 30 | 0.344 / 0.713 | 0.48 | 0.008 / 0.074 | 0.10 | 0.176 / 0.382 | 0.46 |
| arxiv_perelman_ricci_flow | pdf | 39 | 0.368 / 0.779 | 0.47 | 0.009 / 0.077 | 0.11 | 0.230 / 0.689 | 0.33 |
| pdf_makeindex | pdf | 30 | 0.443 / 1.00 | 0.44 | 0.007 / 0.074 | 0.10 | 0.152 / 0.368 | 0.41 |
| xe_natbib_bibtex | xe | 9 | 0.958 / 2.21 | 0.43 | 0.008 / 0.072 | 0.11 | 0.243 / 0.680 | 0.36 |
| arxiv_divisor2squares | pdf | 14 | 0.385 / 0.968 | 0.40 | 0.032 / 0.231 | 0.14 | 0.300 / 0.735 | 0.41 |
| arxiv_attention | pdf | 15 | 1.10 / 3.01 | 0.36 | 0.016 / 0.083 | 0.19 | 0.429 / 1.07 | 0.40 |
| pdf_revtex4_2 | pdf | 7 | 0.384 / 1.01 | 0.38 | 0.009 / 0.076 | 0.11 | 0.196 / 0.509 | 0.38 |
| pdf_elsarticle | pdf | 26 | 0.746 / 2.09 | 0.36 | 0.009 / 0.077 | 0.12 | 0.193 / 0.524 | 0.37 |
| pdf_natbib_numbers | pdf | 24 | 0.664 / 1.98 | 0.34 | 0.009 / 0.076 | 0.11 | 0.173 / 0.480 | 0.36 |
| pdf_ieeetran_conf | pdf | 6 | 0.490 / 1.48 | 0.33 | 0.009 / 0.078 | 0.11 | 0.167 / 0.490 | 0.34 |
| pdf_llncs | pdf | 8 | 0.545 / 1.75 | 0.31 | 0.008 / 0.076 | 0.11 | 0.141 / 0.450 | 0.31 |
| pdf_imakeidx | pdf | 15 | 0.337 / 1.08 | 0.31 | 0.008 / 0.075 | 0.10 | 0.116 / 0.373 | 0.31 |
| pdf_letter | pdf | 1 | 0.140 / 0.486 | 0.29 | 0.007 / 0.074 | 0.10 | 0.073 / 0.276 | 0.26 |
| arxiv_revtex42_unruh | pdf | 33 | 0.635 / 2.31 | 0.27 | 0.013 / 0.081 | 0.17 | 0.327 / 1.15 | 0.28 |

## Documents where TeXres is slower in at least one scenario

None.

## Documents that failed, differ, or could not be timed

None.

