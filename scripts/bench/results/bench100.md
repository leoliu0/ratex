# TeXres vs TeX Live: 100-document benchmark

Run 2026-10-07T14:53:47+11:00; 5 timed runs per cell (median), tools alternating; jobs=16, CPUs/job=4.

Ratio = TeXres wall time / TeX Live wall time (below 1: TeXres faster).

## Environment

| | |
| --- | --- |
| CPU | AMD Ryzen Threadripper PRO 5995WX 64-Cores (128 logical) |
| RAM / OS | 503.5 GiB; Arch Linux, kernel 7.2.8-arch1-2; governor `performance` |
| TeXres | texres 0.7.2 (Rust TeX engine) (sha256 `e88117290f733cee…`); baseline built from 4bae77a |
| TeX Live | pdfTeX 3.141592653-2.6-1.40.29 (TeX Live 2026/Arch Linux); XeTeX 3.141592653-2.6-0.999998 (TeX Live 2026/Arch Linux); This is LuaHBTeX, Version 1.24.0 (TeX Live 2026/Arch Linux); Latexmk, John Collins, 15 June 2025. Version 4.87; biber version: 2.22 |
| Load average | start 20.6, end 33.6 |

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
| differs | 17 |
| texres_failed | 7 |
| timed | 76 |

## Summary: documents where TeXres is faster / slower

| Scenario | TeXres faster | TeXres slower | Median ratio | Worst ratio |
| --- | ---: | ---: | ---: | ---: |
| (a) cold build | 43 | 33 | 0.93 | 23.96 (lua_directlua) |
| (b) no-change rebuild | 63 | 13 | 0.12 | 33.16 (gh_mtheme_demo_xelatex) |
| (c) one-line edit rebuild | 49 | 27 | 0.87 | 1.89 (lua_microtype) |

## All timed documents, slowest first (by worst ratio over scenarios)

| Document | Engine | Pages | (a) TeXres / TL s | ratio | (b) TeXres / TL s | ratio | (c) TeXres / TL s | ratio |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| gh_mtheme_demo_xelatex | xe | 33 | 8.82 / 7.75 | 1.14 | 2.88 / 0.087 | 33.16 | 2.87 / 2.73 | 1.05 |
| lua_beamer | lua | 21 | 61.04 / 3.66 | 16.68 | 3.09 / 0.097 | 31.75 | 2.87 / 1.91 | 1.50 |
| gh_mtheme_demo | pdf | 33 | 8.26 / 6.16 | 1.34 | 2.62 / 0.087 | 30.01 | 2.64 / 1.96 | 1.35 |
| lua_fontspec_pagella | lua | 4 | 62.48 / 5.69 | 10.99 | 2.48 / 0.092 | 26.98 | 1.90 / 1.59 | 1.19 |
| lua_tikz_pgfplots | lua | 6 | 66.40 / 4.93 | 13.48 | 2.38 / 0.088 | 26.89 | 2.35 / 1.67 | 1.41 |
| lua_scrartcl_unicode | lua | 7 | 62.99 / 5.55 | 11.34 | 2.20 / 0.085 | 25.86 | 2.27 / 1.91 | 1.19 |
| lua_microtype | lua | 21 | 60.87 / 4.61 | 13.20 | 2.07 / 0.083 | 24.89 | 3.19 / 1.69 | 1.89 |
| lua_directlua | lua | 3 | 57.91 / 2.42 | 23.96 | 1.15 / 0.087 | 13.22 | 1.64 / 1.22 | 1.35 |
| lua_fontspec_termes | lua | 12 | 63.54 / 6.43 | 9.88 | 1.99 / 0.085 | 23.55 | 2.86 / 1.80 | 1.59 |
| lua_lm_fontspec | lua | 5 | 58.00 / 2.50 | 23.24 | 1.59 / 0.116 | 13.66 | 1.57 / 1.19 | 1.32 |
| lua_siunitx | lua | 5 | 60.31 / 3.41 | 17.67 | 2.74 / 0.128 | 21.41 | 1.66 / 1.21 | 1.38 |
| lua_polyglossia_french | lua | 3 | 59.72 / 2.91 | 20.52 | 1.87 / 0.116 | 16.09 | 1.96 / 1.44 | 1.36 |
| lua_polyglossia_german | lua | 3 | 62.91 / 3.17 | 19.83 | 1.81 / 0.160 | 11.35 | 1.21 / 1.05 | 1.14 |
| pdf_pgfplots_gallery | pdf | 19 | 21.34 / 11.87 | 1.80 | 0.014 / 0.122 | 0.11 | 6.22 / 3.89 | 1.60 |
| pdf_standalone_tikz | pdf | 1 | 1.32 / 0.829 | 1.59 | 0.015 / 0.153 | 0.10 | 0.478 / 0.805 | 0.59 |
| pdf_beamer_metropolis | pdf | 73 | 5.25 / 3.30 | 1.59 | 0.011 / 0.084 | 0.14 | 2.60 / 1.71 | 1.52 |
| pdf_siunitx_tables | pdf | 19 | 5.66 / 3.67 | 1.54 | 0.009 / 0.076 | 0.11 | 1.85 / 1.27 | 1.46 |
| pdf_tikz_diagrams | pdf | 22 | 9.53 / 6.20 | 1.54 | 0.010 / 0.083 | 0.12 | 3.45 / 3.33 | 1.03 |
| xe_tikz_pgfplots | xe | 7 | 7.54 / 5.02 | 1.50 | 0.010 / 0.078 | 0.13 | 2.54 / 1.87 | 1.36 |
| xe_beamer | xe | 21 | 3.49 / 2.52 | 1.38 | 0.011 / 0.081 | 0.14 | 1.73 / 1.38 | 1.26 |
| xe_ctex_article | xe | 24 | 4.65 / 3.42 | 1.36 | 0.011 / 0.114 | 0.10 | 1.16 / 1.01 | 1.14 |
| pdf_beamer_boadilla | pdf | 13 | 4.94 / 3.66 | 1.35 | 0.012 / 0.087 | 0.14 | 2.48 / 1.86 | 1.33 |
| pdf_scrbook_large | pdf | 255 | 6.48 / 5.28 | 1.23 | 0.012 / 0.079 | 0.16 | 2.16 / 1.78 | 1.21 |
| xe_xecjk_chinese | xe | 20 | 2.49 / 2.05 | 1.22 | 0.009 / 0.077 | 0.11 | 0.844 / 0.850 | 0.99 |
| pdf_listings_algorithm2e | pdf | 31 | 2.05 / 1.69 | 1.21 | 0.009 / 0.077 | 0.11 | 0.690 / 0.611 | 1.13 |
| xe_ctex_book | xe | 27 | 2.74 / 2.29 | 1.19 | 0.011 / 0.112 | 0.10 | 0.893 / 0.953 | 0.94 |
| xe_siunitx | xe | 5 | 2.62 / 2.20 | 1.19 | 0.008 / 0.076 | 0.11 | 0.845 / 0.867 | 0.97 |
| xe_polyglossia_german | xe | 20 | 3.28 / 2.80 | 1.17 | 0.011 / 0.116 | 0.10 | 1.14 / 1.08 | 1.05 |
| xe_fontspec_termes | xe | 4 | 3.01 / 2.67 | 1.12 | 0.011 / 0.113 | 0.10 | 1.28 / 1.72 | 0.74 |
| pdf_memoir_book | pdf | 118 | 3.38 / 3.02 | 1.12 | 0.012 / 0.109 | 0.11 | 1.53 / 1.39 | 1.10 |
| arxiv_math_nt_amsart | pdf | 32 | 2.41 / 2.27 | 1.06 | 0.011 / 0.087 | 0.13 | 0.701 / 0.753 | 0.93 |
| pdf_hyperref_toc | pdf | 68 | 2.20 / 2.09 | 1.05 | 0.010 / 0.078 | 0.13 | 0.747 / 0.742 | 1.01 |
| xe_fontspec_schola | xe | 19 | 3.40 / 3.37 | 1.01 | 0.012 / 0.119 | 0.10 | 0.950 / 1.59 | 0.60 |
| xe_biblatex | xe | 6 | 5.28 / 7.22 | 0.73 | 0.018 / 0.135 | 0.13 | 1.43 / 1.42 | 1.01 |
| xe_polyglossia_multilingual | xe | 3 | 1.96 / 1.99 | 0.98 | 0.012 / 0.143 | 0.08 | 0.897 / 1.21 | 0.74 |
| arxiv_ehmp_econ | pdf | 54 | 3.81 / 3.98 | 0.96 | 0.015 / 0.087 | 0.17 | 1.21 / 1.27 | 0.95 |
| xe_book_memoir | xe | 25 | 1.33 / 1.39 | 0.96 | 0.009 / 0.078 | 0.12 | 0.719 / 0.893 | 0.81 |
| pdf_scrartcl_floats | pdf | 50 | 2.83 / 2.99 | 0.95 | 0.012 / 0.148 | 0.08 | 0.892 / 0.982 | 0.91 |
| arxiv_vae | pdf | 14 | 1.92 / 2.05 | 0.94 | 0.022 / 0.085 | 0.26 | 0.919 / 0.970 | 0.95 |
| arxiv_dml_econometrics_journal | pdf | 71 | 2.19 / 2.36 | 0.92 | 0.012 / 0.087 | 0.14 | 0.587 / 0.773 | 0.76 |
| xe_report_tables | xe | 26 | 1.65 / 1.78 | 0.92 | 0.011 / 0.115 | 0.09 | 0.571 / 0.743 | 0.77 |
| pdf_biblatex_chicago | pdf | 13 | 5.22 / 7.56 | 0.69 | 0.016 / 0.077 | 0.21 | 1.44 / 1.58 | 0.91 |
| pdf_biblatex_numeric | pdf | 15 | 4.82 / 6.96 | 0.69 | 0.014 / 0.079 | 0.18 | 1.30 / 1.43 | 0.91 |
| pdf_biblatex_ieee | pdf | 9 | 4.58 / 5.80 | 0.79 | 0.014 / 0.079 | 0.17 | 1.27 / 1.41 | 0.90 |
| pdf_tcolorbox | pdf | 13 | 1.49 / 1.65 | 0.90 | 0.010 / 0.081 | 0.12 | 0.499 / 0.597 | 0.83 |
| arxiv_stat_me_39pp | pdf | 39 | 4.36 / 4.86 | 0.90 | 0.015 / 0.095 | 0.16 | 1.63 / 1.81 | 0.90 |
| xe_polyglossia_spanish | xe | 4 | 2.03 / 2.28 | 0.89 | 0.011 / 0.116 | 0.10 | 0.451 / 0.625 | 0.72 |
| arxiv_sparse_cb_tikz | pdf | 13 | 1.92 / 2.17 | 0.89 | 0.012 / 0.089 | 0.14 | 0.599 / 0.708 | 0.85 |
| pdf_cleveref_hyperref | pdf | 25 | 1.42 / 1.60 | 0.88 | 0.010 / 0.079 | 0.12 | 0.481 / 0.581 | 0.83 |
| pdf_footnote_heavy | pdf | 38 | 0.927 / 1.09 | 0.85 | 0.008 / 0.076 | 0.11 | 0.315 / 0.411 | 0.77 |
| xe_fontspec_bonum | xe | 5 | 2.10 / 2.49 | 0.84 | 0.009 / 0.080 | 0.11 | 0.869 / 1.58 | 0.55 |
| pdf_glossaries | pdf | 4 | 1.11 / 1.35 | 0.82 | 0.008 / 0.077 | 0.11 | 0.387 / 0.512 | 0.76 |
| arxiv_jcap_cmb | pdf | 14 | 1.42 / 1.74 | 0.82 | 0.010 / 0.084 | 0.12 | 0.409 / 0.575 | 0.71 |
| arxiv_sex_chr_bio | pdf | 20 | 1.02 / 1.25 | 0.81 | 0.015 / 0.084 | 0.18 | 0.514 / 0.646 | 0.80 |
| xe_float_heavy | xe | 22 | 2.07 / 2.57 | 0.81 | 0.009 / 0.076 | 0.12 | 0.550 / 0.743 | 0.74 |
| pdf_float_heavy | pdf | 41 | 1.04 / 1.29 | 0.80 | 0.009 / 0.077 | 0.11 | 0.349 / 0.474 | 0.74 |
| xe_fontspec_pagella | xe | 8 | 2.24 / 2.79 | 0.80 | 0.010 / 0.111 | 0.09 | 1.16 / 1.51 | 0.77 |
| arxiv_resnet | pdf | 12 | 1.84 / 2.30 | 0.80 | 0.013 / 0.088 | 0.14 | 0.619 / 0.813 | 0.76 |
| pdf_biblatex_authortitle | pdf | 5 | 2.46 / 3.76 | 0.65 | 0.013 / 0.077 | 0.17 | 0.652 / 0.817 | 0.80 |
| arxiv_perelman_ricci_flow | pdf | 39 | 0.603 / 1.08 | 0.56 | 0.010 / 0.089 | 0.11 | 0.424 / 0.533 | 0.80 |
| pdf_microtype | pdf | 22 | 1.10 / 1.45 | 0.76 | 0.009 / 0.077 | 0.12 | 0.370 / 0.522 | 0.71 |
| xe_polyglossia_french | xe | 4 | 1.37 / 2.15 | 0.64 | 0.010 / 0.110 | 0.09 | 0.679 / 0.922 | 0.74 |
| pdf_amsart | pdf | 40 | 0.888 / 1.21 | 0.73 | 0.008 / 0.077 | 0.11 | 0.301 / 0.441 | 0.68 |
| pdf_report_longtable | pdf | 30 | 0.717 / 0.981 | 0.73 | 0.009 / 0.107 | 0.09 | 0.359 / 0.532 | 0.67 |
| xe_natbib_bibtex | xe | 9 | 2.33 / 3.22 | 0.72 | 0.011 / 0.119 | 0.09 | 0.655 / 1.07 | 0.61 |
| arxiv_svjour3_paper | pdf | 38 | 1.62 / 2.29 | 0.71 | 0.011 / 0.086 | 0.13 | 0.514 / 0.728 | 0.71 |
| arxiv_attention | pdf | 15 | 1.73 / 2.49 | 0.70 | 0.017 / 0.087 | 0.19 | 0.659 / 1.14 | 0.58 |
| xe_lm_fontspec | xe | 4 | 1.12 / 1.65 | 0.68 | 0.009 / 0.111 | 0.09 | 0.506 / 0.822 | 0.62 |
| pdf_revtex4_2 | pdf | 7 | 0.764 / 1.41 | 0.54 | 0.010 / 0.109 | 0.09 | 0.384 / 0.715 | 0.54 |
| arxiv_divisor2squares | pdf | 14 | 0.676 / 1.32 | 0.51 | 0.011 / 0.100 | 0.11 | 0.365 / 0.746 | 0.49 |
| pdf_elsarticle | pdf | 26 | 1.03 / 2.15 | 0.48 | 0.009 / 0.078 | 0.12 | 0.263 / 0.526 | 0.50 |
| pdf_natbib_numbers | pdf | 24 | 0.932 / 2.09 | 0.45 | 0.010 / 0.108 | 0.10 | 0.253 / 0.538 | 0.47 |
| pdf_ieeetran_conf | pdf | 6 | 0.660 / 1.51 | 0.44 | 0.009 / 0.078 | 0.11 | 0.222 / 0.500 | 0.44 |
| pdf_llncs | pdf | 8 | 0.724 / 1.84 | 0.39 | 0.009 / 0.077 | 0.11 | 0.186 / 0.466 | 0.40 |
| arxiv_revtex42_unruh | pdf | 33 | 0.948 / 2.61 | 0.36 | 0.014 / 0.084 | 0.17 | 0.486 / 1.23 | 0.40 |
| pdf_letter | pdf | 1 | 0.163 / 0.497 | 0.33 | 0.007 / 0.075 | 0.10 | 0.084 / 0.281 | 0.30 |

## Documents where TeXres is slower in at least one scenario

| Document | (a) | (b) | (c) |
| --- | ---: | ---: | ---: |
| gh_mtheme_demo_xelatex | 1.14 | 33.16 | 1.05 |
| lua_beamer | 16.68 | 31.75 | 1.50 |
| gh_mtheme_demo | 1.34 | 30.01 | 1.35 |
| lua_fontspec_pagella | 10.99 | 26.98 | 1.19 |
| lua_tikz_pgfplots | 13.48 | 26.89 | 1.41 |
| lua_scrartcl_unicode | 11.34 | 25.86 | 1.19 |
| lua_microtype | 13.20 | 24.89 | 1.89 |
| lua_directlua | 23.96 | 13.22 | 1.35 |
| lua_fontspec_termes | 9.88 | 23.55 | 1.59 |
| lua_lm_fontspec | 23.24 | 13.66 | 1.32 |
| lua_siunitx | 17.67 | 21.41 | 1.38 |
| lua_polyglossia_french | 20.52 | 16.09 | 1.36 |
| lua_polyglossia_german | 19.83 | 11.35 | 1.14 |
| pdf_pgfplots_gallery | 1.80 | 0.11 | 1.60 |
| pdf_standalone_tikz | 1.59 | 0.10 | 0.59 |
| pdf_beamer_metropolis | 1.59 | 0.14 | 1.52 |
| pdf_siunitx_tables | 1.54 | 0.11 | 1.46 |
| pdf_tikz_diagrams | 1.54 | 0.12 | 1.03 |
| xe_tikz_pgfplots | 1.50 | 0.13 | 1.36 |
| xe_beamer | 1.38 | 0.14 | 1.26 |
| xe_ctex_article | 1.36 | 0.10 | 1.14 |
| pdf_beamer_boadilla | 1.35 | 0.14 | 1.33 |
| pdf_scrbook_large | 1.23 | 0.16 | 1.21 |
| xe_xecjk_chinese | 1.22 | 0.11 | 0.99 |
| pdf_listings_algorithm2e | 1.21 | 0.11 | 1.13 |
| xe_ctex_book | 1.19 | 0.10 | 0.94 |
| xe_siunitx | 1.19 | 0.11 | 0.97 |
| xe_polyglossia_german | 1.17 | 0.10 | 1.05 |
| xe_fontspec_termes | 1.12 | 0.10 | 0.74 |
| pdf_memoir_book | 1.12 | 0.11 | 1.10 |
| arxiv_math_nt_amsart | 1.06 | 0.13 | 0.93 |
| pdf_hyperref_toc | 1.05 | 0.13 | 1.01 |
| xe_fontspec_schola | 1.01 | 0.10 | 0.60 |
| xe_biblatex | 0.73 | 0.13 | 1.01 |

## Documents that failed, differ, or could not be timed

| Document | Engine | Status | Detail |
| --- | --- | --- | --- |
| arxiv_bert | pdf | differs | pages TL 16 / TeXres 16; differing pages [6]; TL `…et.8BERTandOpenAIGPTaresingle-model,singletask.F1scoresarereportedforQ…` vs TeXres `…et.8BERTandOpenAIGPTaresingle-8Seesinglemodel,questiontask.10F1inscore…` |
| arxiv_econ_gn_bibtex | pdf | differs | pages TL 34 / TeXres 34; differing pages [6, 9, 14, 16, 25, 27, 28, 29]; TL `…writethecontractindicatorasχt=1ut≥Rt.(4)Remark1.Weremarkthatusingpsych…` vs TeXres `…writethecontractindicatorasχt=ut≥Rt.(4)Remark1.Weremarkthatusingpsycho…` |
| arxiv_ecta_paper | pdf | texres_failed | ! Font `` (TFM `rsfso10`) has no associated outline program / --> <scratch>/arxiv_ecta_paper/texres/work/ManuscriptEcta_31Jan2019.tex:830:1 / \| / 830 \| ⏎ ! Font `` (TFM `rsfso10`) has no associated outline program / --> <scratch>/arxiv_ecta_paper/texres/work/ManuscriptEcta_31Jan2019.tex:830:1 / \| / 830 \| ⏎ ! Font `` (TFM `rsfso10`) has no associated outline program / --> <scratch>/arxiv_ecta_paper/texres/work/Man |
| arxiv_graded_xy | pdf | differs | pages TL 22 / TeXres 22; differing pages [7, 8, 12, 15, 16, 20]; TL `…Steinbergmapc:G→W\\T:q̃(3.1)Ge/Tc̃qc/GW\\TwhereGeistheclosedsubvarie…` vs TeXres `…Steinbergmapc:G→W\\T:q̃(3.1)GeTc̃qcGW\\TwhereGeistheclosedsubvarietyof…` |
| arxiv_ieeetran_kozen | pdf | differs | pages TL 17 / TeXres 17; differing pages [2, 3, 4, 5, 6, 7, 8, 9]; TL `…quasi-Borelspacesandmeasurabledefineameasurekerneltobeameasurablemapf:…` vs TeXres `…quasi-Borelspacesandmeasurablekerneltobeameasurablemapf:X→MYsuchthatco…` |
| arxiv_jhep_6d_scft | pdf | differs | pages TL 28 / TeXres 28; differing pages [9]; TL `…⊕g222A2211111⊕e8⊕f4⊕g222202198A1⊕A2⊕e6⊕e8⊕f4⊕g222221112A1⊕A2⊕e6⊕e8⊕f4⊕…` vs TeXres `…⊕g222A2211111⊕e8⊕f4⊕g2222021982211122222111122A1⊕A2⊕e6⊕e8⊕f4⊕g2A1⊕A2⊕e…` |
| arxiv_lipics_paper | pdf | differs | pages TL 30 / TeXres 30; differing pages [22]; TL `…mmutinginTk,and1c1121u\|du\|e212c22u\|du\|elet∆=splitc(x,H1,H2).Letρ1:−→p1…` vs TeXres `…mmutinginTk,and1c1121u\|du\|e21222cu\|du\|elet∆=splitc(x,H1,H2).Letρ1:−→p1…` |
| arxiv_lmcs_eating | pdf | differs | pages TL 30 / TeXres 30; differing pages [13, 27]; TL `…inthecaseofaNode:D∗i,Node(a,k)=Σd:D(i,a)D∗i[a/d],fd:Setn∗i,Node(a,…` vs TeXres `…inthecaseofaNode:D∗i,Node(a,k)∗=Σd:D(i,a)Di[a/d],fd:Setn∗i,Node(a,k…` |
| arxiv_recursive_partitioning | pdf | differs | pages TL 48 / TeXres 48; differing pages [22, 23, 34]; TL `…●●●●●●●●●●●●●●●●●●●●●●●0.04●●●●1.0●●●●●●●●●●●●●●●●●●●●●●●●●●●●●●●●●●●0…` vs TeXres `…●●●●●●●●●●●●●●●●●●●●●●●0.04●●●1.0●●●●●●●●●●●●●●●●●●●●●●●●●●●●●●●●●●●●0…` |
| arxiv_revtex41_neuro | pdf | differs | pages TL 17 / TeXres 17; differing pages [2, 9, 17]; TL `…y-dominatedscenarioswherethreePinthesystemreducestotheentropyH(X):=mat…` vs TeXres `…y-dominatedscenarioswherethreemationPinthesystemreducestotheentropyH(X…` |
| gh_lkmpg_book | pdf | texres_failed | ! Package minted Error: Missing definition for highlighting style "vs" (minted executable is unavailable or disabled); attempting to substitute fallback style. / --> <scratch>/gh_lkmpg_book/texres/work/lkmpg.tex:136:48 / \| / 136 \| Linux distributions provide the ⏎ ! Package minted Error: Missing definition for highlighting style "default" (minted executable is unavailable or disabled); attempting to substitute fall |
| gh_phd_thesis_template | pdf | differs | pages TL 39 / TeXres 38; differing pages [11]; TL `…xBInstallingtheCUEDclassfile21Index23Listoffigures2.1Minion...........…` vs TeXres `…xBInstallingtheCUEDclassfile21Listoffigures2.1Minion..................…` |
| gh_tufte_book | pdf | differs | pages TL 42 / TeXres 39; differing pages [5]; TL `…tingandSupport35Bibliography39Index41ListofFigures1Thisisamarginfigure…` vs TeXres `…tingandSupport35Bibliography39ListofFigures1Thisisamarginfigure.Thehel…` |
| gh_tufte_handout | pdf | differs | pages TL 5 / TeXres 5; differing pages [3]; TL `…re.}−1\end{marginfigure}0−1y01Themarginfigureandmargintableenvironment…` vs TeXres `…re.}−1\end{marginfigure}0−1y01xThemarginfigureandmargintableenvironmen…` |
| lua_biblatex | lua | texres_failed | ! Improper alphabetic constant / --> <scratch>/lua_biblatex/texres/work/main.tex:79:1 / \| / 79 \| \end{document} ⏎ ! Improper alphabetic constant / --> <scratch>/lua_biblatex/texres/work/main.tex:79:1 / \| / 79 \| \end{document} ⏎ ! Improper alphabetic constant / --> <scratch>/lua_biblatex/texres/work/main.tex:79:1 / \| / 79 \| \end{document} |
| lua_book | lua | differs | pages TL 40 / TeXres 40; differing pages [27, 28]; TL `…rketandtherisk.Thecommonratede-terminesthelownetworkoftheprice.Inthesi…` vs TeXres `…rketandtherisk.Thecommonratedeterminesthelownetworkoftheprice.Inthesim…` |
| lua_luacode_env | lua | texres_failed | ! File ended while scanning use of \luacode@grab@lines / --> <scratch>/lua_luacode_env/texres/work/main.tex:55:1 / \| / 55 \| \begin{luacode} ⏎ ! Unclosed \begingroup; add the missing \endgroup (1 nested group also open) / --> <scratch>/lua_luacode_env/texres/work/main.tex:55:1 / \| / 55 \| \begin{luacode} |
| lua_polyglossia_russian | lua | texres_failed | ! Package fontspec Error: / \| (fontspec)                The font "cmunrm" cannot be found; this may be but / \| (fontspec)                usually is not a fontspec bug. Either there is a / \| (fontspec)                typo in the font name/file, the font is not ⏎ ! Package fontspec Error: / \| (fontspec)                The font "cmunrm" cannot be found; this may be but / \| (fontspec)                usually is not a |
| pdf_imakeidx | pdf | differs | pages TL 15 / TeXres 13; differing pages [2, 3]; TL `…arameter1340EmpiricalLatency13Index141PartialDataWefindthatthelocalpro…` vs TeXres `…arameter1340EmpiricalLatency131PartialDataWefindthatthelocalprobabilit…` |
| pdf_makeindex | pdf | differs | pages TL 30 / TeXres 28; differing pages []; TL `…iciencylimitsthemarginalstate.Indexanalysisearly,21random,5forecast,18…` vs TeXres `…iciencylimitsthemarginalstate.…` |
| xe_polyglossia_greek | xe | texres_failed | ! Package fontspec Error: ^^J(fontspec)                The font "cmunrm" cannot be found; this may be but^^J(fontspec)                usually is not a fontspec bug. Either there is a^^J(fontspec)                typo in the font name/file, the font is not^^J(fontspec)                installed (correc ⏎ ! Package fontspec Error: ^^J(fontspec)                The font "cmunrm" cannot be found; this may be but^^J(fontspec |
| xe_polyglossia_polish | xe | differs | pages TL 4 / TeXres 4; differing pages [1, 2, 3, 4]; TL `…zasemrównowagagospodarkisięsta-bilizuje.Ponadtoniepewnośćzmieniadecyzj…` vs TeXres `…zasemrównowagagospodarkisięstabilizuje.Ponadtoniepewnośćzmieniadecyzję…` |
| xe_polyglossia_russian | xe | texres_failed | ! Package fontspec Error: ^^J(fontspec)                The font "cmunrm" cannot be found; this may be but^^J(fontspec)                usually is not a fontspec bug. Either there is a^^J(fontspec)                typo in the font name/file, the font is not^^J(fontspec)                installed (correc ⏎ ! Package fontspec Error: ^^J(fontspec)                The font "cmunrm" cannot be found; this may be but^^J(fontspec |
| xe_scrbook_fontspec | xe | differs | pages TL 36 / TeXres 36; differing pages [15]; TL `…).Wefindthatthecentralchoiceaffectsthecost,whiletheempiricaltestlimits…` vs TeXres `…).Wefindthatthecentralchoiceaf-fectsthecost,whiletheempiricaltestlimit…` |

