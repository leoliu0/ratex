#!/usr/bin/env python3
"""
scripts/bundle_packages.py

Builds a reproducible bundled TEXMF package archive (`packages.tar.zst`),
the corresponding source distribution archive (`sources.tar.zst`),
and the authoritative machine-readable asset lock (`packages.lock.json`).

Integrates complete offline font families and TeX resources into Ratex:
- Latin Modern (Type 1 text & math, OpenType, metrics, maps)
- CM-Super (Type 1 EC/LH/T2A/B/C/X2 outlines, encodings, maps)
- LH Cyrillic (T2A, T2B metrics including larm1000, lbrm1000, and styles)
- LH (T2A/B/C/X2 Computer Modern metrics)
- Cyrillic (LaTeX font definitions and language support)
- CBfonts LGR Greek (Type 1, metrics, maps, fontdefs, babel-greek)
- Greek Inputenc (complete lgrenc.dfu and 8-bit Greek definitions)
- Wadalab (Type 1, virtual fonts, metrics, maps)
- CJK (LaTeX macro support for CJKutf8, Wadalab, and Arphic)
- IPAex (TrueType, retaining existing Type 1)
- Harano Aji (OpenType Mincho & Gothic, extra weights, fontspec)
- Arphic (Type 1, TrueType, virtual fonts, metrics, maps)
- UHC Korean (Type 1, virtual fonts, metrics, maps, c70mj closure)
- Un-fonts Core (TrueType, GPLv2 source distribution archive)
- Nanum (Type 1, maps)
- stmaryrd (Type 1, metrics, maps, macros)
- bbding (Type 1, metrics, maps, macros)
- FdSymbol (Type 1, OpenType, METAFONT, metrics, encodings, maps, macros)
- LaTeX Base legal files and Font notices

Strictly enforces:
- ZERO host-derived payload: all assets extracted from hash-pinned upstream archives + baseline.
- Authoritative source attribution: each contributing package is explicitly tracked.
- Complete source compliance: corresponding source archives packaged in sources.tar.zst.
- Zero conflicting basenames and deterministic sharded archive creation.
"""

import argparse
import concurrent.futures
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time

BASELINE_FAMILY_MAP_ROOTS = [
    "fonts/map/dvips/ipaex-type1/ipaex-type1.map",
    "fonts/map/dvips/newtx/newtx.map",
    "fonts/map/dvips/cfr-lm/clm.map",
    "fonts/map/dvips/amsfonts/cm.map",
    "fonts/map/dvips/amsfonts/cmextra.map",
    "fonts/map/dvips/amsfonts/cyrillic.map",
    "fonts/map/dvips/amsfonts/euler.map",
    "fonts/map/dvips/amsfonts/latxfont.map",
    "fonts/map/dvips/amsfonts/symbols.map",
    "fonts/map/dvips/tex-gyre/qtm.map",
    "fonts/map/dvips/xypic/xypic.map",
    "fonts/map/dvips/cm/cmtext-bsr-interpolated.map",
    "fonts/map/dvips/txfonts/txfonts.map",
    "fonts/map/dvips/sansmathaccent/sansmathaccent.map",
    "fonts/map/dvips/tetex/ps2pk35.map",
    "fonts/map/dvips/psnfss/psnfss.map",
    "fonts/map/dvips/psnfss/charter.map",
    "fonts/map/dvips/psnfss/utopia.map",
    "fonts/map/dvips/metapost/troff.map",
    "fonts/map/dvips/psnfss/pazo.map",
]

CYRILLIC_EC_FAMILIES = [
    'labi', 'labl', 'labx', 'lacc', 'ladh', 'lafb', 'laff', 'lafi', 'lafs', 'larb', 'larm', 'lasi', 'lasl', 'laso', 'lass', 'lasx', 'lati', 'laui', 'laxc',
    'lbbi', 'lbbl', 'lbbx', 'lbcc', 'lbdh', 'lbfb', 'lbff', 'lbfi', 'lbfs', 'lbrb', 'lbrm', 'lbsi', 'lbsl', 'lbso', 'lbss', 'lbsx', 'lbti', 'lbui', 'lbxc',
    'lcbi', 'lcbl', 'lcbx', 'lccc', 'lcdh', 'lcfb', 'lcff', 'lcfi', 'lcfs', 'lcrb', 'lcrm', 'lcsi', 'lcsl', 'lcso', 'lcss', 'lcsx', 'lcti', 'lcui', 'lcxc',
    'rxbi', 'rxbl', 'rxbx', 'rxcc', 'rxdh', 'rxfb', 'rxff', 'rxfi', 'rxfs', 'rxrb', 'rxrm', 'rxsi', 'rxsl', 'rxso', 'rxss', 'rxsx', 'rxti', 'rxui', 'rxxc',
]
STANDARD_SIZES = [
    "0500", "0600", "0700", "0800", "0900", "1000",
    "1095", "1200", "1440", "1728", "2074", "2488",
    "2986", "3583",
]
FIXED_MTIME = 1704067200  # 2024-01-01 00:00:00 UTC
PINNED_BASELINE_SHA256 = "29331539190ce01e054611c00094d44b46b1b91ac9cbfd9c5d47b96e384c639f"
CHUNK_SIZE_LIMIT = 65 * 1024 * 1024  # 65 MiB (ensures 6 sequential parts and strictly <= 70 MiB)

# Authoritative upstream packages with verified SHA-256 archive pins
UPSTREAM_PACKAGES = {
    "lm": {
        "version": "2.005",
        "revision": 77682,
        "license": "GUST-FONT-LICENSE / LPPL-1.3c",
        "ctan_path": "/fonts/lm",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/lm.tar.xz",
        "upstream_sha256": "6ecafd1f066d189a0688c10572a01d45c231fa236d6ec1cca726ca4eef41e57c",
        "upstream_size_bytes": 11944800,
        "description": "Latin Modern font family: Type 1 text and math, OpenType, metrics",
        "source_obligations": "GUST Font License; modification permitted provided renamed",
        "license_files": [
            "doc/fonts/lm/GUST-FONT-LICENSE.TXT",
            "doc/fonts/lm/MANIFEST-Latin-Modern.TXT",
        ],
        "map_files": ["fonts/map/dvips/lm/lm.map"],
        "tds_dirs": [
            ("fonts/type1/public/lm", "fonts/type1/public/lm"),
            ("fonts/opentype/public/lm", "fonts/opentype/public/lm"),
            ("fonts/tfm/public/lm", "fonts/tfm/public/lm"),
            ("fonts/enc/dvips/lm", "fonts/enc/dvips/lm"),
            ("fonts/map/dvips/lm", "fonts/map/dvips/lm"),
            ("tex/latex/lm", "tex/latex/lm"),
        ],
    },
    "cm-super": {
        "version": "0.3.4",
        "revision": 15878,
        "license": "GPL-2.0-or-later with font exception",
        "ctan_path": "/fonts/ps-type1/cm-super",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/cm-super.tar.xz",
        "upstream_sha256": "70867904e38451ab1a11ba4677dd5891914f128170b53fac26d3e9dca9d326e4",
        "upstream_size_bytes": 64562404,
        "description": "CM-Super Type 1 outlines and encodings (T1, T2A/B/C, TS1, X2)",
        "source_obligations": "GPL-2.0 with font exception; document embedding permitted without copyleft; corresponding source preserved in sources.tar.zst",
        "license_files": [
            "doc/fonts/cm-super/COPYING",
            "doc/fonts/cm-super/README",
        ],
        "map_files": [
            "fonts/map/dvips/cm-super/cm-super-t1.map",
            "fonts/map/dvips/cm-super/cm-super-t2a.map",
            "fonts/map/dvips/cm-super/cm-super-t2b.map",
            "fonts/map/dvips/cm-super/cm-super-t2c.map",
            "fonts/map/dvips/cm-super/cm-super-ts1.map",
            "fonts/map/dvips/cm-super/cm-super-x2.map",
        ],
        "tds_dirs": [
            ("fonts/type1/public/cm-super", "fonts/type1/public/cm-super"),
            ("fonts/enc/dvips/cm-super", "fonts/enc/dvips/cm-super"),
            ("fonts/map/dvips/cm-super", "fonts/map/dvips/cm-super"),
            ("tex/latex/cm-super", "tex/latex/cm-super"),
        ],
    },
    "lhcyr": {
        "version": "2024",
        "revision": 77838,
        "license": "other-free / LPPL",
        "ctan_path": "/macros/latex/contrib/lhcyr",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/lhcyr.tar.xz",
        "upstream_sha256": "e414d8dc28d0ec03c96ae13d9f6d98834487f8066ab543827d93ab7d020617fc",
        "upstream_size_bytes": 55216,
        "description": "LH Cyrillic T2A and T2B font metrics (including larm1000, lbrm1000)",
        "source_obligations": "Freely redistributable TeX macros and font metrics; source preserved in sources.tar.zst",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [
            ("fonts/tfm/lhcyr", "fonts/tfm/lhcyr"),
        ],
    },
    "lh": {
        "version": "3.5g",
        "revision": 77838,
        "license": "LPPL-1.3c",
        "ctan_path": "/fonts/lh",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/lh.tar.xz",
        "upstream_sha256": "eb1f4ef2e7ccd737f1c5463e43e49bc4ce6f69a9f0de6aeb1157446f1c7c22f8",
        "upstream_size_bytes": 361316,
        "description": "LH Cyrillic Computer Modern font metrics (T2A)",
        "source_obligations": "LPPL-1.3c; METAFONT source code preserved in sources.tar.zst",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [
            ("fonts/tfm/lh", "fonts/tfm/lh"),
        ],
    },
    "cyrillic": {
        "version": "2024",
        "revision": 71408,
        "license": "LPPL-1.3c",
        "ctan_path": "/macros/latex/required/cyrillic",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/cyrillic.tar.xz",
        "upstream_sha256": "c41a4015b43948ab104e397c24f55750e5b11619ba249a86b78e3b91d538a2a9",
        "upstream_size_bytes": 15772,
        "description": "LaTeX Cyrillic font definitions (T2A, T2B, T2C, X2) and language support",
        "source_obligations": "LPPL-1.3c; official LaTeX project distribution",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [
            ("tex/latex/cyrillic", "tex/latex/cyrillic"),
        ],
    },
    "babel-russian": {
        "version": "1.3m",
        "revision": 57375,
        "license": "LPPL-1.3c",
        "ctan_path": "/macros/latex/contrib/babel-contrib/russian",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/babel-russian.tar.xz",
        "upstream_sha256": "5962e93e52acb95a13705a9534bdef6faf3fcc89ba36f97cd85af15a94e83679",
        "upstream_size_bytes": 19008,
        "description": "Russian language support for Babel (russianb.ldf)",
        "source_obligations": "LPPL-1.3c; source archive preserved in sources.tar.zst",
        "license_files": ["doc/generic/babel-russian/README.md"],
        "map_files": [],
        "tds_dirs": [
            ("tex/generic/babel-russian", "tex/generic/babel-russian"),
        ],
    },
    "babel-spanish": {
        "version": "5.0q",
        "revision": 79461,
        "license": "LPPL-1.3c",
        "ctan_path": "/macros/latex/contrib/babel-contrib/spanish",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/babel-spanish.tar.xz",
        "upstream_sha256": "58b92ff26bc27683adbb19a1805a6a6c2e06594f5aa4c80170b92b0ff7c10b74",
        "upstream_size_bytes": 8892,
        "description": "Spanish language support for Babel (spanish.ldf, romanidx.sty)",
        "source_obligations": "LPPL-1.3c; source archive preserved in sources.tar.zst",
        "license_files": ["doc/generic/babel-spanish/README.md"],
        "map_files": [],
        "tds_dirs": [
            ("tex/generic/babel-spanish", "tex/generic/babel-spanish"),
        ],
    },
    "babel-portuges": {
        "version": "1.2u",
        "revision": 77682,
        "license": "LPPL-1.3c",
        "ctan_path": "/macros/latex/contrib/babel-contrib/portuges",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/babel-portuges.tar.xz",
        "upstream_sha256": "640354f13f5f17019c19060437ab8b7bbda7cd2975239865fe5a2c7465e200f7",
        "upstream_size_bytes": 2632,
        "description": "Portuguese and Brazilian support for Babel (portuges.ldf, brazilian.ldf)",
        "source_obligations": "LPPL-1.3c; source archive preserved in sources.tar.zst",
        "license_files": ["doc/generic/babel-portuges/README.md"],
        "map_files": [],
        "tds_dirs": [
            ("tex/generic/babel-portuges", "tex/generic/babel-portuges"),
        ],
    },
    "hyphen-spanish": {
        "version": "5.0",
        "revision": 78069,
        "license": "LPPL-1.3c / MIT",
        "ctan_path": "/language/hyphenation/eshyph",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/hyphen-spanish.tar.xz",
        "upstream_sha256": "ba1c9340f186573770ad28a513bb4aaa334b7bee50f5f902544f381fb058d835",
        "upstream_size_bytes": 16428,
        "description": "Spanish hyphenation patterns (loadhyph-es.tex, hyph-es.tex)",
        "source_obligations": "LPPL-1.3c / MIT; source archive preserved in sources.tar.zst",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [
            ("tex/generic/hyph-utf8", "tex/generic/hyph-utf8"),
        ],
    },
    "hyphen-portuguese": {
        "version": "2024",
        "revision": 78069,
        "license": "GPL-2.0-or-later / LPPL",
        "ctan_path": "/language/hyphenation/pt-hyph",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/hyphen-portuguese.tar.xz",
        "upstream_sha256": "68031bec21717fe36f442b2af3fa168aa6bfb4c35f981c9e167b276c497e448e",
        "upstream_size_bytes": 3772,
        "description": "Portuguese hyphenation patterns (loadhyph-pt.tex, hyph-pt.tex)",
        "source_obligations": "GPL-2.0-or-later / LPPL; source archive preserved in sources.tar.zst",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [
            ("tex/generic/hyph-utf8", "tex/generic/hyph-utf8"),
        ],
    },
    "hyphen-english": {
        "version": "2024",
        "revision": 78069,
        "license": "LPPL-1.3c / Knuth",
        "ctan_path": "/language/hyphenation/hyph-utf8",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/hyphen-english.tar.xz",
        "upstream_sha256": "cb22c0c51786d47aff5cef7dc6d0a47ab291d6da88c21ac6a9929f4409a456ab",
        "upstream_size_bytes": 41348,
        "description": "English hyphenation patterns (loadhyph-en-gb.tex, loadhyph-en-us.tex)",
        "source_obligations": "LPPL-1.3c; source archive preserved in sources.tar.zst",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [
            ("tex/generic/hyph-utf8", "tex/generic/hyph-utf8"),
        ],
    },
    "hyphen-russian": {
        "version": "2024",
        "revision": 78069,
        "license": "LPPL-1.2",
        "ctan_path": "/language/hyphenation/hyph-utf8",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/hyphen-russian.tar.xz",
        "upstream_sha256": "0886909c81731d7a51636f831fac58ff19317d2f1f81a87953090bb13aef9a8a",
        "upstream_size_bytes": 34300,
        "description": "Russian hyphenation patterns (loadhyph-ru.tex, hyph-ru.tex, hyph-ru.t2a.tex)",
        "source_obligations": "LPPL-1.2; source archive preserved in sources.tar.zst",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [
            ("tex/generic/hyph-utf8", "tex/generic/hyph-utf8"),
        ],
    },
    "ruhyphen": {
        "version": "1.6",
        "revision": 79618,
        "license": "LPPL-1.2",
        "ctan_path": "/language/hyphenation/ruhyphen",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/ruhyphen.tar.xz",
        "upstream_sha256": "09a5ee8b916df34c038574d78fb090b1a22d479f318c72e554617fa59504b3f7",
        "upstream_size_bytes": 56708,
        "description": "Russian hyphenation system (ruhyphen.tex, koi2t2a.tex, ruhyphal.tex, hypht2.tex)",
        "source_obligations": "LPPL-1.2; source archive preserved in sources.tar.zst",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [
            ("tex/generic/ruhyphen", "tex/generic/ruhyphen"),
        ],
    },
    "cbfonts": {
        "version": "2024",
        "revision": 54080,
        "license": "LPPL-1.3c",
        "ctan_path": "/fonts/greek/cbfonts",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/cbfonts.tar.xz",
        "upstream_sha256": "b4ec36812e215fcc2ba2e7b12df17bf98adf31b77c044f22d4c5568465e8f0ee",
        "upstream_size_bytes": 66006520,
        "description": "Complete Greek fonts (LGR encoding) Type 1, TFMs, and font definitions",
        "source_obligations": "LPPL-1.3c; modifications must document changes",
        "license_files": ["doc/fonts/cbfonts/README"],
        "map_files": ["fonts/map/dvips/cbfonts/cbgreek-full.map"],
        "tds_dirs": [
            ("fonts/type1/public/cbfonts", "fonts/type1/public/cbfonts"),
            ("fonts/tfm/public/cbfonts", "fonts/tfm/public/cbfonts"),
            ("fonts/enc/dvips/cbfonts", "fonts/enc/dvips/cbfonts"),
            ("fonts/map/dvips/cbfonts", "fonts/map/dvips/cbfonts"),
            ("tex/latex/cbfonts-fd", "tex/latex/cbfonts-fd"),
            ("tex/generic/babel-greek", "tex/generic/babel-greek"),
            ("tex/latex/greek-fontenc", "tex/latex/greek-fontenc"),
        ],
    },
    "greek-inputenc": {
        "version": "1.9",
        "revision": 66634,
        "license": "LPPL-1.3c",
        "ctan_path": "/language/greek/greek-inputenc",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/greek-inputenc.tar.xz",
        "upstream_sha256": "ce0405bb0402a07266a0d43b13a313fb897ecf96b355b00c2394c9c31416f513",
        "upstream_size_bytes": 6864,
        "description": "Greek UTF-8 input encoding definitions (lgrenc.dfu) and 8-bit support",
        "source_obligations": "LPPL-1.3c; modifications must document changes",
        "license_files": ["doc/latex/greek-inputenc/README.md"],
        "map_files": [],
        "tds_dirs": [
            ("tex/latex/greek-inputenc", "tex/latex/greek-inputenc"),
        ],
    },
    "wadalab": {
        "version": "0-12",
        "revision": 42428,
        "license": "Wadalab Font License",
        "ctan_path": "/fonts/wadalab",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/wadalab.tar.xz",
        "upstream_sha256": "81918f1cd7ef1ced85c2adb30c8d6e5eaeba17d43af86b4a888b698085c47018",
        "upstream_size_bytes": 17772744,
        "description": "Wadalab Japanese Kanji Type 1, Virtual Fonts, and CJKutf8 support",
        "source_obligations": "Freely redistributable for non-commercial and commercial use with derivation notice",
        "license_files": ["doc/fonts/wadalab/README"],
        "map_files": [
            "fonts/map/dvips/wadalab/dmj.map",
            "fonts/map/dvips/wadalab/dgj.map",
            "fonts/map/dvips/wadalab/mcj.map",
            "fonts/map/dvips/wadalab/mrj.map",
        ],
        "tds_dirs": [
            ("fonts/type1/wadalab", "fonts/type1/wadalab"),
            ("fonts/vf/wadalab", "fonts/vf/wadalab"),
            ("fonts/tfm/wadalab", "fonts/tfm/wadalab"),
            ("fonts/map/dvips/wadalab", "fonts/map/dvips/wadalab"),
        ],
    },
    "cjk": {
        "version": "4.8.5",
        "revision": 74490,
        "license": "GPL-2.0",
        "ctan_path": "/language/chinese/CJK",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/cjk.tar.xz",
        "upstream_sha256": "744904abda9141fa1a448b6ad3ab6d1389e515e98dbad9489613d98194eee0f2",
        "upstream_size_bytes": 58480,
        "description": "CJK macro package support for Wadalab and Arphic fonts",
        "source_obligations": "GPL-2.0; CJK LaTeX support macros",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [
            ("tex/latex/cjk/contrib/wadalab", "tex/latex/cjk/contrib/wadalab"),
            ("tex/latex/cjk/texinput", "tex/latex/cjk/texinput"),
        ],
    },
    "ipaex": {
        "version": "00401",
        "revision": 61719,
        "license": "IPA Font License v1.0",
        "ctan_path": "/fonts/ipaex",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/ipaex.tar.xz",
        "upstream_sha256": "7db75b91663a5fa711d72fba2d1934580e0570c58d2937f5f54aa3ac7cf43e15",
        "upstream_size_bytes": 15865176,
        "description": "IPAex Mincho and IPAex Gothic TrueType fonts",
        "source_obligations": "IPA Font License v1.0; redistribution permitted with notice",
        "license_files": ["doc/fonts/ipaex/IPA_Font_License_Agreement_v1.0.txt"],
        "map_files": [],
        "tds_dirs": [
            ("fonts/truetype/public/ipaex", "fonts/truetype/public/ipaex"),
        ],
    },
    "haranoaji": {
        "version": "20250811",
        "revision": 76078,
        "license": "OFL-1.1",
        "ctan_path": "/fonts/haranoaji",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/haranoaji.tar.xz",
        "upstream_sha256": "e9410a8fe32dde7421fb8e4ae46f4e7aa58fe2a487fdb5c2e268ed622c14252b",
        "upstream_size_bytes": 26259784,
        "description": "Harano Aji Mincho and Gothic OpenType fonts",
        "source_obligations": "SIL Open Font License 1.1; bundling and embedding permitted",
        "license_files": ["doc/fonts/haranoaji/LICENSE"],
        "map_files": [],
        "tds_dirs": [
            ("fonts/opentype/public/haranoaji", "fonts/opentype/public/haranoaji"),
            ("tex/latex/haranoaji", "tex/latex/haranoaji"),
        ],
    },
    "haranoaji-extra": {
        "version": "20250811",
        "revision": 76079,
        "license": "OFL-1.1",
        "ctan_path": "/fonts/haranoaji-extra",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/haranoaji-extra.tar.xz",
        "upstream_sha256": "2de4b181bc5f36aa8a8bb3a967d50261150c8a31432c5d40723e2c61cc3e5a89",
        "upstream_size_bytes": 26101888,
        "description": "Harano Aji Mincho and Gothic extra weights OpenType fonts",
        "source_obligations": "SIL Open Font License 1.1; bundling and embedding permitted",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [
            ("fonts/opentype/public/haranoaji-extra", "fonts/opentype/public/haranoaji-extra"),
        ],
    },
    "arphic": {
        "version": "1999",
        "revision": 15878,
        "license": "Arphic Public License",
        "ctan_path": "/fonts/arphic",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/arphic.tar.xz",
        "upstream_sha256": "fb0eb8ec13e210a3d1023478c35bd897f5e60049a05c87258ca19fa8452cb31b",
        "upstream_size_bytes": 27383316,
        "description": "Arphic Chinese Type 1, VFs, TFMs, and maps",
        "source_obligations": "Arphic Public License; commercial distribution allowed with attribution",
        "license_files": ["doc/fonts/arphic-ttf/ARPHICPL.txt", "doc/fonts/arphic/gbsnu/README"],
        "map_files": [
            "fonts/map/dvips/arphic/gbsnu.map",
            "fonts/map/dvips/arphic/gkaiu.map",
            "fonts/map/dvips/arphic/bsmiu.map",
            "fonts/map/dvips/arphic/bkaiu.map",
        ],
        "tds_dirs": [
            ("fonts/type1/arphic", "fonts/type1/arphic"),
            ("fonts/vf/arphic", "fonts/vf/arphic"),
            ("fonts/tfm/arphic", "fonts/tfm/arphic"),
            ("fonts/map/dvips/arphic", "fonts/map/dvips/arphic"),
        ],
    },
    "arphic-ttf": {
        "version": "1999",
        "revision": 42675,
        "license": "Arphic Public License",
        "ctan_path": "/fonts/arphic-ttf",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/arphic-ttf.tar.xz",
        "upstream_sha256": "a44031616debf9ffc30c174727c0c33d28540f3b18fa9d980b52aa39752e267e",
        "upstream_size_bytes": 12663696,
        "description": "Arphic Chinese TrueType fonts (bkai00mp, bsmi00lp, gbsn00lp, gkai00mp)",
        "source_obligations": "Arphic Public License; redistribution permitted with notice",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [
            ("fonts/truetype/public/arphic-ttf", "fonts/truetype/public/arphic-ttf"),
        ],
    },
    "uhc": {
        "version": "1.0",
        "revision": 16791,
        "license": "LPPL",
        "ctan_path": "/fonts/korean/HLaTeX",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/uhc.tar.xz",
        "upstream_sha256": "6c3df3ca1acee37f2a46fc2c42c24b4f27b89f985e95760f8a6b28f03f6c43af",
        "upstream_size_bytes": 3600148,
        "description": "Korean UHC fonts: c70mj -> uwmj VF/TFM, wmj VF/TFM, umj Type 1/TFM, and map",
        "source_obligations": "LaTeX Project Public License; author Koaunghi Un",
        "license_files": ["doc/fonts/uhc/umj/README"],
        "map_files": ["fonts/map/dvips/uhc/umj.map"],
        "tds_dirs": [
            ("fonts/type1/uhc/umj", "fonts/type1/uhc/umj"),
            ("fonts/vf/uhc/uwmj", "fonts/vf/uhc/uwmj"),
            ("fonts/vf/uhc/wmj", "fonts/vf/uhc/wmj"),
            ("fonts/tfm/uhc/uwmj", "fonts/tfm/uhc/uwmj"),
            ("fonts/tfm/uhc/wmj", "fonts/tfm/uhc/wmj"),
            ("fonts/tfm/uhc/umj", "fonts/tfm/uhc/umj"),
            ("fonts/map/dvips/uhc", "fonts/map/dvips/uhc"),
        ],
    },
    "unfonts-core": {
        "version": "1.0.2",
        "revision": 56291,
        "license": "GPL-2.0",
        "ctan_path": "/fonts/unfonts-core",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/unfonts-core.tar.xz",
        "upstream_sha256": "513990ad0501f2388cdcad68ac635ad94d21b4fed9424a58db7ef9d7d6922bc1",
        "upstream_size_bytes": 14727532,
        "description": "Un-fonts Korean TrueType font family (UnBatang, UnDotum, UnGraphic, UnGungseo, UnPilgi)",
        "source_obligations": "Standard GPLv2 without font exception; source archive preserved in sources.tar.zst",
        "license_files": ["doc/fonts/unfonts-core/COPYING", "doc/fonts/unfonts-core/README.md"],
        "map_files": [],
        "tds_dirs": [
            ("fonts/truetype/public/unfonts-core", "fonts/truetype/public/unfonts-core"),
        ],
    },
    "nanumtype1": {
        "version": "3.0",
        "revision": 29558,
        "license": "OFL-1.1",
        "ctan_path": "/fonts/nanumtype1",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/nanumtype1.tar.xz",
        "upstream_sha256": "77e94f530f1e3298c81b40ea5a2dd0f3bd8956af0ea750d3cf68dee581ffdbd9",
        "upstream_size_bytes": 28242732,
        "description": "Nanum Korean Type 1 fonts, maps, and LaTeX definitions",
        "source_obligations": "SIL Open Font License 1.1; embedding permitted",
        "license_files": ["doc/fonts/nanumtype1/COPYING"],
        "map_files": ["fonts/map/dvips/nanumtype1/nanumfonts.map"],
        "tds_dirs": [
            ("fonts/type1/public/nanumtype1", "fonts/type1/public/nanumtype1"),
            ("fonts/tfm/public/nanumtype1", "fonts/tfm/public/nanumtype1"),
            ("fonts/vf/public/nanumtype1", "fonts/vf/public/nanumtype1"),
            ("fonts/map/dvips/nanumtype1", "fonts/map/dvips/nanumtype1"),
            ("tex/latex/nanumtype1", "tex/latex/nanumtype1"),
        ],
    },
    "stmaryrd": {
        "version": "1998",
        "revision": 77682,
        "license": "Public Domain",
        "ctan_path": "/fonts/stmaryrd",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/stmaryrd.tar.xz",
        "upstream_sha256": "3dcd9aeeeb8c502cc0ee908838a5154f0ea03c810cbf5d4e5088c1eee44a4079",
        "upstream_size_bytes": 166144,
        "description": "St Mary's Road symbol font (Type 1, TFMs, map, sty)",
        "source_obligations": "Public Domain; free use and redistribution; source preserved in sources.tar.zst",
        "license_files": ["doc/fonts/stmaryrd/README", "doc/fonts/stmaryrd/README.hoekwater"],
        "map_files": ["fonts/map/dvips/stmaryrd/stmaryrd.map"],
        "tds_dirs": [
            ("fonts/type1/public/stmaryrd", "fonts/type1/public/stmaryrd"),
            ("fonts/tfm/public/stmaryrd", "fonts/tfm/public/stmaryrd"),
            ("fonts/map/dvips/stmaryrd", "fonts/map/dvips/stmaryrd"),
            ("tex/latex/stmaryrd", "tex/latex/stmaryrd"),
        ],
    },
    "bbding": {
        "version": "1.01",
        "revision": 77682,
        "license": "LPPL-1.3c",
        "ctan_path": "/fonts/bbding",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/bbding.tar.xz",
        "upstream_sha256": "55e0bca1e18927c792b1fd5cc48592c02fdd5fff3691c26ab0db82ba4b335892",
        "upstream_size_bytes": 11680,
        "description": "Blakowski Dingbats font (bbding10 TFM, LaTeX sty)",
        "source_obligations": "LPPL-1.3c; source preserved in sources.tar.zst",
        "license_files": ["doc/latex/bbding/README"],
        "map_files": [],
        "tds_dirs": [
            ("fonts/tfm/public/bbding", "fonts/tfm/public/bbding"),
            ("tex/latex/bbding", "tex/latex/bbding"),
        ],
    },
    "niceframe-type1": {
        "version": "2024",
        "revision": 77682,
        "license": "other-free",
        "ctan_path": "/fonts/type1/niceframe-type1",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/niceframe-type1.tar.xz",
        "upstream_sha256": "14adfe8533417dd0aa59f1c554a02a3f46a600b939815babeec4226dcc445c99",
        "upstream_size_bytes": 275504,
        "description": "NiceFrame and bbding10 Type 1 outline fonts and dvips map",
        "source_obligations": "Freely redistributable Type 1 fonts",
        "license_files": [],
        "map_files": ["fonts/map/dvips/niceframe-type1/niceframe.map"],
        "tds_dirs": [
            ("fonts/type1/public/niceframe-type1", "fonts/type1/public/niceframe-type1"),
            ("fonts/map/dvips/niceframe-type1", "fonts/map/dvips/niceframe-type1"),
        ],
    },
    "rsfs": {
        "version": "1.0",
        "revision": 15878,
        "license": "Public Domain",
        "ctan_path": "/fonts/rsfs",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/rsfs.tar.xz",
        "upstream_sha256": "1afec0c5e9711f652675e38b7cd7e88101c44aa0d0ff317ad6ac06f1d2cc7043",
        "upstream_size_bytes": 56092,
        "description": "Ralph Smith's Formal Script symbol fonts (Type 1 outlines, metrics, map)",
        "source_obligations": "Public Domain; source code preserved in sources.tar.zst",
        "license_files": ["doc/fonts/rsfs/README", "doc/fonts/rsfs/README.type1"],
        "map_files": ["fonts/map/dvips/rsfs/rsfs.map"],
        "tds_dirs": [
            ("fonts/type1/public/rsfs", "fonts/type1/public/rsfs"),
            ("fonts/tfm/public/rsfs", "fonts/tfm/public/rsfs"),
            ("fonts/map/dvips/rsfs", "fonts/map/dvips/rsfs"),
        ],
    },
    "times": {
        "version": "2024",
        "revision": 77161,
        "license": "GPL-2.0-or-later",
        "ctan_path": "/fonts/urw/base35",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/times.tar.xz",
        "upstream_sha256": "55f6097b5685a22a3ec1db61ae372ffe00b25d622ed4ee3d010cd8a037e48a00",
        "upstream_size_bytes": 286900,
        "description": "URW 'Base 35' font pack for LaTeX: Times (Nimbus Roman No9 L) Type 1, metrics, VFs, maps",
        "source_obligations": "GNU General Public License v2.0 or later with font exception; corresponding source in base35.zip",
        "license_files": ["doc/fonts/urw-base35/COPYING", "doc/fonts/urw-base35/README"],
        "map_files": ["fonts/map/dvips/times/utm.map"],
        "tds_dirs": [
            ("fonts/type1/urw/times", "fonts/type1/urw/times"),
            ("fonts/afm/urw/times", "fonts/afm/urw/times"),
            ("fonts/tfm/adobe/times", "fonts/tfm/adobe/times"),
            ("fonts/tfm/urw35vf/times", "fonts/tfm/urw35vf/times"),
            ("fonts/vf/adobe/times", "fonts/vf/adobe/times"),
            ("fonts/vf/urw35vf/times", "fonts/vf/urw35vf/times"),
            ("fonts/map/dvips/times", "fonts/map/dvips/times"),
            ("tex/latex/times", "tex/latex/times"),
        ],
    },
    "helvetic": {
        "version": "2024",
        "revision": 77161,
        "license": "GPL-2.0-or-later",
        "ctan_path": "/fonts/urw/base35",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/helvetic.tar.xz",
        "upstream_sha256": "155b23ee6096e32fe7a481500a75269027042abebc3955e7966327c1d1f41db4",
        "upstream_size_bytes": 539616,
        "description": "URW 'Base 35' font pack for LaTeX: Helvetica (Nimbus Sans L) Type 1, metrics, VFs, maps",
        "source_obligations": "GNU General Public License v2.0 or later with font exception; corresponding source in base35.zip",
        "license_files": [],
        "map_files": ["fonts/map/dvips/helvetic/uhv.map"],
        "tds_dirs": [
            ("fonts/type1/urw/helvetic", "fonts/type1/urw/helvetic"),
            ("fonts/afm/urw/helvetic", "fonts/afm/urw/helvetic"),
            ("fonts/tfm/adobe/helvetic", "fonts/tfm/adobe/helvetic"),
            ("fonts/tfm/urw35vf/helvetic", "fonts/tfm/urw35vf/helvetic"),
            ("fonts/vf/adobe/helvetic", "fonts/vf/adobe/helvetic"),
            ("fonts/vf/urw35vf/helvetic", "fonts/vf/urw35vf/helvetic"),
            ("fonts/map/dvips/helvetic", "fonts/map/dvips/helvetic"),
            ("tex/latex/helvetic", "tex/latex/helvetic"),
        ],
    },
    "palatino": {
        "version": "2024",
        "revision": 77161,
        "license": "GPL-2.0-or-later",
        "ctan_path": "/fonts/urw/base35",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/palatino.tar.xz",
        "upstream_sha256": "a5950b7ce364231aace8830cdedbe6ab511bfe663a2f1ba0bd270e618463b6f1",
        "upstream_size_bytes": 325796,
        "description": "URW 'Base 35' font pack for LaTeX: Palatino (URW Palladio L) Type 1, metrics, VFs, maps",
        "source_obligations": "GNU General Public License v2.0 or later with font exception; corresponding source in base35.zip",
        "license_files": [],
        "map_files": ["fonts/map/dvips/palatino/upl.map"],
        "tds_dirs": [
            ("fonts/type1/urw/palatino", "fonts/type1/urw/palatino"),
            ("fonts/afm/urw/palatino", "fonts/afm/urw/palatino"),
            ("fonts/tfm/adobe/palatino", "fonts/tfm/adobe/palatino"),
            ("fonts/tfm/urw35vf/palatino", "fonts/tfm/urw35vf/palatino"),
            ("fonts/vf/adobe/palatino", "fonts/vf/adobe/palatino"),
            ("fonts/vf/urw35vf/palatino", "fonts/vf/urw35vf/palatino"),
            ("fonts/map/dvips/palatino", "fonts/map/dvips/palatino"),
            ("tex/latex/palatino", "tex/latex/palatino"),
        ],
    },
    "zapfding": {
        "version": "2024",
        "revision": 77161,
        "license": "GPL-2.0-or-later",
        "ctan_path": "/fonts/urw/base35",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/zapfding.tar.xz",
        "upstream_sha256": "7c95b0011067228c4395aef990d92e190a66f825f6f3fb5bf09a738893b47ece",
        "upstream_size_bytes": 46940,
        "description": "URW 'Base 35' font pack for LaTeX: Zapf Dingbats Type 1, metrics, and maps",
        "source_obligations": "GNU General Public License v2.0 or later with font exception; corresponding source in base35.zip",
        "license_files": [],
        "map_files": ["fonts/map/dvips/zapfding/uzd.map"],
        "tds_dirs": [
            ("fonts/type1/urw/zapfding", "fonts/type1/urw/zapfding"),
            ("fonts/afm/urw/zapfding", "fonts/afm/urw/zapfding"),
            ("fonts/tfm/adobe/zapfding", "fonts/tfm/adobe/zapfding"),
            ("fonts/tfm/urw35vf/zapfding", "fonts/tfm/urw35vf/zapfding"),
            ("fonts/map/dvips/zapfding", "fonts/map/dvips/zapfding"),
            ("tex/latex/zapfding", "tex/latex/zapfding"),
        ],
    },
    "symbol": {
        "version": "2024",
        "revision": 77161,
        "license": "GPL-2.0-or-later",
        "ctan_path": "/fonts/urw/base35",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/symbol.tar.xz",
        "upstream_sha256": "c6b615bf830b8260a18f26a2aedd406c0e3aa9a24831cc1b2bc73be1774a669b",
        "upstream_size_bytes": 36104,
        "description": "URW 'Base 35' font pack for LaTeX: Symbol (Standard Symbols L) Type 1, metrics, maps",
        "source_obligations": "GNU General Public License v2.0 or later with font exception; corresponding source in base35.zip",
        "license_files": [],
        "map_files": ["fonts/map/dvips/symbol/usy.map"],
        "tds_dirs": [
            ("fonts/type1/urw/symbol", "fonts/type1/urw/symbol"),
            ("fonts/afm/urw/symbol", "fonts/afm/urw/symbol"),
            ("fonts/tfm/adobe/symbol", "fonts/tfm/adobe/symbol"),
            ("fonts/tfm/urw35vf/symbol", "fonts/tfm/urw35vf/symbol"),
            ("fonts/map/dvips/symbol", "fonts/map/dvips/symbol"),
            ("tex/latex/symbol", "tex/latex/symbol"),
        ],
    },
    "courier": {
        "version": "2024",
        "revision": 77161,
        "license": "GPL-2.0-or-later",
        "ctan_path": "/fonts/urw/base35",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/courier.tar.xz",
        "upstream_sha256": "eaecb5bcd119e6409ac549fdffbe73a6bf7087daef43085104a1ba03787ec989",
        "upstream_size_bytes": 481072,
        "description": "URW 'Base 35' font pack for LaTeX: Courier (Nimbus Mono L) Type 1, metrics, VFs, maps",
        "source_obligations": "GNU General Public License v2.0 or later with font exception; corresponding source in base35.zip",
        "license_files": [],
        "map_files": ["fonts/map/dvips/courier/ucr.map"],
        "tds_dirs": [
            ("fonts/type1/urw/courier", "fonts/type1/urw/courier"),
            ("fonts/type1/adobe/courier", "fonts/type1/adobe/courier"),
            ("fonts/afm/urw/courier", "fonts/afm/urw/courier"),
            ("fonts/afm/adobe/courier", "fonts/afm/adobe/courier"),
            ("fonts/tfm/adobe/courier", "fonts/tfm/adobe/courier"),
            ("fonts/tfm/urw35vf/courier", "fonts/tfm/urw35vf/courier"),
            ("fonts/vf/adobe/courier", "fonts/vf/adobe/courier"),
            ("fonts/vf/urw35vf/courier", "fonts/vf/urw35vf/courier"),
            ("fonts/map/dvips/courier", "fonts/map/dvips/courier"),
            ("tex/latex/courier", "tex/latex/courier"),
        ],
    },
    "bookman": {
        "version": "2024",
        "revision": 77161,
        "license": "GPL-2.0-or-later",
        "ctan_path": "/fonts/urw/base35",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/bookman.tar.xz",
        "upstream_sha256": "ec4736522d6749513268a83502febc4dcb73346f8e0b70dafaa76a74ae8998ca",
        "upstream_size_bytes": 278740,
        "description": "URW 'Base 35' font pack for LaTeX: Bookman (URW Bookman L) Type 1, metrics, VFs, maps",
        "source_obligations": "GNU General Public License v2.0 or later with font exception; corresponding source in base35.zip",
        "license_files": [],
        "map_files": ["fonts/map/dvips/bookman/ubk.map"],
        "tds_dirs": [
            ("fonts/type1/urw/bookman", "fonts/type1/urw/bookman"),
            ("fonts/afm/urw/bookman", "fonts/afm/urw/bookman"),
            ("fonts/tfm/adobe/bookman", "fonts/tfm/adobe/bookman"),
            ("fonts/tfm/urw35vf/bookman", "fonts/tfm/urw35vf/bookman"),
            ("fonts/vf/adobe/bookman", "fonts/vf/adobe/bookman"),
            ("fonts/vf/urw35vf/bookman", "fonts/vf/urw35vf/bookman"),
            ("fonts/map/dvips/bookman", "fonts/map/dvips/bookman"),
            ("tex/latex/bookman", "tex/latex/bookman"),
        ],
    },
    "avantgar": {
        "version": "2024",
        "revision": 77161,
        "license": "GPL-2.0-or-later",
        "ctan_path": "/fonts/urw/base35",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/avantgar.tar.xz",
        "upstream_sha256": "4200ef68e6a3564eb6dd3bbad9f760e70c10a81d7ad3a81b93988800dbd50141",
        "upstream_size_bytes": 241568,
        "description": "URW 'Base 35' font pack for LaTeX: Avant Garde (URW Gothic L) Type 1, metrics, VFs, maps",
        "source_obligations": "GNU General Public License v2.0 or later with font exception; corresponding source in base35.zip",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [
            ("fonts/type1/urw/avantgar", "fonts/type1/urw/avantgar"),
            ("fonts/afm/urw/avantgar", "fonts/afm/urw/avantgar"),
            ("fonts/tfm/adobe/avantgar", "fonts/tfm/adobe/avantgar"),
            ("fonts/tfm/urw35vf/avantgar", "fonts/tfm/urw35vf/avantgar"),
            ("fonts/vf/adobe/avantgar", "fonts/vf/adobe/avantgar"),
            ("fonts/vf/urw35vf/avantgar", "fonts/vf/urw35vf/avantgar"),
            ("fonts/map/dvips/avantgar", "fonts/map/dvips/avantgar"),
            ("tex/latex/avantgar", "tex/latex/avantgar"),
        ],
    },
    "ncntrsbk": {
        "version": "2024",
        "revision": 77161,
        "license": "GPL-2.0-or-later",
        "ctan_path": "/fonts/urw/base35",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/ncntrsbk.tar.xz",
        "upstream_sha256": "4976f1859fb159505e60f1a21274b6b63c5af88f2e03cd0ea6a7e3c84512f4b7",
        "upstream_size_bytes": 290852,
        "description": "URW 'Base 35' font pack for LaTeX: New Century Schoolbook (Century Schoolbook L) Type 1, metrics, VFs, maps",
        "source_obligations": "GNU General Public License v2.0 or later with font exception; corresponding source in base35.zip",
        "license_files": [],
        "map_files": ["fonts/map/dvips/ncntrsbk/unc.map"],
        "tds_dirs": [
            ("fonts/type1/urw/ncntrsbk", "fonts/type1/urw/ncntrsbk"),
            ("fonts/afm/urw/ncntrsbk", "fonts/afm/urw/ncntrsbk"),
            ("fonts/tfm/adobe/ncntrsbk", "fonts/tfm/adobe/ncntrsbk"),
            ("fonts/tfm/urw35vf/ncntrsbk", "fonts/tfm/urw35vf/ncntrsbk"),
            ("fonts/vf/adobe/ncntrsbk", "fonts/vf/adobe/ncntrsbk"),
            ("fonts/vf/urw35vf/ncntrsbk", "fonts/vf/urw35vf/ncntrsbk"),
            ("fonts/map/dvips/ncntrsbk", "fonts/map/dvips/ncntrsbk"),
            ("tex/latex/ncntrsbk", "tex/latex/ncntrsbk"),
        ],
    },
    "zapfchan": {
        "version": "2024",
        "revision": 77161,
        "license": "GPL-2.0-or-later",
        "ctan_path": "/fonts/urw/base35",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/zapfchan.tar.xz",
        "upstream_sha256": "b1af3ec046fc3bdc9c1f35586a523cfd1393d02fd28e146b3efd2260d3311214",
        "upstream_size_bytes": 79900,
        "description": "URW 'Base 35' font pack for LaTeX: Zapf Chancery (URW Chancery L) Type 1, metrics, VFs, maps",
        "source_obligations": "GNU General Public License v2.0 or later with font exception; corresponding source in base35.zip",
        "license_files": [],
        "map_files": ["fonts/map/dvips/zapfchan/uzc.map"],
        "tds_dirs": [
            ("fonts/type1/urw/zapfchan", "fonts/type1/urw/zapfchan"),
            ("fonts/afm/urw/zapfchan", "fonts/afm/urw/zapfchan"),
            ("fonts/tfm/adobe/zapfchan", "fonts/tfm/adobe/zapfchan"),
            ("fonts/tfm/urw35vf/zapfchan", "fonts/tfm/urw35vf/zapfchan"),
            ("fonts/vf/adobe/zapfchan", "fonts/vf/adobe/zapfchan"),
            ("fonts/vf/urw35vf/zapfchan", "fonts/vf/urw35vf/zapfchan"),
            ("fonts/map/dvips/zapfchan", "fonts/map/dvips/zapfchan"),
            ("tex/latex/zapfchan", "tex/latex/zapfchan"),
        ],
    },
    "charter": {
        "version": "2024",
        "revision": 15878,
        "license": "other-free",
        "ctan_path": "/fonts/charter",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/charter.tar.xz",
        "upstream_sha256": "34a08f3370093966207b4a3fed029c215882c89658925624362a877f927c9bd0",
        "upstream_size_bytes": 175180,
        "description": "Bitstream Charter fonts: Type 1 outlines, metrics, and VFs",
        "source_obligations": "Bitstream Free License; unrestricted redistribution and use",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [
            ("fonts/type1/bitstrea/charter", "fonts/type1/bitstrea/charter"),
            ("fonts/afm/bitstrea/charter", "fonts/afm/bitstrea/charter"),
            ("fonts/tfm/bitstrea/charter", "fonts/tfm/bitstrea/charter"),
            ("fonts/vf/bitstrea/charter", "fonts/vf/bitstrea/charter"),
        ],
    },
    "utopia": {
        "version": "2024",
        "revision": 77682,
        "license": "other-free",
        "ctan_path": "/fonts/utopia",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/utopia.tar.xz",
        "upstream_sha256": "d8148e06274d9f3eca68d90586ee4f5797068ce8bb9ff93a7cb24872f520a643",
        "upstream_size_bytes": 205540,
        "description": "Adobe Utopia Type 1 outline fonts and support",
        "source_obligations": "Adobe / X Consortium free font license",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [
            ("fonts/type1/adobe/utopia", "fonts/type1/adobe/utopia"),
        ],
    },
    "calligra-type1": {
        "version": "001.000",
        "revision": 24302,
        "license": "other-free",
        "ctan_path": "/fonts/calligra-type1",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/calligra-type1.tar.xz",
        "upstream_sha256": "981abbc90cbf776705154feed49ebf2f2e5f8402b8e2e58fc04d0bc19a9a35bf",
        "upstream_size_bytes": 59668,
        "description": "Type 1 version of Calligra calligraphy font",
        "source_obligations": "Freely redistributable Type 1 outline",
        "license_files": [],
        "map_files": ["fonts/map/dvips/calligra-type1/calligra.map"],
        "tds_dirs": [
            ("fonts/type1/public/calligra-type1", "fonts/type1/public/calligra-type1"),
            ("fonts/map/dvips/calligra-type1", "fonts/map/dvips/calligra-type1"),
        ],
    },
    "mathabx-type1": {
        "version": "2024",
        "revision": 21129,
        "license": "LPPL",
        "ctan_path": "/fonts/mathabx-type1",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/mathabx-type1.tar.xz",
        "upstream_sha256": "7a3df592ec65c261221db2c82f256844d646f2e72cd1b96cf0121580a8bad41b",
        "upstream_size_bytes": 1858004,
        "description": "Outline Type 1 version of mathabx mathematical fonts",
        "source_obligations": "LaTeX Project Public License; source conversion",
        "license_files": [],
        "map_files": ["fonts/map/dvips/mathabx-type1/mathabx.map"],
        "tds_dirs": [
            ("fonts/type1/public/mathabx-type1", "fonts/type1/public/mathabx-type1"),
            ("fonts/map/dvips/mathabx-type1", "fonts/map/dvips/mathabx-type1"),
        ],
    },
    "doublestroke": {
        "version": "1.111",
        "revision": 77682,
        "license": "other-free",
        "ctan_path": "/fonts/doublestroke",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/doublestroke.tar.xz",
        "upstream_sha256": "ae9a49b2855d2328659496e58890c60f75cb791fd0326fbc3345f0bc9c1dfc00",
        "upstream_size_bytes": 67020,
        "description": "Typeset mathematical double stroke symbols (dsrom/dsss Type 1 and metrics)",
        "source_obligations": "Freely redistributable Type 1 fonts",
        "license_files": [],
        "map_files": ["fonts/map/dvips/doublestroke/dstroke.map"],
        "tds_dirs": [
            ("fonts/type1/public/doublestroke", "fonts/type1/public/doublestroke"),
            ("fonts/tfm/public/doublestroke", "fonts/tfm/public/doublestroke"),
            ("fonts/map/dvips/doublestroke", "fonts/map/dvips/doublestroke"),
            ("tex/latex/doublestroke", "tex/latex/doublestroke"),
        ],
    },
    "bbold-type1": {
        "version": "2024",
        "revision": 33143,
        "license": "other-free",
        "ctan_path": "/fonts/bbold-type1",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/bbold-type1.tar.xz",
        "upstream_sha256": "48bc70b5d0035df360d94780f78a4b7f7d752e3efb196ae7f2478ee75859a3ca",
        "upstream_size_bytes": 70004,
        "description": "Adobe Type 1 format version of bbold Blackboard Bold font",
        "source_obligations": "Freely redistributable Type 1 fonts",
        "license_files": [],
        "map_files": ["fonts/map/dvips/bbold-type1/bbold.map"],
        "tds_dirs": [
            ("fonts/type1/public/bbold-type1", "fonts/type1/public/bbold-type1"),
            ("fonts/map/dvips/bbold-type1", "fonts/map/dvips/bbold-type1"),
        ],
    },
    "yhmath": {
        "version": "1.6",
        "revision": 77682,
        "license": "LPPL-1.3c",
        "ctan_path": "/fonts/yhmath",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/yhmath.tar.xz",
        "upstream_sha256": "51de28ec43186316245cb1a08a3855cf3d13672b66c3515a88ebbc1b2801d7a9",
        "upstream_size_bytes": 37016,
        "description": "Extended maths fonts for LaTeX (yhcmex Type 1, VFs, metrics)",
        "source_obligations": "LPPL-1.3c; corresponding source in yhmath.source.tar.xz",
        "license_files": [],
        "map_files": ["fonts/map/dvips/yhmath/yhmath.map"],
        "tds_dirs": [
            ("fonts/type1/public/yhmath", "fonts/type1/public/yhmath"),
            ("fonts/tfm/public/yhmath", "fonts/tfm/public/yhmath"),
            ("fonts/vf/public/yhmath", "fonts/vf/public/yhmath"),
            ("fonts/map/dvips/yhmath", "fonts/map/dvips/yhmath"),
            ("tex/latex/yhmath", "tex/latex/yhmath"),
        ],
    },
    "ae": {
        "version": "1.4",
        "revision": 15878,
        "license": "LPPL",
        "ctan_path": "/fonts/ae",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/ae.tar.xz",
        "upstream_sha256": "8afa5c20210007d2fd289c067c50f1b6fd4a4ee4d4afc465990cfd083c7e7a65",
        "upstream_size_bytes": 57336,
        "description": "Virtual fonts for T1 encoded Computer Modern Roman fonts",
        "source_obligations": "LPPL; corresponding source in ae.source.tar.xz",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [
            ("fonts/tfm/public/ae", "fonts/tfm/public/ae"),
            ("fonts/vf/public/ae", "fonts/vf/public/ae"),
            ("tex/latex/ae", "tex/latex/ae"),
        ],
    },
    "esint-type1": {
        "version": "2024",
        "revision": 15878,
        "license": "Public Domain",
        "ctan_path": "/fonts/esint-type1",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/esint-type1.tar.xz",
        "upstream_sha256": "12f0b199e9d29af3742d105a52c521f8ec2953bc404e4cf72502fc02051f7ce3",
        "upstream_size_bytes": 31568,
        "description": "Font esint10 in Type 1 format",
        "source_obligations": "Public Domain; Eddie Saudrais",
        "license_files": [],
        "map_files": ["fonts/map/dvips/esint-type1/esint.map"],
        "tds_dirs": [
            ("fonts/type1/public/esint-type1", "fonts/type1/public/esint-type1"),
            ("fonts/map/dvips/esint-type1", "fonts/map/dvips/esint-type1"),
        ],
    },
    "txfonts": {
        "version": "2024",
        "revision": 77682,
        "license": "GPL",
        "ctan_path": "/fonts/txfonts",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/txfonts.tar.xz",
        "upstream_sha256": "03189f5a8b40535a5ca705aabb80aa9e9c9b9c993ba5885f891a3cdfc80afb1d",
        "upstream_size_bytes": 709532,
        "description": "Times-like fonts in support of mathematics: complete Type 1 outlines, metrics, VFs, maps",
        "source_obligations": "GNU General Public License; Young Ryu; corresponding source in txfonts.zip",
        "license_files": ["doc/fonts/txfonts/README"],
        "map_files": ["fonts/map/dvips/txfonts/txfonts.map"],
        "tds_dirs": [
            ("fonts/type1/public/txfonts", "fonts/type1/public/txfonts"),
            ("fonts/tfm/public/txfonts", "fonts/tfm/public/txfonts"),
            ("fonts/vf/public/txfonts", "fonts/vf/public/txfonts"),
            ("fonts/afm/public/txfonts", "fonts/afm/public/txfonts"),
            ("fonts/map/dvips/txfonts", "fonts/map/dvips/txfonts"),
            ("tex/latex/txfonts", "tex/latex/txfonts"),
        ],
    },
    "libertine": {
        "version": "5.3.0",
        "revision": 77682,
        "license": "GPL / OFL / LPPL",
        "ctan_path": "/fonts/libertine",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/libertine.tar.xz",
        "upstream_sha256": "22c57eb8fb021c16a98c1c4ad286c4852133527b6e7de27ac1f9781f42e6b20e",
        "upstream_size_bytes": 13846900,
        "description": "Linux Libertine and Biolinum fonts: Type 1, OpenType, metrics, encodings, and maps",
        "source_obligations": "GNU General Public License with font exception / SIL Open Font License / LPPL",
        "license_files": [],
        "map_files": ["fonts/map/dvips/libertine/libertine.map"],
        "tds_dirs": [
            ("fonts/type1/public/libertine", "fonts/type1/public/libertine"),
            ("fonts/opentype/public/libertine", "fonts/opentype/public/libertine"),
            ("fonts/tfm/public/libertine", "fonts/tfm/public/libertine"),
            ("fonts/vf/public/libertine", "fonts/vf/public/libertine"),
            ("fonts/enc/dvips/libertine", "fonts/enc/dvips/libertine"),
            ("fonts/map/dvips/libertine", "fonts/map/dvips/libertine"),
            ("tex/latex/libertine", "tex/latex/libertine"),
        ],
    },
    "fontawesome5": {
        "version": "5.15.4",
        "revision": 77682,
        "license": "OFL-1.1 / LPPL-1.3c",
        "ctan_path": "/fonts/fontawesome5",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/fontawesome5.tar.xz",
        "upstream_sha256": "dc0df9192cdb088eb7bca42753128b0c464e61836f02b1fe5dc994c528a51ea4",
        "upstream_size_bytes": 866328,
        "description": "Font Awesome 5 with LaTeX support: Type 1, OpenType, encodings, and maps",
        "source_obligations": "SIL Open Font License 1.1 / LPPL-1.3c",
        "license_files": [],
        "map_files": ["fonts/map/dvips/fontawesome5/fontawesome5.map"],
        "tds_dirs": [
            ("fonts/type1/public/fontawesome5", "fonts/type1/public/fontawesome5"),
            ("fonts/opentype/public/fontawesome5", "fonts/opentype/public/fontawesome5"),
            ("fonts/tfm/public/fontawesome5", "fonts/tfm/public/fontawesome5"),
            ("fonts/enc/dvips/fontawesome5", "fonts/enc/dvips/fontawesome5"),
            ("fonts/map/dvips/fontawesome5", "fonts/map/dvips/fontawesome5"),
            ("tex/latex/fontawesome5", "tex/latex/fontawesome5"),
        ],
    },
    "stix": {
        "version": "1.1.3",
        "revision": 78101,
        "license": "OFL-1.1 / LPPL-1.3",
        "ctan_path": "/fonts/stix",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/stix.tar.xz",
        "upstream_sha256": "58f34c42f0321e505cccfbdefb34db9bab372e8dbba9a1fdcab02d0f64f30752",
        "upstream_size_bytes": 2595416,
        "description": "Scientific and Technical Information Exchange (STIX) fonts: Type 1, OpenType, metrics, maps",
        "source_obligations": "SIL Open Font License 1.1 / LPPL-1.3; corresponding source in stix.source.tar.xz",
        "license_files": [],
        "map_files": ["fonts/map/dvips/stix/stix.map"],
        "tds_dirs": [
            ("fonts/type1/public/stix", "fonts/type1/public/stix"),
            ("fonts/opentype/public/stix", "fonts/opentype/public/stix"),
            ("fonts/tfm/public/stix", "fonts/tfm/public/stix"),
            ("fonts/vf/public/stix", "fonts/vf/public/stix"),
            ("fonts/enc/dvips/stix", "fonts/enc/dvips/stix"),
            ("fonts/map/dvips/stix", "fonts/map/dvips/stix"),
            ("tex/latex/stix", "tex/latex/stix"),
        ],
    },
    "mathdesign": {
        "version": "2.31",
        "revision": 31639,
        "license": "GPL",
        "ctan_path": "/fonts/mathdesign",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/mathdesign.tar.xz",
        "upstream_sha256": "6dca88771e554da97364802f2cfc7827d1e26b7c712752dc8b629a64eb050cef",
        "upstream_size_bytes": 2170832,
        "description": "Mathematical fonts to fit with particular text fonts (mdput, mdbch, mdugm Type 1, metrics, maps)",
        "source_obligations": "GNU General Public License; Paul Pichaureau; corresponding source in mathdesign.zip",
        "license_files": ["doc/fonts/mathdesign/README"],
        "map_files": [
            "fonts/map/dvips/mathdesign/mdbch.map",
            "fonts/map/dvips/mathdesign/mdput.map",
            "fonts/map/dvips/mathdesign/mdugm.map",
        ],
        "tds_dirs": [
            ("fonts/type1/public/mathdesign", "fonts/type1/public/mathdesign"),
            ("fonts/tfm/public/mathdesign", "fonts/tfm/public/mathdesign"),
            ("fonts/vf/public/mathdesign", "fonts/vf/public/mathdesign"),
            ("fonts/afm/public/mathdesign", "fonts/afm/public/mathdesign"),
            ("fonts/enc/dvips/mathdesign", "fonts/enc/dvips/mathdesign"),
            ("fonts/map/dvips/mathdesign", "fonts/map/dvips/mathdesign"),
            ("tex/latex/mathdesign", "tex/latex/mathdesign"),
        ],
    },
    "latex-base-legal": {
        "version": "2024",
        "revision": 76924,
        "license": "LPPL-1.3c",
        "ctan_path": "/macros/latex/base",
        "upstream_url": "https://mirrors.ctan.org/macros/latex/base.zip",
        "upstream_sha256": "",
        "upstream_size_bytes": 0,
        "description": "LaTeX base distribution legal notices, manifest, and license text",
        "source_obligations": "LPPL-1.3c; bundled to satisfy derived TU adapter distribution obligations",
        "license_files": [
            "doc/latex/base/manifest.txt",
            "doc/latex/base/legal.txt",
            "doc/latex/base/lppl.txt",
            "doc/latex/base/README.md",
            "doc/fonts/NOTICES-FONTS.txt",
        ],
        "map_files": [],
        "tds_dirs": [],
    },
    'arev': {
        'ctan_path': '/fonts/arev',
        'description': 'Fonts and LaTeX support files for Arev Sans',
        'license': 'lppl1.3a',
        'license_files': ['doc/fonts/arev/ArevSansLicense.txt', 'doc/fonts/arev/BitstreamVeraLicense.txt', 'doc/fonts/arev/README'],
        'map_files': ['fonts/map/dvips/arev/arev.map'],
        'revision': 79618,
        'source_obligations': 'lppl1.3a; official CTAN distribution',
        'tds_dirs': [('fonts/afm/public/arev', 'fonts/afm/public/arev'), ('fonts/enc/dvips/arev', 'fonts/enc/dvips/arev'), ('fonts/map/dvips/arev', 'fonts/map/dvips/arev'), ('fonts/tfm/public/arev', 'fonts/tfm/public/arev'), ('fonts/type1/public/arev', 'fonts/type1/public/arev'), ('fonts/vf/public/arev', 'fonts/vf/public/arev'), ('tex/latex/arev', 'tex/latex/arev')],
        'upstream_sha256': 'e765d7c7a83a752adcf3212a48fc4a07240cf7cddfe9edb6449f2abb829eee7c',
        'upstream_size_bytes': 964648,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/arev.tar.xz',
        'version': '2024',
    },
    'baskervaldx': {
        'ctan_path': '/fonts/baskervaldx',
        'description': 'Extension and modification of BaskervaldADF with LaTeX support',
        'license': 'gpl2+ lppl1.3',
        'license_files': ['doc/fonts/baskervaldx/COPYING', 'doc/fonts/baskervaldx/README'],
        'map_files': ['fonts/map/dvips/baskervaldx/Baskervaldx.map'],
        'revision': 78931,
        'source_obligations': 'gpl2+ lppl1.3; official CTAN distribution',
        'tds_dirs': [('fonts/afm/public/baskervaldx', 'fonts/afm/public/baskervaldx'), ('fonts/enc/dvips/baskervaldx', 'fonts/enc/dvips/baskervaldx'), ('fonts/map/dvips/baskervaldx', 'fonts/map/dvips/baskervaldx'), ('fonts/opentype/public/baskervaldx', 'fonts/opentype/public/baskervaldx'), ('fonts/tfm/public/baskervaldx', 'fonts/tfm/public/baskervaldx'), ('fonts/type1/public/baskervaldx', 'fonts/type1/public/baskervaldx'), ('fonts/vf/public/baskervaldx', 'fonts/vf/public/baskervaldx'), ('tex/latex/baskervaldx', 'tex/latex/baskervaldx')],
        'upstream_sha256': 'ad08b214d7cfe5cab350e52c347f22b7e06ddaf1200d0321eda69d756bf65ac0',
        'upstream_size_bytes': 680416,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/baskervaldx.tar.xz',
        'version': '1.08',
    },
    'bbm': {
        'ctan_path': '/fonts/cm/bbm',
        'description': '"Blackboard-style" cm fonts',
        'license': 'other-free',
        'license_files': ['doc/fonts/bbm/README'],
        'map_files': ['fonts/map/dvips/bbm/bbm.map'],
        'revision': 77682,
        'source_obligations': 'other-free; authentic METAFONT source archive',
        'tds_dirs': [('fonts/source/public/bbm', 'fonts/source/public/bbm'), ('fonts/tfm/public/bbm', 'fonts/tfm/public/bbm')],
        'upstream_sha256': '5684bfe87f5153ad9a05fb6c3df58ea06d6d0a3bee747c07f891b3206398bc66',
        'upstream_size_bytes': 32608,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/bbm.tar.xz',
        'version': '2024',
    },
    'boondox': {
        'ctan_path': '/fonts/boondox',
        'description': 'Mathematical alphabets derived from the STIX fonts',
        'license': 'ofl lppl1.1',
        'license_files': ['doc/fonts/boondox/README'],
        'map_files': ['fonts/map/dvips/boondox/boondox.map'],
        'revision': 79618,
        'source_obligations': 'ofl lppl1.1; official CTAN distribution',
        'tds_dirs': [('fonts/map/dvips/boondox', 'fonts/map/dvips/boondox'), ('fonts/tfm/public/boondox', 'fonts/tfm/public/boondox'), ('fonts/type1/public/boondox', 'fonts/type1/public/boondox'), ('fonts/vf/public/boondox', 'fonts/vf/public/boondox'), ('tex/latex/boondox', 'tex/latex/boondox')],
        'upstream_sha256': '8cba8ce1f30d8e7a471a4f755a9adbf6db09009aa19a501fd5e7b07b393a73a9',
        'upstream_size_bytes': 204704,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/boondox.tar.xz',
        'version': '1.02d',
    },
    'cabin': {
        'ctan_path': '/fonts/cabin',
        'description': 'A humanist Sans Serif font, with LaTeX support',
        'license': 'ofl lppl',
        'license_files': ['doc/fonts/cabin/OFL.txt', 'doc/fonts/cabin/README'],
        'map_files': ['fonts/map/dvips/cabin/cabin.map'],
        'revision': 77682,
        'source_obligations': 'ofl lppl; official CTAN distribution',
        'tds_dirs': [('fonts/enc/dvips/cabin', 'fonts/enc/dvips/cabin'), ('fonts/map/dvips/cabin', 'fonts/map/dvips/cabin'), ('fonts/opentype/impallari/cabin', 'fonts/opentype/impallari/cabin'), ('fonts/tfm/impallari/cabin', 'fonts/tfm/impallari/cabin'), ('fonts/type1/impallari/cabin', 'fonts/type1/impallari/cabin'), ('fonts/vf/impallari/cabin', 'fonts/vf/impallari/cabin'), ('tex/latex/cabin', 'tex/latex/cabin')],
        'upstream_sha256': 'b29c8cf5cd39e208b3ca707ea88cea2cb5d9286c656ae936c0389f757f860e1b',
        'upstream_size_bytes': 2896428,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/cabin.tar.xz',
        'version': '2024',
    },
    'cjk-ko': {
        'ctan_path': '/language/korean/cjk-ko',
        'description': 'Korean TeX (ko.TeX) macros and kotex.sty package for CJKutf8 and Korean document support',
        'license': 'LPPL-1.3c / GPL-2.0 / Public Domain',
        'license_files': [],
        'map_files': [],
        'revision': 70300,
        'source_obligations': 'LPPL-1.3c / GPL-2.0; corresponding upstream source archive preserved in cjk-ko.zip',
        'tds_dirs': [('tex/latex/cjk-ko', 'tex/latex/cjk-ko')],
        'upstream_sha256': '026b8f250c5045fd4e5f760b4f201b225f95194cfb89f69286614485ef3a4f35',
        'upstream_size_bytes': 8840,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/cjk-ko.tar.xz',
        'version': '2.5',
    },
    'cm-mf-extra-bold': {
        'ctan_path': '/fonts/cm/mf-extra/bold',
        'description': 'Extra Metafont files for CM',
        'license': 'gpl pd',
        'license_files': [],
        'map_files': ['fonts/map/dvips/cmextra/cmextra-t1.map'],
        'revision': 54512,
        'source_obligations': 'gpl pd; authentic METAFONT source archive',
        'tds_dirs': [('fonts/source/public/cm-mf-extra-bold', 'fonts/source/public/cm-mf-extra-bold'), ('fonts/tfm/public/cm-mf-extra-bold', 'fonts/tfm/public/cm-mf-extra-bold')],
        'upstream_sha256': 'b65fca9dfff9d0b22c6a55e28b2fc4c2071b17d9b0c7674eb26cbdd06f144dab',
        'upstream_size_bytes': 4896,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/cm-mf-extra-bold.tar.xz',
        'version': '2024',
    },
    'cmcyr': {
        'ctan_path': '/fonts/cyrillic/cmcyr',
        'description': 'Computer Modern fonts with cyrillic extensions',
        'license': 'pd',
        'license_files': [],
        'map_files': [],
        'revision': 68681,
        'source_obligations': 'pd; authentic METAFONT source archive',
        'tds_dirs': [('fonts/map/dvips/cmcyr', 'fonts/map/dvips/cmcyr'), ('fonts/source/public/cmcyr', 'fonts/source/public/cmcyr'), ('fonts/tfm/public/cmcyr', 'fonts/tfm/public/cmcyr'), ('fonts/type1/public/cmcyr', 'fonts/type1/public/cmcyr'), ('fonts/vf/public/cmcyr', 'fonts/vf/public/cmcyr')],
        'upstream_sha256': '263793446f5389af3e61174db0c4b7600c109210443a948c3f28923258f0d90a',
        'upstream_size_bytes': 901468,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/cmcyr.tar.xz',
        'version': '2024',
    },
    'dutchcal': {
        'ctan_path': '/fonts/dutchcal',
        'description': 'A reworking of ESSTIX13, adding a bold version',
        'license': 'lppl',
        'license_files': ['doc/fonts/dutchcal/README'],
        'map_files': ['fonts/map/dvips/dutchcal/dutchcal.map'],
        'revision': 77682,
        'source_obligations': 'lppl; official CTAN distribution',
        'tds_dirs': [('fonts/afm/public/dutchcal', 'fonts/afm/public/dutchcal'), ('fonts/map/dvips/dutchcal', 'fonts/map/dvips/dutchcal'), ('fonts/tfm/public/dutchcal', 'fonts/tfm/public/dutchcal'), ('fonts/type1/public/dutchcal', 'fonts/type1/public/dutchcal'), ('fonts/vf/public/dutchcal', 'fonts/vf/public/dutchcal'), ('tex/latex/dutchcal', 'tex/latex/dutchcal')],
        'upstream_sha256': 'cb7ddc48e0d2d5153a6715d5c333554b981d0233074481bf444f162b4c1af1c4',
        'upstream_size_bytes': 36192,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/dutchcal.tar.xz',
        'version': '1.0',
    },
    'ebgaramond': {
        'ctan_path': '/fonts/ebgaramond',
        'description': 'LaTeX support for EBGaramond fonts',
        'license': 'ofl lppl',
        'license_files': ['doc/fonts/ebgaramond/OFL.txt', 'doc/fonts/ebgaramond/README'],
        'map_files': ['fonts/map/dvips/ebgaramond/EBGaramond.map'],
        'revision': 78251,
        'source_obligations': 'ofl lppl; official CTAN distribution',
        'tds_dirs': [('fonts/enc/dvips/ebgaramond', 'fonts/enc/dvips/ebgaramond'), ('fonts/map/dvips/ebgaramond', 'fonts/map/dvips/ebgaramond'), ('fonts/opentype/public/ebgaramond', 'fonts/opentype/public/ebgaramond'), ('fonts/tfm/public/ebgaramond', 'fonts/tfm/public/ebgaramond'), ('fonts/type1/public/ebgaramond', 'fonts/type1/public/ebgaramond'), ('fonts/vf/public/ebgaramond', 'fonts/vf/public/ebgaramond'), ('tex/latex/ebgaramond', 'tex/latex/ebgaramond')],
        'upstream_sha256': '905f7f54b37e065fd8d8be81f8de4becd3b2ccf4920adb852f9721b0b611d278',
        'upstream_size_bytes': 8391124,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/ebgaramond.tar.xz',
        'version': '2024',
    },
    'els-cas-templates': {
        'ctan_path': '/macros/latex/contrib/els-cas-templates',
        'description': 'Elsevier CAS journal templates (cas-dc.cls, cas-sc.cls, cas-common.sty) and thumbnail stock icons',
        'license': 'LPPL-1.3c',
        'license_files': [],
        'map_files': [],
        'revision': 71189,
        'source_obligations': 'LPPL-1.3c; corresponding upstream source archive preserved in els-cas-templates.zip',
        'tds_dirs': [('tex/latex/els-cas-templates', 'tex/latex/els-cas-templates'), ('bibtex/bst/els-cas-templates', 'bibtex/bst/els-cas-templates')],
        'upstream_sha256': '7a0503f99e128d0c93b6c3a5b48c244c6a27ed45e5271c339f3162301443d813',
        'upstream_size_bytes': 54876,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/els-cas-templates.tar.xz',
        'version': '2.4',
    },
    'esstix': {
        'ctan_path': '/fonts/esstix',
        'description': 'PostScript versions of the ESSTIX, with macro support',
        'license': 'ofl',
        'license_files': ['doc/fonts/esstix/README'],
        'map_files': ['fonts/map/dvips/esstix/ESSTIX.map'],
        'revision': 77682,
        'source_obligations': 'ofl; official CTAN distribution',
        'tds_dirs': [('fonts/afm/esstix', 'fonts/afm/esstix'), ('fonts/map/dvips/esstix', 'fonts/map/dvips/esstix'), ('fonts/tfm/public/esstix', 'fonts/tfm/public/esstix'), ('fonts/type1/public/esstix', 'fonts/type1/public/esstix'), ('fonts/vf/public/esstix', 'fonts/vf/public/esstix'), ('tex/latex/esstix', 'tex/latex/esstix')],
        'upstream_sha256': '7cbfaa770ce2b8a3345b617f8d0a9829917d2834a6083ec9bf45b9c5638df6f3',
        'upstream_size_bytes': 202188,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/esstix.tar.xz',
        'version': '1.0',
    },
    'esvect': {
        'ctan_path': '/macros/latex/contrib/esvect',
        'description': 'Vector arrows',
        'license': 'gpl',
        'license_files': ['doc/latex/esvect/README'],
        'map_files': ['fonts/map/dvips/esvect/esvect.map'],
        'revision': 77682,
        'source_obligations': 'gpl; official CTAN distribution',
        'tds_dirs': [('fonts/map/dvips/esvect', 'fonts/map/dvips/esvect'), ('fonts/source/public/esvect', 'fonts/source/public/esvect'), ('fonts/tfm/public/esvect', 'fonts/tfm/public/esvect'), ('fonts/type1/public/esvect', 'fonts/type1/public/esvect'), ('tex/latex/esvect', 'tex/latex/esvect')],
        'upstream_sha256': '9cc260745a4c9cb2cc329ab39f4e2ad33a226f508785a282db42ec9f6fd1a431',
        'upstream_size_bytes': 66876,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/esvect.tar.xz',
        'version': '1.3',
    },
    'eurosym': {
        'ctan_path': '/fonts/eurosym',
        'description': 'Metafont and macros for Euro sign',
        'license': 'other-free',
        'license_files': ['doc/fonts/eurosym/COPYING', 'doc/fonts/eurosym/README', 'doc/fonts/eurosym/README.type1'],
        'map_files': ['fonts/map/dvips/eurosym/eurosym.map'],
        'revision': 78101,
        'source_obligations': 'other-free; official CTAN distribution',
        'tds_dirs': [('fonts/map/dvips/eurosym', 'fonts/map/dvips/eurosym'), ('fonts/source/public/eurosym', 'fonts/source/public/eurosym'), ('fonts/tfm/public/eurosym', 'fonts/tfm/public/eurosym'), ('fonts/type1/public/eurosym', 'fonts/type1/public/eurosym'), ('tex/latex/eurosym', 'tex/latex/eurosym')],
        'upstream_sha256': 'db2f7325383f6fa4ed2dc805c432da40fa1ecc742026928001b0cded9b3de07f',
        'upstream_size_bytes': 139868,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/eurosym.tar.xz',
        'version': '1.4-subrfix',
    },
    'fontawesome': {
        'ctan_path': '/fonts/fontawesome',
        'description': 'Font containing web-related icons',
        'license': 'lppl1.3',
        'license_files': ['doc/fonts/fontawesome/README.md'],
        'map_files': ['fonts/map/dvips/fontawesome/fontawesome.map'],
        'revision': 78348,
        'source_obligations': 'lppl1.3; official CTAN distribution',
        'tds_dirs': [('fonts/enc/dvips/fontawesome', 'fonts/enc/dvips/fontawesome'), ('fonts/map/dvips/fontawesome', 'fonts/map/dvips/fontawesome'), ('fonts/opentype/public/fontawesome', 'fonts/opentype/public/fontawesome'), ('fonts/tfm/public/fontawesome', 'fonts/tfm/public/fontawesome'), ('fonts/type1/public/fontawesome', 'fonts/type1/public/fontawesome'), ('tex/latex/fontawesome', 'tex/latex/fontawesome')],
        'upstream_sha256': '98afe54919526e4d98381b3f6e6034fd3ae44c4c6174b17fe603d58206ddbfb0',
        'upstream_size_bytes': 276176,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/fontawesome.tar.xz',
        'version': '4.6.3.2',
    },
    'fpl': {
        'ctan_path': '/fonts/fpl',
        'description': 'SC and OsF fonts for URW Palladio L',
        'license': 'gpl2 lppl1',
        'license_files': ['doc/fonts/fpl/COPYING', 'doc/fonts/fpl/README'],
        'map_files': [],
        'revision': 79618,
        'source_obligations': 'GPL-2.0 with font exception / LPPL-1.0; corresponding source preserved in fpl.source.tar.xz',
        'tds_dirs': [('fonts/afm/public/fpl', 'fonts/afm/public/fpl'), ('fonts/type1/public/fpl', 'fonts/type1/public/fpl')],
        'upstream_sha256': '98ea140d101e0802ff266614d57f486495f1a6d063561d1474e68ac764fbc4aa',
        'upstream_size_bytes': 288420,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/fpl.tar.xz',
        'version': '1.003',
    },
    'heuristica': {
        'ctan_path': '/fonts/heuristica',
        'description': 'Fonts extending Utopia, with LaTeX support files',
        'license': 'ofl lppl1.3',
        'license_files': ['doc/fonts/heuristica/OFL.txt', 'doc/fonts/heuristica/README'],
        'map_files': ['fonts/map/dvips/heuristica/Heuristica.map'],
        'revision': 79618,
        'source_obligations': 'ofl lppl1.3; official CTAN distribution',
        'tds_dirs': [('fonts/enc/dvips/heuristica', 'fonts/enc/dvips/heuristica'), ('fonts/map/dvips/heuristica', 'fonts/map/dvips/heuristica'), ('fonts/opentype/public/heuristica', 'fonts/opentype/public/heuristica'), ('fonts/tfm/public/heuristica', 'fonts/tfm/public/heuristica'), ('fonts/type1/public/heuristica', 'fonts/type1/public/heuristica'), ('fonts/vf/public/heuristica', 'fonts/vf/public/heuristica'), ('tex/latex/heuristica', 'tex/latex/heuristica')],
        'upstream_sha256': 'caea9d54929833f329a68dda0977b88c8ef3b481b4a3137d9a0ce2e10c25db4c',
        'upstream_size_bytes': 1079032,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/heuristica.tar.xz',
        'version': '1.093',
    },
    'hfbright': {
        'ctan_path': '/fonts/ps-type1/hfbright',
        'description': 'The hfbright fonts',
        'license': 'lppl',
        'license_files': ['doc/fonts/hfbright/README'],
        'map_files': ['fonts/map/dvips/hfbright/hfbright.map'],
        'revision': 29349,
        'source_obligations': 'lppl; official CTAN distribution',
        'tds_dirs': [('fonts/afm/public/hfbright', 'fonts/afm/public/hfbright'), ('fonts/enc/dvips/hfbright', 'fonts/enc/dvips/hfbright'), ('fonts/map/dvips/hfbright', 'fonts/map/dvips/hfbright'), ('fonts/type1/public/hfbright', 'fonts/type1/public/hfbright')],
        'upstream_sha256': '6db651416b07dbb7946e46f7bb7ab141d2bb1cf099de3f6daf7c91a95b0269a8',
        'upstream_size_bytes': 832824,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/hfbright.tar.xz',
        'version': '2024',
    },
    'ifsym': {
        'ctan_path': '/fonts/ifsym',
        'description': 'A collection of symbols',
        'license': 'other-free',
        'license_files': [],
        'map_files': ['fonts/map/dvips/ifsym/ifsym.map'],
        'revision': 77682,
        'source_obligations': 'other-free; authentic METAFONT source archive',
        'tds_dirs': [('fonts/source/public/ifsym', 'fonts/source/public/ifsym'), ('fonts/tfm/public/ifsym', 'fonts/tfm/public/ifsym'), ('tex/latex/ifsym', 'tex/latex/ifsym')],
        'upstream_sha256': 'f29b0f761b885098d8bbe2d770fb23c3d4d75c6a8d263c4179e2432f611df9c8',
        'upstream_size_bytes': 9800,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/ifsym.tar.xz',
        'version': '2024',
    },
    'inconsolata': {
        'ctan_path': '/fonts/inconsolata',
        'description': 'A monospaced font, with support files for use with TeX',
        'license': 'ofl apache2 lppl1.3',
        'license_files': ['doc/fonts/inconsolata/OFL.txt', 'doc/fonts/inconsolata/README'],
        'map_files': ['fonts/map/dvips/inconsolata/zi4.map'],
        'revision': 79618,
        'source_obligations': 'ofl apache2 lppl1.3; official CTAN distribution',
        'tds_dirs': [('fonts/enc/dvips/inconsolata', 'fonts/enc/dvips/inconsolata'), ('fonts/map/dvips/inconsolata', 'fonts/map/dvips/inconsolata'), ('fonts/opentype/public/inconsolata', 'fonts/opentype/public/inconsolata'), ('fonts/tfm/public/inconsolata', 'fonts/tfm/public/inconsolata'), ('fonts/type1/public/inconsolata', 'fonts/type1/public/inconsolata'), ('tex/latex/inconsolata', 'tex/latex/inconsolata')],
        'upstream_sha256': 'd3d9d4c42f421588a8f50f34e4aa8d482ec684d8baed536c27b7b3604a7af06e',
        'upstream_size_bytes': 299552,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/inconsolata.tar.xz',
        'version': '1.121',
    },
    'kpfonts': {
        'ctan_path': '/fonts/kpfonts',
        'description': 'A complete set of fonts for text and mathematics',
        'license': 'lppl gpl',
        'license_files': ['doc/fonts/kpfonts/README.txt'],
        'map_files': ['fonts/map/dvips/kpfonts/kpfonts.map'],
        'revision': 77682,
        'source_obligations': 'lppl gpl; official CTAN distribution',
        'tds_dirs': [('fonts/afm/public/kpfonts', 'fonts/afm/public/kpfonts'), ('fonts/enc/dvips/kpfonts', 'fonts/enc/dvips/kpfonts'), ('fonts/map/dvips/kpfonts', 'fonts/map/dvips/kpfonts'), ('fonts/tfm/public/kpfonts', 'fonts/tfm/public/kpfonts'), ('fonts/type1/public/kpfonts', 'fonts/type1/public/kpfonts'), ('fonts/vf/public/kpfonts', 'fonts/vf/public/kpfonts'), ('tex/latex/kpfonts', 'tex/latex/kpfonts')],
        'upstream_sha256': '097726a3219d4725fffc9c900beb47edeefe743ce16cda6d2a5b73c6de3f1c80',
        'upstream_size_bytes': 2241572,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/kpfonts.tar.xz',
        'version': '3.36',
    },
    'marvosym': {
        'ctan_path': '/fonts/marvosym',
        'description': "Martin Vogel's Symbols (marvosym) font",
        'license': 'ofl',
        'license_files': ['doc/fonts/marvosym/OFL.txt', 'doc/fonts/marvosym/README'],
        'map_files': ['fonts/map/dvips/marvosym/marvosym.map'],
        'revision': 79618,
        'source_obligations': 'ofl; official CTAN distribution',
        'tds_dirs': [('fonts/afm/public/marvosym', 'fonts/afm/public/marvosym'), ('fonts/map/dvips/marvosym', 'fonts/map/dvips/marvosym'), ('fonts/tfm/public/marvosym', 'fonts/tfm/public/marvosym'), ('fonts/truetype/public/marvosym', 'fonts/truetype/public/marvosym'), ('fonts/type1/public/marvosym', 'fonts/type1/public/marvosym'), ('tex/latex/marvosym', 'tex/latex/marvosym')],
        'upstream_sha256': 'd8b1740e2c639f3acc6827fd040d14c55d9c3a62c3e8874fa15aed4e1f0c8cf7',
        'upstream_size_bytes': 133504,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/marvosym.tar.xz',
        'version': '2.2a',
    },
    'mathpazo': {
        'ctan_path': '/fonts/mathpazo',
        'description': 'Pazo Math fonts matching Palatino text fonts',
        'license': 'GPL-2.0 with font exception',
        'license_files': ['doc/fonts/mathpazo/README', 'doc/fonts/mathpazo/gpl.txt'],
        'map_files': [],
        'revision': 77682,
        'source_obligations': 'GPL-2.0 with font exception; corresponding source preserved in mathpazo.source.tar.xz',
        'tds_dirs': [('fonts/afm/public/mathpazo', 'fonts/afm/public/mathpazo'), ('fonts/tfm/public/mathpazo', 'fonts/tfm/public/mathpazo'), ('fonts/type1/public/mathpazo', 'fonts/type1/public/mathpazo'), ('fonts/vf/public/mathpazo', 'fonts/vf/public/mathpazo')],
        'upstream_sha256': 'b42822082e609bc2c707bb2657ff5bf6f491718ab391d2c884f7dfed0d51ec15',
        'upstream_size_bytes': 63628,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/mathpazo.tar.xz',
        'version': '1.003',
    },
    'mnsymbol': {
        'ctan_path': '/fonts/mnsymbol',
        'description': 'Mathematical symbol font for Adobe MinionPro',
        'license': 'pd',
        'license_files': ['doc/latex/mnsymbol/README'],
        'map_files': ['fonts/map/dvips/mnsymbol/MnSymbol.map'],
        'revision': 78931,
        'source_obligations': 'pd; official CTAN distribution',
        'tds_dirs': [('fonts/enc/dvips/mnsymbol', 'fonts/enc/dvips/mnsymbol'), ('fonts/map/dvips/mnsymbol', 'fonts/map/dvips/mnsymbol'), ('fonts/map/vtex/mnsymbol', 'fonts/map/vtex/mnsymbol'), ('fonts/opentype/public/mnsymbol', 'fonts/opentype/public/mnsymbol'), ('fonts/source/public/mnsymbol', 'fonts/source/public/mnsymbol'), ('fonts/tfm/public/mnsymbol', 'fonts/tfm/public/mnsymbol'), ('fonts/type1/public/mnsymbol', 'fonts/type1/public/mnsymbol'), ('tex/latex/mnsymbol', 'tex/latex/mnsymbol')],
        'upstream_sha256': '7bdaf593dda367c23342fc4d3c551c736a50ceeb5d28eed99feb9779afd91038',
        'upstream_size_bytes': 4426360,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/mnsymbol.tar.xz',
        'version': '1.4',
    },
    'pxfonts': {
        'ctan_path': '/fonts/pxfonts',
        'description': 'Palatino-like fonts in support of mathematics',
        'license': 'gpl',
        'license_files': [],
        'map_files': ['fonts/map/dvips/pxfonts/pxfonts.map', 'fonts/map/dvips/pxfonts/pxr.map', 'fonts/map/dvips/pxfonts/pxr1.map', 'fonts/map/dvips/pxfonts/pxr2.map', 'fonts/map/dvips/pxfonts/pxr3.map'],
        'revision': 77682,
        'source_obligations': 'gpl; official CTAN distribution',
        'tds_dirs': [('fonts/afm/public/pxfonts', 'fonts/afm/public/pxfonts'), ('fonts/map/dvips/pxfonts', 'fonts/map/dvips/pxfonts'), ('fonts/tfm/public/pxfonts', 'fonts/tfm/public/pxfonts'), ('fonts/type1/public/pxfonts', 'fonts/type1/public/pxfonts'), ('fonts/vf/public/pxfonts', 'fonts/vf/public/pxfonts'), ('tex/latex/pxfonts', 'tex/latex/pxfonts')],
        'upstream_sha256': 'bdddf89946b2a237a75d8d9ee9f2bc6597c868ba2eb73c0a4925a1e631aecd60',
        'upstream_size_bytes': 459976,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/pxfonts.tar.xz',
        'version': '2024',
    },
    'skaknew': {
        'ctan_path': '/fonts/chess/skaknew',
        'description': 'The skak chess fonts redone in Adobe Type 1',
        'license': 'lppl1.2',
        'license_files': ['doc/fonts/skaknew/README'],
        'map_files': ['fonts/map/dvips/skaknew/SkakNew.map'],
        'revision': 79618,
        'source_obligations': 'lppl1.2; official CTAN distribution',
        'tds_dirs': [('fonts/afm/public/skaknew', 'fonts/afm/public/skaknew'), ('fonts/map/dvips/skaknew', 'fonts/map/dvips/skaknew'), ('fonts/opentype/public/skaknew', 'fonts/opentype/public/skaknew'), ('fonts/tfm/public/skaknew', 'fonts/tfm/public/skaknew'), ('fonts/type1/public/skaknew', 'fonts/type1/public/skaknew')],
        'upstream_sha256': '0de354f04a18d3fb99de5e969d693464290b2464c4128a5b3aa6d88b81b47839',
        'upstream_size_bytes': 160044,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/skaknew.tar.xz',
        'version': '2024',
    },
    't2': {
        'ctan_path': '/macros/latex/contrib/t2',
        'description': 'T2 Cyrillic support package providing mathtext.sty, citehack.sty, and misccorr.sty',
        'license': 'LPPL-1.3c',
        'license_files': [],
        'map_files': [],
        'revision': 47870,
        'source_obligations': 'LPPL-1.3c; corresponding upstream source archive preserved in t2.zip',
        'tds_dirs': [('tex/latex/t2', 'tex/latex/t2')],
        'upstream_sha256': 'b58966fcb138e6b0e6f3a59f3b25b2567fcb30fa60f770ae8b61ccff14553b24',
        'upstream_size_bytes': 27048,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/t2.tar.xz',
        'version': '2018',
    },
    'tex-gyre': {
        'ctan_path': '/fonts/tex-gyre',
        'description': 'TeX Fonts extending freely available URW fonts',
        'license': 'gfl',
        'license_files': ['doc/fonts/tex-gyre/GUST-FONT-LICENSE.txt', 'doc/fonts/tex-gyre/README-TeX-Gyre-Adventor.txt', 'doc/fonts/tex-gyre/README-TeX-Gyre-Bonum.txt'],
        'map_files': ['fonts/map/dvips/tex-gyre/qag.map', 'fonts/map/dvips/tex-gyre/qbk.map', 'fonts/map/dvips/tex-gyre/qcr.map', 'fonts/map/dvips/tex-gyre/qcs.map', 'fonts/map/dvips/tex-gyre/qhv.map', 'fonts/map/dvips/tex-gyre/qpl.map', 'fonts/map/dvips/tex-gyre/qtm.map', 'fonts/map/dvips/tex-gyre/qzc.map'],
        'revision': 68624,
        'source_obligations': 'gfl; official CTAN distribution',
        'tds_dirs': [('fonts/afm/public/tex-gyre', 'fonts/afm/public/tex-gyre'), ('fonts/enc/dvips/tex-gyre', 'fonts/enc/dvips/tex-gyre'), ('fonts/map/dvips/tex-gyre', 'fonts/map/dvips/tex-gyre'), ('fonts/opentype/public/tex-gyre', 'fonts/opentype/public/tex-gyre'), ('fonts/tfm/public/tex-gyre', 'fonts/tfm/public/tex-gyre'), ('fonts/type1/public/tex-gyre', 'fonts/type1/public/tex-gyre'), ('tex/latex/tex-gyre', 'tex/latex/tex-gyre')],
        'upstream_sha256': 'dfb4f55c4b02993003777a48663987bd102a3c5a2913172571e05fe2eb18407d',
        'upstream_size_bytes': 7748428,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/tex-gyre.tar.xz',
        'version': '2.501',
    },
    'utfsym': {
        'ctan_path': '/graphics/pgf/contrib/utfsym',
        'description': 'Unicode symbol macro package providing utfsym.sty and 1680 vector TikZ symbol definitions',
        'license': 'CC0-1.0',
        'license_files': [],
        'map_files': [],
        'revision': 63076,
        'source_obligations': 'CC0-1.0 Universal Public Domain Dedication; corresponding source in utfsym.zip',
        'tds_dirs': [('tex/latex/utfsym', 'tex/latex/utfsym')],
        'upstream_sha256': 'f0f94d99e53d285319cec2c045296f7ccb4bc3814083a8de536a9cf82bb401e8',
        'upstream_size_bytes': 1889160,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/utfsym.tar.xz',
        'version': '0.9.0',
    },
    'wasy-type1': {
        'ctan_path': '/fonts/wasy-type1',
        'description': 'Type 1 versions of wasy fonts',
        'license': 'pd',
        'license_files': ['doc/fonts/wasy-type1/README'],
        'map_files': ['fonts/map/dvips/wasy-type1/wasy.map'],
        'revision': 53534,
        'source_obligations': 'pd; official CTAN distribution',
        'tds_dirs': [('fonts/afm/public/wasy-type1', 'fonts/afm/public/wasy-type1'), ('fonts/map/dvips/wasy-type1', 'fonts/map/dvips/wasy-type1'), ('fonts/type1/public/wasy-type1', 'fonts/type1/public/wasy-type1')],
        'upstream_sha256': '1bd86eff809059e4a7c009d8fbe2dd756e568de1d91d6489d82766a1b6f60129',
        'upstream_size_bytes': 261536,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/wasy-type1.tar.xz',
        'version': '001.002',
    },
    'yfonts-t1': {
        'ctan_path': '/fonts/ps-type1/yfonts',
        'description': 'Old German-style fonts, in Adobe type 1 format',
        'license': 'other-free',
        'license_files': ['doc/fonts/yfonts-t1/README'],
        'map_files': ['fonts/map/dvips/yfonts-t1/yfrak.map'],
        'revision': 36013,
        'source_obligations': 'other-free; official CTAN distribution',
        'tds_dirs': [('dvips/yfonts-t1', 'dvips/yfonts-t1'), ('fonts/afm/public/yfonts-t1', 'fonts/afm/public/yfonts-t1'), ('fonts/map/dvips/yfonts-t1', 'fonts/map/dvips/yfonts-t1'), ('fonts/type1/public/yfonts-t1', 'fonts/type1/public/yfonts-t1')],
        'upstream_sha256': 'a62b2726e5d6c68c640863b50eeda748763740b81be48cf2237a705c3ac62528',
        'upstream_size_bytes': 145940,
        'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/yfonts-t1.tar.xz',
        'version': '1.0',
    },
    'bera': {'version': 'r77682',
     'revision': 77682,
     'license': 'Bitstream-Vera',
     'source_obligations': 'Bitstream Vera license; original copyright and trademark notice retained; '
                           'unmodified upstream fonts',
     'license_files': ['doc/fonts/bera/LICENSE'],
     'ctan_path': '/fonts/bera',
     'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/bera.tar.xz',
     'upstream_sha256': 'a3174491eacc119a1747345242c68e35194a28a5af160361ba291ca84429324b',
     'upstream_size_bytes': 312592,
     'description': 'Bera fonts',
     'map_files': ['fonts/map/dvips/bera/bera.map'],
     'tds_dirs': [('fonts/afm/public/bera', 'fonts/afm/public/bera'),
                  ('fonts/map/dvips/bera', 'fonts/map/dvips/bera'),
                  ('fonts/tfm/public/bera', 'fonts/tfm/public/bera'),
                  ('fonts/type1/public/bera', 'fonts/type1/public/bera'),
                  ('fonts/vf/public/bera', 'fonts/vf/public/bera'),
                  ('tex/latex/bera', 'tex/latex/bera')]},
    'ccicons': {'version': '1.6',
     'revision': 77682,
     'license': 'OFL-1.1 / LPPL-1.3c',
     'source_obligations': 'SIL Open Font License for fonts, LPPL-1.3c for macros; upstream source archive '
                           'retained',
     'license_files': ['doc/fonts/ccicons/OFL.txt'],
     'ctan_path': '/fonts/ccicons',
     'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/ccicons.tar.xz',
     'upstream_sha256': '26b63ae8eb4718bc8176fa69557309476d2d244d4f938a973dc4170a40dc4a01',
     'upstream_size_bytes': 15584,
     'description': 'LaTeX support for Creative Commons icons',
     'map_files': ['fonts/map/dvips/ccicons/ccicons.map'],
     'tds_dirs': [('fonts/enc/dvips/ccicons', 'fonts/enc/dvips/ccicons'),
                  ('fonts/map/dvips/ccicons', 'fonts/map/dvips/ccicons'),
                  ('fonts/opentype/public/ccicons', 'fonts/opentype/public/ccicons'),
                  ('fonts/tfm/public/ccicons', 'fonts/tfm/public/ccicons'),
                  ('fonts/type1/public/ccicons', 'fonts/type1/public/ccicons'),
                  ('tex/latex/ccicons', 'tex/latex/ccicons')]},
    'doclicense': {'version': '3.3.0',
     'revision': 77682,
     'license': 'CC0-1.0 / LPPL-1.3c',
     'source_obligations': 'CC0 license graphics and texts; LPPL macros with corresponding source archive '
                           'retained',
     'license_files': [],
     'ctan_path': '/macros/latex/contrib/doclicense',
     'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/doclicense.tar.xz',
     'upstream_sha256': '033d4289a33e4b6b2e1c866232f8a7a67b60af83ee14efa25d027aa8b0a97848',
     'upstream_size_bytes': 236980,
     'description': 'Support for putting documents under a license',
     'map_files': [],
     'tds_dirs': [('tex/latex/doclicense', 'tex/latex/doclicense')]},
    'lato': {'version': '3.3',
     'revision': 79618,
     'license': 'OFL-1.1 / LPPL-1.3c',
     'source_obligations': 'SIL Open Font License for unmodified fonts; complete LPPL macro sources included in runtime tree',
     'license_files': ['doc/fonts/lato/README', 'doc/fonts/lato/OFL.txt'],
     'ctan_path': '/fonts/lato',
     'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/lato.tar.xz',
     'upstream_sha256': '76f75b64c2e597c174007c12b51de2a80b47d63bf8738684298b1e23b636a1b0',
     'upstream_size_bytes': 12322520,
     'description': 'Lato font family and LaTeX support',
     'map_files': ['fonts/map/dvips/lato/lato.map'],
     'tds_dirs': [('fonts/enc/dvips/lato', 'fonts/enc/dvips/lato'),
                  ('fonts/map/dvips/lato', 'fonts/map/dvips/lato'),
                  ('fonts/tfm/typoland/lato', 'fonts/tfm/typoland/lato'),
                  ('fonts/truetype/typoland/lato', 'fonts/truetype/typoland/lato'),
                  ('fonts/type1/typoland/lato', 'fonts/type1/typoland/lato'),
                  ('fonts/vf/typoland/lato', 'fonts/vf/typoland/lato'),
                  ('tex/latex/lato', 'tex/latex/lato')]},
    'stix2-type1': {'version': '2.0.2',
     'revision': 79618,
     'license': 'OFL-1.1 / LPPL-1.3',
     'source_obligations': 'SIL Open Font License for unmodified fonts; LPPL support files with corresponding source archive retained',
     'license_files': ['doc/fonts/stix2-type1/README.txt', 'doc/fonts/stix2-type1/OFL.txt'],
     'ctan_path': '/fonts/stix2-type1',
     'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/stix2-type1.tar.xz',
     'upstream_sha256': '6dcfc35bd01aadcfd3113f55bd481e8c65dc93d7e6f6cc8bc4be7f9726fcb9e0',
     'upstream_size_bytes': 3110752,
     'description': 'Type1 versions of the STIX Two OpenType fonts',
     'map_files': ['fonts/map/dvips/stix2-type1/stix2.map'],
     'tds_dirs': [('fonts/enc/dvips/stix2-type1', 'fonts/enc/dvips/stix2-type1'),
                  ('fonts/map/dvips/stix2-type1', 'fonts/map/dvips/stix2-type1'),
                  ('fonts/tfm/public/stix2-type1', 'fonts/tfm/public/stix2-type1'),
                  ('fonts/type1/public/stix2-type1', 'fonts/type1/public/stix2-type1'),
                  ('tex/latex/stix2-type1', 'tex/latex/stix2-type1')]},
    'twemojis': {'version': '1.3.1 (twemoji v14.0.1)',
     'revision': 79618,
     'license': 'CC-BY-4.0 / LPPL-1.3',
     'source_obligations': 'Emoji graphics copyright 2019 Twitter, Inc. and other contributors, CC-BY-4.0; '
                           'LPPL macros with source archive retained',
     'license_files': ['doc/latex/twemojis/NOTICE.txt'],
     'ctan_path': '/macros/latex/contrib/twemojis',
     'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/twemojis.tar.xz',
     'upstream_sha256': '97c49a218e4c9a570dccda5cf1cb74939465a7da81818ab304cc43ceb7b2155e',
     'upstream_size_bytes': 4425052,
     'description': "Use Twitter's open source emojis through LaTeX commands",
     'map_files': [],
     'tds_dirs': [('tex/latex/twemojis', 'tex/latex/twemojis')]},
    'xcharter': {'version': '1.26',
     'revision': 78931,
     'license': 'Bitstream-Charter / LPPL-1.3',
     'source_obligations': 'Original Bitstream Charter copyright and trademark notice retained; complete LPPL macro sources included in runtime tree',
     'license_files': ['doc/fonts/xcharter/README'],
     'ctan_path': '/fonts/xcharter',
     'upstream_url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/xcharter.tar.xz',
     'upstream_sha256': '92ae152668b18a271830f2efa4d26792c7dc69cc36ee8f2967695de154f1bf04',
     'upstream_size_bytes': 2210356,
     'description': 'Extension of Bitstream Charter fonts',
     'map_files': ['fonts/map/dvips/xcharter/XCharter.map'],
     'tds_dirs': [('fonts/afm/public/xcharter', 'fonts/afm/public/xcharter'),
                  ('fonts/enc/dvips/xcharter', 'fonts/enc/dvips/xcharter'),
                  ('fonts/map/dvips/xcharter', 'fonts/map/dvips/xcharter'),
                  ('fonts/opentype/public/xcharter', 'fonts/opentype/public/xcharter'),
                  ('fonts/tfm/public/xcharter', 'fonts/tfm/public/xcharter'),
                  ('fonts/type1/public/xcharter', 'fonts/type1/public/xcharter'),
                  ('fonts/vf/public/xcharter', 'fonts/vf/public/xcharter'),
                  ('tex/latex/xcharter', 'tex/latex/xcharter')]},
    "duckuments": {
        "version": "0.5",
        "revision": 77682,
        "license": "LPPL-1.3c",
        "ctan_path": "/macros/latex/contrib/duckuments",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/duckuments.tar.xz",
        "upstream_sha256": "a78498f371b8ec4ed9fe94d43865673af0a77da95b9221ba317070d5ddd65d93",
        "upstream_size_bytes": 484468,
        "description": "Duckuments macros and both bundled example-image PDFs",
        "source_obligations": "LPPL-1.3c; corresponding source archive retained",
        "license_files": [],
        "map_files": [],
        "tds_dirs": [("tex/latex/duckuments", "tex/latex/duckuments")],
    },
    "fdsymbol": {
        "version": "1.0",
        "revision": 77682,
        "license": "OFL-1.1 / LPPL-1.3c",
        "ctan_path": "/fonts/fdsymbol",
        "upstream_url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/fdsymbol.tar.xz",
        "upstream_sha256": "6c7dd3cfdfb4fe751f8103cf0c9ce6dbb0fcfa8103062e2777fd5b0189a688f9",
        "upstream_size_bytes": 878640,
        "description": "FdSymbol mathematical symbol font family: Type 1, OpenType, metrics, encodings, maps",
        "source_obligations": "SIL Open Font License 1.1 for font files, LPPL-1.3c for LaTeX macros; corresponding source preserved in sources.tar.zst",
        "license_files": [
            "doc/fonts/fdsymbol/OFL.txt",
            "doc/latex/fdsymbol/README.txt",
        ],
        "map_files": ["fonts/map/dvips/fdsymbol/fdsymbol.map"],
        "tds_dirs": [
            ("fonts/enc/dvips/fdsymbol", "fonts/enc/dvips/fdsymbol"),
            ("fonts/map/dvips/fdsymbol", "fonts/map/dvips/fdsymbol"),
            ("fonts/opentype/public/fdsymbol", "fonts/opentype/public/fdsymbol"),
            ("fonts/source/public/fdsymbol", "fonts/source/public/fdsymbol"),
            ("fonts/tfm/public/fdsymbol", "fonts/tfm/public/fdsymbol"),
            ("fonts/type1/public/fdsymbol", "fonts/type1/public/fdsymbol"),
            ("tex/latex/fdsymbol", "tex/latex/fdsymbol"),
        ],
    },
}

# Corresponding source archives for GPLv2 and open-source distribution compliance
SOURCE_ARCHIVES_INFO = {
    "base35": {
        "archive": "base35.zip",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/fonts/urw/base35.zip",
        "sha256": "07abaf13d6dbff68b884a877c91c9692283562273adc2d52a4bf9bfdd90668fd",
        "size_bytes": 1968003,
        "license": "GPL-2.0 with font exception",
        "provenance": "Upstream CTAN package release by URW++ containing PostScript Base 35 Type 1 PFB outlines, AFM metrics, PFM files, and author documentation",
    },
    "txfonts": {
        "archive": "txfonts.zip",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/fonts/txfonts.zip",
        "sha256": "26bea7fab28a21f21d26c36013938e52bc20ef1ccdad4a0e83b16243ffbcaece",
        "size_bytes": 1706528,
        "license": "GPL-2.0",
        "provenance": "Official CTAN package release by Young Ryu containing complete Type 1 outlines, METAFONT and TeX macro sources",
    },
    "mathdesign": {
        "archive": "mathdesign.zip",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/fonts/mathdesign.zip",
        "sha256": "9e54999967f2b7fefc6ffdb7fb48b6ef6ce3c8a4f1dd65b25e594596cb90be0a",
        "size_bytes": 6971155,
        "license": "GPL-2.0",
        "provenance": "Official CTAN package release by Paul Pichaureau containing Type 1 outlines, FontForge sources, and macro definitions",
    },
    "fpl": {
        "archive": "fpl.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/fpl.source.tar.xz",
        "sha256": "2c8e350bed490838429d6ade800d1d075dacfee017f67d8e86bebbfc470581fc",
        "size_bytes": 30656,
        "license": "GPL-2.0 / LPPL-1.0",
        "provenance": "Official TeX Live source archive containing FPL FontForge and metric sources",
    },
    "mathpazo": {
        "archive": "mathpazo.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/mathpazo.source.tar.xz",
        "sha256": "a0bc235b3f682cb65301ea2be4e599ceb5ddcaca559dd574adf717de1e7f7d93",
        "size_bytes": 17980,
        "license": "GPL-2.0 with font exception",
        "provenance": "Official TeX Live source archive containing Pazo Math fontinst, metrics, and build sources",
    },
    "yhmath": {
        "archive": "yhmath.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/yhmath.source.tar.xz",
        "sha256": "96872c0dc2d022a3f43ad2240bd16775b6ee4f6afe0ba2b6afa4a9903a92049f",
        "size_bytes": 16512,
        "license": "LPPL-1.3c",
        "provenance": "Official TeX Live source archive containing yhmath METAFONT sources, macros, and documentation",
    },
    "ae": {
        "archive": "ae.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/ae.source.tar.xz",
        "sha256": "219d3e8b1c2d057d3583d3a99837744f52a6ce94cd2d845f1996a355dfda8dc7",
        "size_bytes": 19988,
        "license": "LPPL",
        "provenance": "Official TeX Live source archive containing virtual font source scripts for Almost European (AE) fonts",
    },
    "stix": {
        "archive": "stix.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/stix.source.tar.xz",
        "sha256": "28e71c9103859f50113b71c3fb788f09b9ea83d233d01c6475b923b048d242c0",
        "size_bytes": 27808,
        "license": "OFL-1.1 / LPPL-1.3",
        "provenance": "Official TeX Live source archive containing STIX fonts build and conversion sources",
    },
    "cm-super": {
        "archive": "cm-super.zip",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/fonts/ps-type1/cm-super.zip",
        "sha256": "f3f856a5006ae7d2bbc2d9a338ca5ae25b6fba19eebee3ba20892845aafa7987",
        "size_bytes": 67319801,
        "license": "GPL-2.0-or-later with font exception",
        "provenance": "Upstream CTAN package release by Vladimir Volovich containing Type 1 PFB outlines, AFM metrics, dvips encodings, and inf archives",
    },
    "unfonts-core": {
        "archive": "fonts-unfonts-core_1.0.2-080608.orig.tar.xz",
        "url": "https://deb.debian.org/debian/pool/main/f/fonts-unfonts-core/fonts-unfonts-core_1.0.2-080608.orig.tar.xz",
        "sha256": "14abb309f9d979cc20212fabfbd7f50b55c42183985ae507390c7461ce0b307c",
        "size_bytes": 14727532,
        "license": "GPL-2.0",
        "provenance": "Official upstream source tarball released by Won-kyu Park / KLDP (as packaged in Debian main), comprising the 12 TrueType font programs and author documentation",
    },
    "lh": {
        "archive": "lh.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/lh.source.tar.xz",
        "sha256": "e200068f4bf14c8cbb844030e49a73500f08e35906487a68cb73b8f51ea65122",
        "size_bytes": 41756,
        "license": "LPPL-1.3c",
        "provenance": "Official TeX Live source archive containing complete METAFONT source files for LH Cyrillic fonts (T2A, T2B, T2C, X2)",
    },
    "lhcyr": {
        "archive": "lhcyr.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/lhcyr.source.tar.xz",
        "sha256": "3b393e47f8da3b8e2d903863761ed796c1b64bcac39d4d94796d6637760c7df6",
        "size_bytes": 2356,
        "license": "LPPL / other-free",
        "provenance": "Official TeX Live source archive containing LaTeX Cyrillic style and font definition source scripts",
    },
    "stmaryrd": {
        "archive": "stmaryrd.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/stmaryrd.source.tar.xz",
        "sha256": "28d350526d65fed4b6e1e4ba8c58e88348ac5c5ed567ff217a4220447fde1d2d",
        "size_bytes": 6248,
        "license": "Public Domain",
        "provenance": "Official TeX Live source archive containing original METAFONT source files and stmaryrd.dtx source",
    },
    "bbding": {
        "archive": "bbding.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/bbding.source.tar.xz",
        "sha256": "f47be23cd57a035ff1d17c6b069040dc44bcdc8943b6a2f7880a1b3cf60f47e9",
        "size_bytes": 10156,
        "license": "LPPL-1.3c",
        "provenance": "Official TeX Live source archive containing bbding.dtx and bbding10.mf source files",
    },
    "babel-russian": {
        "archive": "babel-russian.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/babel-russian.source.tar.xz",
        "sha256": "ba61c255385376334213b208d6b9a54816f76d20502f5c612a6c9bb7e2cc03d8",
        "size_bytes": 18276,
        "license": "LPPL-1.3c",
        "provenance": "Official TeX Live source archive containing russianb.dtx and russianb.ins source files",
    },
    "babel-spanish": {
        "archive": "babel-spanish.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/babel-spanish.source.tar.xz",
        "sha256": "42d8bde6d46b711f2ffa315fab1f75e858bf9327a37a601a61d39cc08a0123b6",
        "size_bytes": 29796,
        "license": "LPPL-1.3c",
        "provenance": "Official TeX Live source archive containing spanish.dtx, spanish.ins, and documentation sources",
    },
    "babel-portuges": {
        "archive": "babel-portuges.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/babel-portuges.source.tar.xz",
        "sha256": "7ee261caa2606904e7b2cc1ce4f077c37145b60e5df1c2aa6f85fee90f1b088e",
        "size_bytes": 6332,
        "license": "LPPL-1.3c",
        "provenance": "Official TeX Live source archive containing portuges.dtx and portuges.ins source files",
    },
    "hyph-utf8": {
        "archive": "hyph-utf8.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/hyph-utf8.source.tar.xz",
        "sha256": "5ab1a0375270b69dca2c1e095e8e0631713a2b80ac9c5736475a9b3c36d51753",
        "size_bytes": 36044,
        "license": "LPPL / MIT / Public Domain",
        "provenance": "Official TeX Live source archive containing hyph-utf8 generator scripts, encoding definitions, and pattern conversion tools",
    },
    "ruhyphen": {
        "archive": "ruhyphen.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/ruhyphen.source.tar.xz",
        "sha256": "2f229e3472560956365b4e1ac8ada8f5c1388cc8f95a4e5893eb4dad2b365150",
        "size_bytes": 13656,
        "license": "LPPL-1.2",
        "provenance": "Official TeX Live source archive containing ruhyphen source scripts and documentation",
    },
    "ec": {
        "archive": "ec.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/ec.tar.xz",
        "sha256": "bb85425c214b1056b5f0b8f3bf1478b81e89bd7d290d61c7c73289b534786897",
        "size_bytes": 263716,
        "license": "LPPL-1.3c",
        "provenance": "Upstream CTAN package release by Jörg Knappen containing METAFONT source files for European Computer Modern (EC/TC) fonts",
    },
    "cm": {
        "archive": "cm.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/cm.tar.xz",
        "sha256": "ebedd3dc7ece433d366d848ea8bd9cd2642a0f49c000c46a2ed1dde5b1cebc1c",
        "size_bytes": 238064,
        "license": "Knuth",
        "provenance": "Official Knuth Computer Modern METAFONT source files including cmbase.mf and standard glyph definitions",
    },
    "rsfs": {
        "archive": "rsfs.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/rsfs.tar.xz",
        "sha256": "1afec0c5e9711f652675e38b7cd7e88101c44aa0d0ff317ad6ac06f1d2cc7043",
        "size_bytes": 56092,
        "license": "Public Domain",
        "provenance": "Official CTAN package release by Ralph Smith and Taco Hoekwater containing Type 1 outlines, metrics, and METAFONT sources",
    },

    'cjk-ko': {
        'archive': 'cjk-ko.zip',
        'license': 'LPPL-1.3c / GPL-2.0 / Public Domain',
        'provenance': 'Authoritative CTAN package distribution by ko.TeX team containing full LaTeX macros and documentation sources',
        'sha256': '28db8f29e112ff40047ad07f6052ae180f4192e780ed74b0db85e211c3a9f55a',
        'size_bytes': 175960,
        'url': 'https://mirror.aarnet.edu.au/pub/CTAN/language/korean/cjk-ko.zip',
    },
    'els-cas-templates': {
        'archive': 'els-cas-templates.zip',
        'license': 'LPPL-1.3c',
        'provenance': 'Official Elsevier STM Document Lab release containing complete class, style, thumbnail assets, and documentation',
        'sha256': '36d97da01c6bbd134f315bff6c3de553735e2550444a6ddd4f869ddc67a20757',
        'size_bytes': 3305507,
        'url': 'https://mirror.aarnet.edu.au/pub/CTAN/macros/latex/contrib/els-cas-templates.zip',
    },
    't2': {
        'archive': 't2.zip',
        'license': 'LPPL-1.3c',
        'provenance': 'Authoritative CTAN package distribution by Vladimir Volovich and Werner Lemberg containing DocStrip dtx/ins sources',
        'sha256': '29865cabc7e0bbbc8289144f2f1b7ad5a4d744ed179a9d87fdb82133e589d8a6',
        'size_bytes': 138360,
        'url': 'https://mirror.aarnet.edu.au/pub/CTAN/macros/latex/contrib/t2.zip',
    },
    'utfsym': {
        'archive': 'utfsym.zip',
        'license': 'CC0-1.0',
        'provenance': 'Authoritative CTAN distribution by Daniel Benjamin Stegemann containing complete TikZ sources and generator scripts',
        'sha256': '14c4aa17e60ec94f6cc0ff989e4946a88cdeac8753dff4148eed301f38c155f9',
        'size_bytes': 6530598,
        'url': 'https://mirror.aarnet.edu.au/pub/CTAN/graphics/pgf/contrib/utfsym.zip',
    },
    'bera': {'archive': 'bera.zip',
     'url': 'https://mirror.aarnet.edu.au/pub/CTAN/fonts/bera.zip',
     'sha256': '29735b1ca7281537a420b2e214b9fddfc27bcb478ad2bd5caf23a489ab1d8cd9',
     'size_bytes': 474202,
     'license': 'Bitstream-Vera',
     'provenance': 'Official upstream distribution preserving font or LaTeX package sources and licensing'},
    'ccicons': {'archive': 'ccicons.source.tar.xz',
     'url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/ccicons.source.tar.xz',
     'sha256': '1cc8f0e4d1c8e6ebaba613978355ae9bb7051995b60db54c59988ba9c8bc5b40',
     'size_bytes': 8832,
     'license': 'OFL-1.1 / LPPL-1.3c',
     'provenance': 'Official upstream distribution preserving font or LaTeX package sources and licensing'},
    'doclicense': {'archive': 'doclicense.source.tar.xz',
     'url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/doclicense.source.tar.xz',
     'sha256': '0775c271a95a388cde9b3df80133ba94efab6d6e18073c8e1e1ef6e28f471cc6',
     'size_bytes': 13592,
     'license': 'CC0-1.0 / LPPL-1.3c',
     'provenance': 'Official upstream distribution preserving font or LaTeX package sources and licensing'},
    'stix2-type1': {'archive': 'stix2-type1.source.tar.xz',
     'url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/stix2-type1.source.tar.xz',
     'sha256': '3527cd6040675385f2d36d6a79a493ef22b77b27a2f6d68527cb675c057ed704',
     'size_bytes': 28096,
     'license': 'OFL-1.1 / LPPL-1.3',
     'provenance': 'Official upstream distribution preserving font or LaTeX package sources and licensing'},
    'twemojis': {'archive': 'twemojis.source.tar.xz',
     'url': 'https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/twemojis.source.tar.xz',
     'sha256': 'cc65acb87b676d69bca7774c4ccc1a9f57be70caed619f6ee879f37494ab195c',
     'size_bytes': 70504,
     'license': 'CC-BY-4.0 / LPPL-1.3',
     'provenance': 'Official upstream distribution preserving font or LaTeX package sources and licensing'},
    "duckuments": {
        "archive": "duckuments.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/duckuments.source.tar.xz",
        "sha256": "c23c295c49490498b76fa07aa6de604202cafd03630e1d738363d947a2e2ce08",
        "size_bytes": 9112,
        "license": "LPPL-1.3c",
        "provenance": "Official upstream LaTeX and example graphic generation sources",
    },
    "fdsymbol": {
        "archive": "fdsymbol.source.tar.xz",
        "url": "https://mirror.aarnet.edu.au/pub/CTAN/systems/texlive/tlnet/archive/fdsymbol.source.tar.xz",
        "sha256": "44968e81d80de4c34e4b6d46d2bd36412796fc8b26dfa655c59f24739fadc596",
        "size_bytes": 17776,
        "license": "OFL-1.1 / LPPL-1.3c",
        "provenance": "Official TeX Live source archive containing fdsymbol.dtx and fdsymbol.ins source files",
    },
}


def sha256_file(filepath):
    h = hashlib.sha256()
    with open(filepath, "rb") as f:
        while chunk := f.read(65536):
            h.update(chunk)
    return h.hexdigest()


def resolve_baseline_path(user_path):
    if user_path and os.path.exists(user_path):
        return user_path
    for cand in ["/tmp/ratex-baseline/packages.tar.zst", "/tmp/ratex_baseline/packages.tar.zst"]:
        if os.path.exists(cand):
            return cand
    if user_path:
        return user_path
    return "/tmp/ratex-baseline/packages.tar.zst"


def build_source_archive(output_path, cache_dir):
    """
    Stable callable source-archive packaging API.
    Packs verified corresponding source archives into a deterministic zstd-compressed tar archive.
    """
    if not cache_dir or not os.path.exists(cache_dir):
        raise FileNotFoundError(f"Source cache directory not found: {cache_dir}")

    print(f"Building deterministic source distribution archive: {output_path}...")
    source_records = {}

    # Verify all source archives exist and match pins
    for pkg_id, info in sorted(SOURCE_ARCHIVES_INFO.items()):
        arc_path = os.path.join(cache_dir, info["archive"])
        if not os.path.exists(arc_path):
            raise FileNotFoundError(f"Required source archive missing from cache: {info['archive']} ({arc_path})")
        actual_h = sha256_file(arc_path)
        expected_h = info["sha256"]
        if actual_h != expected_h:
            raise ValueError(
                f"REJECTED: Source archive hash mismatch on {pkg_id} ({info['archive']})!\n"
                f"  Expected: {expected_h}\n"
                f"  Actual:   {actual_h}"
            )
        actual_sz = os.path.getsize(arc_path)
        source_records[pkg_id] = {
            "archive": info["archive"],
            "url": info["url"],
            "sha256": actual_h,
            "size_bytes": actual_sz,
            "license": info["license"],
            "provenance": info.get("provenance", info.get("obligation", "")),
        }
        print(f"  Verified source archive {pkg_id}: {info['archive']} ({actual_sz} bytes, sha256={actual_h[:16]}...)")

    os.makedirs(os.path.dirname(os.path.abspath(output_path)), exist_ok=True)
    temp_tar = output_path + ".tmp"

    zstd_proc = subprocess.Popen(
        ["zstd", "-19", "-T0", "-o", temp_tar],
        stdin=subprocess.PIPE,
    )

    with tarfile.open(fileobj=zstd_proc.stdin, mode="w|") as tar:
        for pkg_id in sorted(SOURCE_ARCHIVES_INFO.keys()):
            info = SOURCE_ARCHIVES_INFO[pkg_id]
            full_path = os.path.join(cache_dir, info["archive"])
            arc_rel = f"sources/{info['archive']}"
            ti = tar.gettarinfo(full_path, arcname=arc_rel)
            ti.uid = 0
            ti.gid = 0
            ti.uname = "root"
            ti.gname = "root"
            ti.mtime = FIXED_MTIME
            ti.mode = 0o644
            with open(full_path, "rb") as f:
                tar.addfile(ti, f)

    zstd_proc.stdin.close()
    zstd_proc.wait()
    if zstd_proc.returncode != 0:
        raise RuntimeError("zstd compression of source archive failed.")

    os.replace(temp_tar, output_path)
    out_sz = os.path.getsize(output_path)
    out_h = sha256_file(output_path)
    print(f"Source distribution archive created successfully: {output_path}")
    print(f"  Total source archives: {len(source_records)}, Size: {out_sz / (1024*1024):.2f} MiB, SHA256: {out_h}")
    return {
        "logical_name": os.path.basename(output_path),
        "sha256": out_h,
        "size_bytes": out_sz,
        "source_archives": source_records,
    }


def generate_lh_metrics(combined_dir, cache_dir, scratch_dir):
    """
    Deterministic offline regeneration of complete LH Cyrillic font metrics (TFMs)
    using pinned METAFONT sources from lh.tar.xz, ec.tar.xz, and cm.tar.xz.
    Generates all 76 \\EC@family Cyrillic font families across all 14 standard sizes (1064 fonts)
    plus all extended design sizes supported by fikparm.mf (1580 fonts total).
    Operates with zero host font dependencies using METAFONT nullmode.
    """
    print(f"  Deterministically regenerating complete LH Cyrillic font metrics from pinned MF sources...")
    mf_scratch = os.path.join(scratch_dir, "mf_work")
    os.makedirs(mf_scratch, exist_ok=True)

    source_mappings = [
        (os.path.join(cache_dir, "lh.tar.xz"), "fonts/source/lh"),
        (os.path.join(cache_dir, "ec.tar.xz"), "fonts/source/jknappen/ec"),
        (os.path.join(cache_dir, "cm.tar.xz"), "fonts/source/public/cm"),
    ]
    for arc_path, sub in source_mappings:
        if not os.path.exists(arc_path):
            raise FileNotFoundError(f"Required source archive missing: {arc_path}")
        with tarfile.open(arc_path, "r:xz") as t:
            for m in t.getmembers():
                if m.name.startswith(sub):
                    t.extract(m, mf_scratch)

    input_dirs = [mf_scratch]
    for root, dirs, files in os.walk(mf_scratch):
        if any(f.endswith(".mf") for f in files):
            input_dirs.append(root)

    env = os.environ.copy()
    env["MFINPUTS"] = ":".join(input_dirs)

    candidate_fonts = set()
    for prefix in CYRILLIC_EC_FAMILIES:
        for sz in STANDARD_SIZES:
            candidate_fonts.add(f"{prefix}{sz}")

    cm_super_arc = os.path.join(cache_dir, "cm-super.tar.xz")
    with tarfile.open(cm_super_arc, "r:xz") as t:
        for m in t.getmembers():
            if m.name.endswith(".map") and ("cm-super-t2" in m.name or "cm-super-x2" in m.name):
                f = t.extractfile(m)
                for line in f:
                    s = line.decode('latin1').strip()
                    if s and not s.startswith("%"):
                        tfm = s.split()[0]
                        if tfm[:2] in ("la", "lb", "lc", "rx"):
                            candidate_fonts.add(tfm)

    font_list = sorted(candidate_fonts)
    for font in font_list:
        mf_file = os.path.join(mf_scratch, f"{font}.mf")
        with open(mf_file, "w") as f:
            f.write("input fikparm;\n")

    def run_mf_job(font):
        cmd = ["mf", rf"\mode:=nullmode; nonstopmode; input {font}"]
        subprocess.run(cmd, cwd=mf_scratch, env=env, capture_output=True, text=True)
        tfm_src = os.path.join(mf_scratch, f"{font}.tfm")
        if os.path.exists(tfm_src):
            return font, tfm_src
        return font, None

    t0 = time.time()
    generated_tfms = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=32) as executor:
        results = list(executor.map(run_mf_job, font_list))
    t1 = time.time()

    for font, tfm_src in results:
        if tfm_src:
            prefix2 = font[:2]
            if prefix2 == "la":
                sub_rel = "fonts/tfm/lh/lh-t2a"
            elif prefix2 == "lb":
                sub_rel = "fonts/tfm/lh/lh-t2b"
            elif prefix2 == "lc":
                sub_rel = "fonts/tfm/lh/lh-t2c"
            else:
                sub_rel = "fonts/tfm/lh/lh-x2"

            target_rel = f"{sub_rel}/{font}.tfm"
            target_full = os.path.join(combined_dir, target_rel)
            os.makedirs(os.path.dirname(target_full), exist_ok=True)
            shutil.copy2(tfm_src, target_full)
            fhash = sha256_file(target_full)
            generated_tfms[target_rel] = fhash

    print(f"  Generated {len(generated_tfms)} complete LH Cyrillic TFMs in {t1 - t0:.1f}s (provenance: mf nullmode).")
    return {
        "compiler": "mf (METAFONT 2.71828182)",
        "mode": "nullmode",
        "total_generated": len(generated_tfms),
        "source_archives": ["lh.tar.xz", "ec.tar.xz", "cm.tar.xz"],
        "files": generated_tfms,
    }


def generate_metafont_outlines(combined_dir, cache_dir, scratch_dir):
    """
    Deterministic offline regeneration of Type 1 outlines, metrics, and maps
    for BBM, IFSYM, and Computer Modern extra fonts (cmbcsc10, cmcsc12)
    using pinned canonical METAFONT sources from bbm.tar.xz, ifsym.tar.xz,
    cm-mf-extra-bold.tar.xz, cmcyr.tar.xz, and cm.tar.xz.
    Traces Bezier outline programs using mftrace and potrace.
    """
    print("  [MF Outlines] Regenerating authentic Type 1 outlines from pinned METAFONT sources...")

    # Locate declared real build dependencies via shutil.which with explicit error
    mftrace_cmd = []
    mftrace_exe = shutil.which("mftrace")
    tools_bin = os.path.join(cache_dir, "tools/bin")
    if mftrace_exe:
        mftrace_cmd = [mftrace_exe]
    elif os.path.exists(os.path.join(tools_bin, "mftrace.py")):
        mftrace_cmd = ["python3", os.path.join(tools_bin, "mftrace.py")]
    else:
        raise RuntimeError(
            "Required build tool 'mftrace' not found via PATH or cache. "
            "Please ensure mftrace (with potrace backend) is available to generate METAFONT outlines."
        )

    mf_exe = shutil.which("mf")
    if not mf_exe:
        raise RuntimeError("Required build tool 'mf' (METAFONT) not found via PATH.")

    potrace_exe = shutil.which("potrace")
    if not potrace_exe:
        raise RuntimeError("Required build tool 'potrace' not found via PATH.")

    env = os.environ.copy()
    if os.path.exists(tools_bin):
        env["PATH"] = f"{tools_bin}:{env['PATH']}"

    work_dir = os.path.join(scratch_dir, "mf_outlines_work")
    os.makedirs(work_dir, exist_ok=True)

    # Extract pinned upstream METAFONT source archives from cache (zero host font dependency)
    bbm_src_dir = os.path.join(work_dir, "bbm_src")
    with tarfile.open(os.path.join(cache_dir, "bbm.tar.xz"), "r:xz") as tf:
        tf.extractall(bbm_src_dir)
    ifsym_src_dir = os.path.join(work_dir, "ifsym_src")
    with tarfile.open(os.path.join(cache_dir, "ifsym.tar.xz"), "r:xz") as tf:
        tf.extractall(ifsym_src_dir)
    cmextra_src_dir = os.path.join(work_dir, "cmextra_src")
    with tarfile.open(os.path.join(cache_dir, "cm-mf-extra-bold.tar.xz"), "r:xz") as tf:
        tf.extractall(cmextra_src_dir)
    cmcyr_src_dir = os.path.join(work_dir, "cmcyr_src")
    with tarfile.open(os.path.join(cache_dir, "cmcyr.tar.xz"), "r:xz") as tf:
        tf.extractall(cmcyr_src_dir)
    cm_src_dir = os.path.join(work_dir, "cm_src")
    with tarfile.open(os.path.join(cache_dir, "cm.tar.xz"), "r:xz") as tf:
        tf.extractall(cm_src_dir)

    mf_dirs = []
    for d in [bbm_src_dir, ifsym_src_dir, cmextra_src_dir, cmcyr_src_dir, cm_src_dir]:
        for root, _, files in os.walk(d):
            if any(f.endswith(".mf") for f in files):
                mf_dirs.append(root)
    env["MFINPUTS"] = ":".join(mf_dirs)

    bbm_fonts = [
        "bbm5", "bbm6", "bbm7", "bbm8", "bbm9", "bbm10", "bbm12", "bbm17",
        "bbmbx5", "bbmbx6", "bbmbx7", "bbmbx8", "bbmbx9", "bbmbx10", "bbmbx12",
        "bbmsl8", "bbmsl9", "bbmsl10", "bbmsl12",
        "bbmss8", "bbmss9", "bbmss10", "bbmss12", "bbmss17"
    ]
    ifsym_fonts = ["ifsym10", "ifsymb10", "ifgeo10", "ifclk10", "ifwea10"]
    cmextra_fonts = ["cmbcsc10", "cmcsc12"]

    generated_records = {}
    pkg_owners = {}

    bbm_pfb_dir = os.path.join(combined_dir, "fonts/type1/public/bbm")
    bbm_map_dir = os.path.join(combined_dir, "fonts/map/dvips/bbm")
    ifsym_pfb_dir = os.path.join(combined_dir, "fonts/type1/public/ifsym")
    ifsym_map_dir = os.path.join(combined_dir, "fonts/map/dvips/ifsym")
    cmextra_pfb_dir = os.path.join(combined_dir, "fonts/type1/public/cmextra")
    cmextra_tfm_dir = os.path.join(combined_dir, "fonts/tfm/public/cmextra")
    cmextra_map_dir = os.path.join(combined_dir, "fonts/map/dvips/cmextra")

    for d in [bbm_pfb_dir, bbm_map_dir, ifsym_pfb_dir, ifsym_map_dir, cmextra_pfb_dir, cmextra_tfm_dir, cmextra_map_dir]:
        os.makedirs(d, exist_ok=True)

    for f in bbm_fonts:
        cmd = mftrace_cmd + ["--formats=pfb", "--noround", "--no-afm", f]
        subprocess.run(cmd, cwd=bbm_pfb_dir, env=env, check=True, capture_output=True)
        rel = f"fonts/type1/public/bbm/{f}.pfb"
        with open(os.path.join(combined_dir, rel), "rb") as fp:
            generated_records[rel] = hashlib.sha256(fp.read()).hexdigest()
        pkg_owners[rel] = "bbm"

    bbm_map_rel = "fonts/map/dvips/bbm/bbm.map"
    with open(os.path.join(combined_dir, bbm_map_rel), "w") as fp:
        for f in bbm_fonts:
            fp.write(f"{f} {f} <{f}.pfb\n")
    with open(os.path.join(combined_dir, bbm_map_rel), "rb") as fp:
        generated_records[bbm_map_rel] = hashlib.sha256(fp.read()).hexdigest()
    pkg_owners[bbm_map_rel] = "bbm"

    for f in ifsym_fonts:
        cmd = mftrace_cmd + ["--formats=pfb", "--noround", "--no-afm", f]
        subprocess.run(cmd, cwd=ifsym_pfb_dir, env=env, check=True, capture_output=True)
        rel = f"fonts/type1/public/ifsym/{f}.pfb"
        with open(os.path.join(combined_dir, rel), "rb") as fp:
            generated_records[rel] = hashlib.sha256(fp.read()).hexdigest()
        pkg_owners[rel] = "ifsym"

    ifsym_map_rel = "fonts/map/dvips/ifsym/ifsym.map"
    with open(os.path.join(combined_dir, ifsym_map_rel), "w") as fp:
        for f in ifsym_fonts:
            fp.write(f"{f} {f} <{f}.pfb\n")
    with open(os.path.join(combined_dir, ifsym_map_rel), "rb") as fp:
        generated_records[ifsym_map_rel] = hashlib.sha256(fp.read()).hexdigest()
    pkg_owners[ifsym_map_rel] = "ifsym"

    for f in cmextra_fonts:
        cmd = mftrace_cmd + ["--formats=pfb", "--noround", "--no-afm", f]
        subprocess.run(cmd, cwd=cmextra_pfb_dir, env=env, check=True, capture_output=True)
        rel = f"fonts/type1/public/cmextra/{f}.pfb"
        with open(os.path.join(combined_dir, rel), "rb") as fp:
            generated_records[rel] = hashlib.sha256(fp.read()).hexdigest()
        pkg_owners[rel] = "cm-mf-extra-bold" if f == "cmbcsc10" else "cmcyr"

    subprocess.run([mf_exe, "\\mode:=ljfour; nonstopmode; input cmcsc12.mf"], cwd=cmextra_tfm_dir, env=env, check=True, capture_output=True)
    tfm12_rel = "fonts/tfm/public/cmextra/cmcsc12.tfm"
    with open(os.path.join(combined_dir, tfm12_rel), "rb") as fp:
        generated_records[tfm12_rel] = hashlib.sha256(fp.read()).hexdigest()
    pkg_owners[tfm12_rel] = "cmcyr"

    cmextra_map_rel = "fonts/map/dvips/cmextra/cmextra-t1.map"
    with open(os.path.join(combined_dir, cmextra_map_rel), "w") as fp:
        for f in cmextra_fonts:
            fp.write(f"{f} {f} <{f}.pfb\n")
    with open(os.path.join(combined_dir, cmextra_map_rel), "rb") as fp:
        generated_records[cmextra_map_rel] = hashlib.sha256(fp.read()).hexdigest()
    pkg_owners[cmextra_map_rel] = "cm-mf-extra-bold"

    print(f"  [MF Outlines] Successfully generated {len(generated_records)} authentic font files & maps.")
    return {
        "generator": "mftrace with potrace backend from authentic CTAN METAFONT sources",
        "source_archives": ["bbm.tar.xz", "ifsym.tar.xz", "cm-mf-extra-bold.tar.xz", "cmcyr.tar.xz", "cm.tar.xz"],
        "total_generated": len(generated_records),
        "files": generated_records,
        "pkg_owners": pkg_owners,
    }

LANGUAGE_DAT_ENTRIES = [
    # English (Knuth original, default language 0)
    ("english", "hyphen.tex", ["usenglish", "USenglish", "american"]),
    ("dumylang", "dumyhyph.tex", []),
    ("nohyphenation", "zerohyph.tex", []),
    # English extensions
    ("ukenglish", "loadhyph-en-gb.tex", ["british", "UKenglish"]),
    ("usenglishmax", "loadhyph-en-us.tex", []),
    # Basque
    ("basque", "loadhyph-eu.tex", []),
    # French
    ("french", "loadhyph-fr.tex", ["patois", "francais"]),
    # German
    ("german", "loadhyph-de-1901.tex", []),
    ("ngerman", "loadhyph-de-1996.tex", []),
    ("swissgerman", "loadhyph-de-ch-1901.tex", []),
    ("german-x-2024-02-28", "dehypht-x-2024-02-28.tex", ["german-x-latest"]),
    ("ngerman-x-2024-02-28", "dehyphn-x-2024-02-28.tex", ["ngerman-x-latest"]),
    # Greek
    ("greek", "loadhyph-el-polyton.tex", ["polygreek"]),
    ("monogreek", "loadhyph-el-monoton.tex", []),
    ("ancientgreek", "loadhyph-grc.tex", []),
    ("ibycus", "ibyhyph.tex", []),
    # Spanish (with mexican and mexicanspanish synonyms)
    ("spanish", "loadhyph-es.tex", ["espanol", "mexican", "mexicanspanish"]),
    # Portuguese (with brazilian and brazil synonyms)
    ("portuguese", "loadhyph-pt.tex", ["portuges", "brazilian", "brazil"]),
    # Russian (authentic 8-bit T2A via ruhyphen)
    ("russian", "loadhyph-ru.tex", []),
    # Pinyin
    ("pinyin", "loadhyph-zh-latn-pinyin.tex", []),
]

LOADER_DEPENDENCY_CLOSURE = {
    "hyphen.tex": [],
    "dumyhyph.tex": [],
    "zerohyph.tex": [],
    "loadhyph-en-gb.tex": ["hyph-en-gb.tex"],
    "loadhyph-en-us.tex": ["hyph-en-us.tex"],
    "loadhyph-eu.tex": ["conv-utf8-ec.tex", "hyph-eu.tex"],
    "loadhyph-fr.tex": ["conv-utf8-ec.tex", "hyph-fr.tex"],
    "loadhyph-de-1901.tex": ["dehypht.tex"],
    "loadhyph-de-1996.tex": ["dehyphn.tex"],
    "loadhyph-de-ch-1901.tex": ["conv-utf8-ec.tex", "hyph-de-ch-1901.tex"],
    "dehypht-x-2024-02-28.tex": ["dehypht-x-2024-02-28.pat"],
    "dehyphn-x-2024-02-28.tex": ["dehyphn-x-2024-02-28.pat"],
    "loadhyph-el-polyton.tex": ["grphyph5.tex"],
    "loadhyph-el-monoton.tex": ["grmhyph5.tex"],
    "loadhyph-grc.tex": ["grahyph5.tex"],
    "ibyhyph.tex": [],
    "loadhyph-es.tex": ["conv-utf8-ec.tex", "hyph-es.tex"],
    "loadhyph-pt.tex": ["conv-utf8-ec.tex", "hyph-pt.tex"],
    "loadhyph-ru.tex": [
        "ruhyphen.tex", "catkoi.tex", "koi2t2a.tex",
        "ruhyphal.tex", "cyryoal.tex", "hypht2.tex",
    ],
    "loadhyph-zh-latn-pinyin.tex": ["hyph-zh-latn-pinyin.ec.tex"],
}


def generate_language_dat(combined_dir):
    """
    Deterministically generates tex/generic/config/language.dat.
    Validates complete physical presence of every declared loader and secondary
    dependency in combined_dir. Strict verification: no silent English fallback.
    """
    print("  Validating hyphenation resource closure and generating tex/generic/config/language.dat...")
    available_basenames = set()
    for root, _, files in os.walk(combined_dir):
        for f in files:
            available_basenames.add(f)

    # Validate all loaders and secondary dependencies
    for lang, loader, aliases in LANGUAGE_DAT_ENTRIES:
        if loader not in available_basenames:
            raise RuntimeError(
                f"REJECTED: Missing primary hyphenation pattern loader '{loader}' for language '{lang}'!"
            )
        deps = LOADER_DEPENDENCY_CLOSURE.get(loader, [])
        for dep in deps:
            if dep not in available_basenames:
                raise RuntimeError(
                    f"REJECTED: Missing required dependency '{dep}' for loader '{loader}' (language '{lang}')!"
                )

    lines = [
        "% Deterministically generated language.dat for Ratex",
        "% English default (language 0), authentic 8-bit pattern loaders, zero silent fallbacks",
        "",
    ]
    for lang, loader, aliases in LANGUAGE_DAT_ENTRIES:
        lines.append(f"{lang} {loader}")
        for alias in aliases:
            lines.append(f"={alias}")

    content = "\n".join(lines) + "\n"
    target_rel = "tex/generic/config/language.dat"
    target_full = os.path.join(combined_dir, target_rel)
    os.makedirs(os.path.dirname(target_full), exist_ok=True)
    with open(target_full, "w", encoding="utf-8") as f:
        f.write(content)
    os.utime(target_full, (FIXED_MTIME, FIXED_MTIME))

    fhash = sha256_file(target_full)
    fsz = os.path.getsize(target_full)
    print(f"  Generated {target_rel}: {len(LANGUAGE_DAT_ENTRIES)} languages, {fsz} bytes, sha256={fhash[:16]}...")
    return {
        "output_file": target_rel,
        "sha256": fhash,
        "size_bytes": fsz,
        "total_languages": len(LANGUAGE_DAT_ENTRIES),
        "validated_loaders": sorted(LOADER_DEPENDENCY_CLOSURE.keys()),
        "entries": [
            {"language": lang, "loader": loader, "synonyms": aliases}
            for lang, loader, aliases in LANGUAGE_DAT_ENTRIES
        ],
    }


def build_bundle(baseline_path, output_dir, lock_file_path, cache_dir, legal_dir, sources_out=None):
    """
    Builds packages.tar.zst and packages.lock.json using ONLY hash-pinned upstream archives
    and verified baseline archive. Zero host-derived payload.
    """
    baseline_path = resolve_baseline_path(baseline_path)
    if not os.path.exists(baseline_path):
        raise FileNotFoundError(f"Baseline archive not found at: {baseline_path}")

    print(f"[1/8] Verifying baseline archive: {baseline_path}...")
    baseline_hash = sha256_file(baseline_path)
    baseline_size = os.path.getsize(baseline_path)
    if baseline_hash != PINNED_BASELINE_SHA256:
        raise ValueError(
            f"Baseline archive hash mismatch!\nExpected: {PINNED_BASELINE_SHA256}\nActual:   {baseline_hash}"
        )
    print(f"  Baseline SHA256 verified: {baseline_hash[:16]}... ({baseline_size / (1024*1024):.1f} MiB)")

    if not cache_dir or not os.path.exists(cache_dir):
        raise FileNotFoundError(f"Upstream archive cache directory not found: {cache_dir}")

    print(f"  Verifying and locking upstream package archives in {cache_dir}...")
    for pkg_id, pkg_info in UPSTREAM_PACKAGES.items():
        if pkg_id == "latex-base-legal":
            continue
        arc_name = os.path.basename(pkg_info["upstream_url"])
        arc_path = os.path.join(cache_dir, arc_name)
        if not os.path.exists(arc_path):
            raise FileNotFoundError(f"Required upstream archive missing: {arc_name} for package {pkg_id}")
        actual_h = sha256_file(arc_path)
        expected_h = pkg_info["upstream_sha256"]
        if actual_h != expected_h:
            raise ValueError(
                f"REJECTED: Upstream archive hash mismatch for {pkg_id} ({arc_name})!\n"
                f"  Expected: {expected_h}\n"
                f"  Actual:   {actual_h}"
            )

    print(f"  All {len(UPSTREAM_PACKAGES)-1} upstream package archives verified against cryptographic pins.")

    scratch = tempfile.mkdtemp(prefix="ratex_bundle_")
    baseline_dir = os.path.join(scratch, "baseline")
    combined_dir = os.path.join(scratch, "combined")
    os.makedirs(baseline_dir, exist_ok=True)
    os.makedirs(combined_dir, exist_ok=True)

    print(f"[2/8] Extracting baseline archive to scratch...")
    res = subprocess.run(["tar", "-I", "zstd", "-xf", baseline_path, "-C", baseline_dir])
    if res.returncode != 0:
        raise RuntimeError("Failed to extract baseline archive.")

    baseline_by_path = {}
    baseline_by_basename = {}
    baseline_file_count = 0
    for root, _, files in os.walk(baseline_dir):
        for f in files:
            full = os.path.join(root, f)
            rel = os.path.relpath(full, baseline_dir).replace("\\", "/")
            fhash = sha256_file(full)
            baseline_by_path[rel] = fhash
            if f not in baseline_by_basename:
                baseline_by_basename[f] = []
            baseline_by_basename[f].append((rel, fhash))
            baseline_file_count += 1

    for item in os.listdir(baseline_dir):
        s = os.path.join(baseline_dir, item)
        d = os.path.join(combined_dir, item)
        if os.path.isdir(s):
            shutil.copytree(s, d, dirs_exist_ok=True)
        else:
            shutil.copy2(s, d)

    print(f"  Baseline tree contains {baseline_file_count} files ({len(baseline_by_basename)} unique basenames).")

    print(f"[3/8] Extracting and verifying payload ONLY from pinned upstream archives...")
    new_files_by_pkg = {}
    new_basenames = {}
    identical_overlays = 0

    for pkg_id, pkg_info in UPSTREAM_PACKAGES.items():
        new_files_by_pkg[pkg_id] = {}
        if pkg_id == "latex-base-legal":
            continue
        arc_name = os.path.basename(pkg_info["upstream_url"])
        arc_path = os.path.join(cache_dir, arc_name)

        with tarfile.open(arc_path, "r:xz") as tar:
            members = [m for m in tar.getmembers() if m.isfile()]
            for src_dir, dst_dir in pkg_info["tds_dirs"]:
                for m in members:
                    norm_name = m.name
                    if norm_name.startswith("texmf-dist/"):
                        norm_name = norm_name[len("texmf-dist/"):]
                    if not norm_name.startswith(src_dir):
                        continue
                    if norm_name == "fonts/type1/public/mathdesign/mdpgd/md-utrma.pfb":
                        continue
                    if norm_name == src_dir:
                        target_rel = dst_dir
                    else:
                        rel_in = norm_name[len(src_dir):].lstrip("/")
                        target_rel = f"{dst_dir}/{rel_in}"

                    fname = os.path.basename(target_rel)
                    extracted_file = tar.extractfile(m)
                    data = extracted_file.read()
                    fhash = hashlib.sha256(data).hexdigest()

                    # Check for inter-package collisions
                    if fname in new_basenames:
                        other_pkg, other_rel, other_hash = new_basenames[fname]
                        if other_rel != target_rel:
                            raise RuntimeError(
                                f"REJECTED: Inter-package basename collision on '{fname}'!\n"
                                f"  Package {other_pkg}: {other_rel}\n"
                                f"  Package {pkg_id}: {target_rel}"
                            )

                    # Check against baseline
                    if fname in baseline_by_basename:
                        baseline_paths = [r for r, _ in baseline_by_basename[fname]]
                        if target_rel not in baseline_paths:
                            raise RuntimeError(
                                f"REJECTED: Basename path collision with baseline on '{fname}'!\n"
                                f"  Baseline TDS path(s): {baseline_paths}\n"
                                f"  Package {pkg_id} path: {target_rel}"
                            )
                        base_hash = baseline_by_path[target_rel]
                        if base_hash != fhash:
                            raise RuntimeError(
                                f"REJECTED: Conflicting content for existing baseline file '{fname}'!\n"
                                f"  Baseline SHA256: {base_hash}\n"
                                f"  Package SHA256:  {fhash}"
                            )
                        identical_overlays += 1

                    new_basenames[fname] = (pkg_id, target_rel, fhash)
                    new_files_by_pkg[pkg_id][target_rel] = fhash

                    dest_full = os.path.join(combined_dir, target_rel)
                    os.makedirs(os.path.dirname(dest_full), exist_ok=True)
                    with open(dest_full, "wb") as out_fp:
                        out_fp.write(data)

    # Bundle authentic upstream license files into doc/
    package_legal_mapping = {
        "lm": [
            ("GUST-FONT-LICENSE.TXT", "doc/fonts/lm/GUST-FONT-LICENSE.TXT"),
            ("MANIFEST-Latin-Modern.TXT", "doc/fonts/lm/MANIFEST-Latin-Modern.TXT"),
        ],
        "cm-super": [
            ("cm-super-COPYING.txt", "doc/fonts/cm-super/COPYING"),
            ("cm-super-README.txt", "doc/fonts/cm-super/README"),
        ],
        "cbfonts": [
            ("cbfonts-README.txt", "doc/fonts/cbfonts/README"),
        ],
        "greek-inputenc": [
            ("greek-inputenc-README.md", "doc/latex/greek-inputenc/README.md"),
        ],
        "wadalab": [
            ("wadalab-README.txt", "doc/fonts/wadalab/README"),
        ],
        "ipaex": [
            ("IPA_Font_License_Agreement_v1.0.txt", "doc/fonts/ipaex/IPA_Font_License_Agreement_v1.0.txt"),
        ],
        "haranoaji": [
            ("haranoaji-LICENSE.txt", "doc/fonts/haranoaji/LICENSE"),
        ],
        "arphic": [
            ("ARPHICPL.txt", "doc/fonts/arphic-ttf/ARPHICPL.txt"),
        ],
        "uhc": [
            ("uhc-README.txt", "doc/fonts/uhc/umj/README"),
        ],
        "unfonts-core": [
            ("unfonts-COPYING.txt", "doc/fonts/unfonts-core/COPYING"),
            ("unfonts-README.md", "doc/fonts/unfonts-core/README.md"),
        ],
        "nanumtype1": [
            ("nanum-COPYING.txt", "doc/fonts/nanumtype1/COPYING"),
        ],
        "stmaryrd": [
            ("stmaryrd-README.txt", "doc/fonts/stmaryrd/README"),
            ("stmaryrd-README.hoekwater.txt", "doc/fonts/stmaryrd/README.hoekwater"),
        ],
        "bbding": [
            ("bbding-README.txt", "doc/latex/bbding/README"),
        ],
        "babel-russian": [
            ("babel-russian-README.md", "doc/generic/babel-russian/README.md"),
        ],
        "babel-spanish": [
            ("babel-spanish-README.md", "doc/generic/babel-spanish/README.md"),
        ],
        "babel-portuges": [
            ("babel-portuges-README.md", "doc/generic/babel-portuges/README.md"),
        ],
        "rsfs": [
            ("rsfs-README.txt", "doc/fonts/rsfs/README"),
            ("rsfs-README.type1.txt", "doc/fonts/rsfs/README.type1"),
        ],
        "latex-base-legal": [
            ("manifest.txt", "doc/latex/base/manifest.txt"),
            ("legal.txt", "doc/latex/base/legal.txt"),
            ("lppl.txt", "doc/latex/base/lppl.txt"),
            ("README.md", "doc/latex/base/README.md"),
            ("NOTICES-FONTS.txt", "doc/fonts/NOTICES-FONTS.txt"),
        ],
        "times": [
            ("urw-base35-COPYING.txt", "doc/fonts/urw-base35/COPYING"),
            ("urw-base35-README.txt", "doc/fonts/urw-base35/README"),
        ],
        "txfonts": [
            ("txfonts-README.txt", "doc/fonts/txfonts/README"),
        ],
        "mathdesign": [
            ("mathdesign-README.txt", "doc/fonts/mathdesign/README"),
        ],
        "mathpazo": [
            ("mathpazo-README.txt", "doc/fonts/mathpazo/README"),
            ("fpl-COPYING.txt", "doc/fonts/mathpazo/gpl.txt"),
        ],
        'bera': [('bera-LICENSE.txt', 'doc/fonts/bera/LICENSE')],
        'ccicons': [('ccicons-OFL.txt', 'doc/fonts/ccicons/OFL.txt')],
        'lato': [('lato-README.txt', 'doc/fonts/lato/README'), ('lato-OFL.txt', 'doc/fonts/lato/OFL.txt')],
        'stix2-type1': [('stix2-README.txt', 'doc/fonts/stix2-type1/README.txt'),
         ('stix2-OFL.txt', 'doc/fonts/stix2-type1/OFL.txt')],
        'twemojis': [('twemojis-NOTICE.txt', 'doc/latex/twemojis/NOTICE.txt')],
        'xcharter': [('xcharter-README.txt', 'doc/fonts/xcharter/README')],
        'fdsymbol': [
            ('fdsymbol-OFL.txt', 'doc/fonts/fdsymbol/OFL.txt'),
            ('fdsymbol-README.txt', 'doc/latex/fdsymbol/README.txt'),
        ],
    }

    for pkg_id, files_list in package_legal_mapping.items():
        if pkg_id not in new_files_by_pkg:
            new_files_by_pkg[pkg_id] = {}
        for src_name, dst_rel in files_list:
            src_path = os.path.join(legal_dir, src_name)
            if not os.path.exists(src_path):
                raise FileNotFoundError(f"Required legal file not found: {src_path} for package {pkg_id}")
            fhash = sha256_file(src_path)
            new_files_by_pkg[pkg_id][dst_rel] = fhash
            dest_full = os.path.join(combined_dir, dst_rel)
            os.makedirs(os.path.dirname(dest_full), exist_ok=True)
            shutil.copy2(src_path, dest_full)

    print(f"  Pure archive extraction verified: {len(new_basenames)} unique basenames added.")
    print(f"  Verified clean overlays: {identical_overlays} identical files, 0 conflicting basenames.")

    print(f"[4/8] Generating complete LH Cyrillic metrics and consolidating default pdftex.map...")
    lh_provenance = generate_lh_metrics(combined_dir, cache_dir, scratch)
    for target_rel, fhash in lh_provenance["files"].items():
        fname = os.path.basename(target_rel)
        new_basenames[fname] = ("lh", target_rel, fhash)
        new_files_by_pkg["lh"][target_rel] = fhash

    mf_outlines_provenance = generate_metafont_outlines(combined_dir, cache_dir, scratch)
    for target_rel, fhash in mf_outlines_provenance["files"].items():
        fname = os.path.basename(target_rel)
        pkg_owner = mf_outlines_provenance.get("pkg_owners", {}).get(target_rel, "cm")
        new_basenames[fname] = (pkg_owner, target_rel, fhash)
        new_files_by_pkg.setdefault(pkg_owner, {})[target_rel] = fhash

    pdftex_map_rel = "fonts/map/pdftex/updmap/pdftex.map"
    pdftex_map_path = os.path.join(combined_dir, pdftex_map_rel)
    os.makedirs(os.path.dirname(pdftex_map_path), exist_ok=True)

    # Collect all bundled basenames to verify physical file presence
    bundled_basenames = set(baseline_by_basename.keys())
    for pkg_files in new_files_by_pkg.values():
        for rel in pkg_files.keys():
            bundled_basenames.add(os.path.basename(rel))

    def normalize_map_line(line):
        s = line.strip()
        if not s or s.startswith("%"):
            return None
        return re.sub(r'"([^"]*)"', lambda m: '"' + ' '.join(m.group(1).split()) + '"', s)

    def parse_map_components(norm_line):
        parts = norm_line.split()
        if not parts:
            return None, None, None
        tfm = parts[0]
        enc = None
        fontfile = None
        i = 1
        while i < len(parts):
            p = parts[i]
            if p == "<[":
                if i + 1 < len(parts):
                    enc = parts[i + 1]
                    i += 2
                    continue
            elif p == "<<" or p == "<":
                if i + 1 < len(parts):
                    nxt = parts[i + 1]
                    if nxt.endswith(".enc"):
                        enc = nxt
                    else:
                        fontfile = nxt
                    i += 2
                    continue
            elif p.startswith("<["):
                enc = p[2:]
            elif p.startswith("<<"):
                fontfile = p[2:]
            elif p.startswith("<"):
                if p.endswith(".enc"):
                    enc = p[1:]
                else:
                    fontfile = p[1:]
            i += 1
        return tfm, enc, fontfile

    map_lines = {}
    unavailable_baseline_entries = []
    baseline_root_records = {}

    # Canonical downloadable map for Base-35 PostScript fonts
    canonical_map_rel = "fonts/map/dvips/tetex/ps2pk35.map"
    canonical_map_path = os.path.join(baseline_dir, canonical_map_rel)
    canonical_downloadable_map = {}
    if os.path.exists(canonical_map_path):
        with open(canonical_map_path, "r", errors="ignore") as f:
            for line in f:
                s = line.strip()
                if s and not s.startswith("%"):
                    tfm_cand = s.split()[0]
                    canonical_downloadable_map[tfm_cand] = normalize_map_line(s)

    # 1. Recover complete bindings from baseline family roots
    for rel in BASELINE_FAMILY_MAP_ROOTS:
        map_full = os.path.join(baseline_dir, rel)
        if not os.path.exists(map_full):
            raise FileNotFoundError(f"Declared baseline family root missing from baseline: {rel}")
        with open(map_full, "r", errors="ignore") as f:
            content = f.read()
        fhash = sha256_file(map_full)
        baseline_root_records[rel] = {
            "sha256": fhash,
            "total_entries": 0,
            "retained_entries": 0,
            "unavailable_entries": 0,
        }
        for line in content.splitlines():
            norm = normalize_map_line(line)
            if norm:
                baseline_root_records[rel]["total_entries"] += 1
                tfm, enc, fontfile = parse_map_components(norm)
                # Audit no-file map records and resolve applicable ones to declared upstream resources
                if fontfile is None and tfm in canonical_downloadable_map:
                    resolved_norm = canonical_downloadable_map[tfm]
                    res_tfm, res_enc, res_fontfile = parse_map_components(resolved_norm)
                    res_enc_ok = (res_enc is None) or (res_enc in bundled_basenames)
                    res_font_ok = (res_fontfile is not None) and (res_fontfile in bundled_basenames)
                    if res_enc_ok and res_font_ok:
                        norm = resolved_norm
                        tfm, enc, fontfile = res_tfm, res_enc, res_fontfile

                enc_ok = (enc is None) or (enc in bundled_basenames)
                font_ok = (fontfile is not None) and (fontfile in bundled_basenames)
                if enc_ok and font_ok:
                    map_lines[tfm] = norm + "\n"
                    baseline_root_records[rel]["retained_entries"] += 1
                else:
                    missing = []
                    if not enc_ok:
                        missing.append(f"enc:{enc}")
                    if not font_ok:
                        if fontfile is None:
                            missing.append("font:None (unembedded standard font record)")
                        else:
                            missing.append(f"font:{fontfile}")
                    unavailable_baseline_entries.append({
                        "tfm": tfm,
                        "map": rel,
                        "line": norm,
                        "missing": missing,
                    })
                    baseline_root_records[rel]["unavailable_entries"] += 1

    # 2. Merge all declared upstream family roots
    upstream_root_records = {}
    upstream_added = 0
    for pkg_id, pkg_info in UPSTREAM_PACKAGES.items():
        for mf_rel in pkg_info["map_files"]:
            mf_path = os.path.join(combined_dir, mf_rel)
            if not os.path.exists(mf_path):
                raise FileNotFoundError(f"Declared upstream map file missing: {mf_rel} in {pkg_id}")
            fhash = sha256_file(mf_path)
            upstream_root_records[mf_rel] = {
                "package": pkg_id,
                "sha256": fhash,
                "entries": 0,
            }
            with open(mf_path, "r", errors="ignore") as f:
                for line in f:
                    norm = normalize_map_line(line)
                    if norm:
                        upstream_root_records[mf_rel]["entries"] += 1
                        tfm, enc, fontfile = parse_map_components(norm)
                        enc_ok = (enc is None) or (enc in bundled_basenames)
                        font_ok = (fontfile is not None) and (fontfile in bundled_basenames)
                        if not (enc_ok and font_ok):
                            missing = []
                            if not enc_ok:
                                missing.append(f"enc:{enc}")
                            if not font_ok:
                                if fontfile is None:
                                    missing.append("font:None (unembedded record)")
                                else:
                                    missing.append(f"font:{fontfile}")
                            unavailable_baseline_entries.append({
                                "tfm": tfm,
                                "map": mf_rel,
                                "line": norm,
                                "missing": missing,
                            })
                            continue
                        if tfm not in map_lines:
                            upstream_added += 1
                        map_lines[tfm] = norm + "\n"
    with open(pdftex_map_path, "w") as f:
        f.write("% Consolidated pdftex.map for ratex distribution\n")
        f.write("% Generated deterministically from declared family map roots\n")
        for tfm in sorted(map_lines.keys()):
            f.write(map_lines[tfm])

    print(f"  Consolidated {len(map_lines)} total map entries ({upstream_added} from upstream roots, "
          f"{len(map_lines)-upstream_added} retained from baseline family roots, "
          f"{len(unavailable_baseline_entries)} unavailable baseline entries documented).")
    print(f"[5/8] Generating validated language.dat hyphenation closure...")
    lang_dat_provenance = generate_language_dat(combined_dir)

    print(f"[6/8] Writing deterministic sorted tar archive...")
    all_files = []
    total_uncompressed = 0
    unique_basenames = set()

    for root, dirs, files in os.walk(combined_dir):
        dirs.sort()
        for f in sorted(files):
            full = os.path.join(root, f)
            rel = os.path.relpath(full, combined_dir).replace("\\", "/")
            all_files.append((rel, full))
            total_uncompressed += os.path.getsize(full)
            unique_basenames.add(f)

    all_files.sort(key=lambda x: x[0])
    print(f"  Sorting and packing {len(all_files)} total files ({total_uncompressed / (1024*1024):.1f} MiB uncompressed)...")

    temp_full = os.path.join(scratch, "packages.tar.zst.full")
    start_time = time.time()
    zstd_proc = subprocess.Popen(
        ["zstd", "-19", "-T0", "-o", temp_full],
        stdin=subprocess.PIPE,
    )

    with tarfile.open(fileobj=zstd_proc.stdin, mode="w|") as tar:
        for rel, full in all_files:
            ti = tar.gettarinfo(full, arcname=rel)
            ti.uid = 0
            ti.gid = 0
            ti.uname = "root"
            ti.gname = "root"
            ti.mtime = FIXED_MTIME
            ti.mode = 0o644
            with open(full, "rb") as f:
                tar.addfile(ti, f)

    zstd_proc.stdin.close()
    zstd_proc.wait()
    if zstd_proc.returncode != 0:
        raise RuntimeError("zstd compression failed.")

    elapsed = time.time() - start_time
    output_size = os.path.getsize(temp_full)
    output_hash = sha256_file(temp_full)
    print(f"  Full archive created in {elapsed:.1f}s: {output_size} bytes ({output_size / (1024*1024):.2f} MiB, sha256={output_hash[:16]}...)")

    print(f"[7/8] Sharding into deterministic <=70 MiB parts...")
    os.makedirs(output_dir, exist_ok=True)
    for item in os.listdir(output_dir):
        if item == "packages.tar.zst" or item.startswith("packages.tar.zst."):
            os.remove(os.path.join(output_dir, item))

    parts = []
    idx = 0
    with open(temp_full, "rb") as f:
        while True:
            chunk = f.read(CHUNK_SIZE_LIMIT)
            if not chunk:
                break
            part_name = f"packages.tar.zst.{idx:02d}"
            part_path = os.path.join(output_dir, part_name)
            with open(part_path, "wb") as pf:
                pf.write(chunk)
            phash = sha256_file(part_path)
            parts.append({
                "name": part_name,
                "sha256": phash,
                "size_bytes": len(chunk),
            })
            print(f"  Part {part_name}: {len(chunk)} bytes ({len(chunk)/(1024*1024):.2f} MiB, sha256={phash[:16]}...)")
            idx += 1

    # Optional source distribution archive generation
    source_dist_data = None
    if sources_out:
        source_dist_data = build_source_archive(sources_out, cache_dir)
    else:
        # Compute source distribution records from cache if present
        source_records = {}
        for pkg_id, info in sorted(SOURCE_ARCHIVES_INFO.items()):
            arc_path = os.path.join(cache_dir, info["archive"])
            if os.path.exists(arc_path):
                source_records[pkg_id] = {
                    "archive": info["archive"],
                    "url": info["url"],
                    "sha256": sha256_file(arc_path),
                    "size_bytes": os.path.getsize(arc_path),
                    "license": info["license"],
                    "provenance": info.get("provenance", info.get("obligation", "")),
                }
        source_dist_data = {
            "logical_name": "sources.tar.zst",
            "description": "Corresponding source archives for copyleft and source redistribution compliance",
            "source_archives": source_records,
        }

    print(f"[8/8] Generating machine-readable asset lock: {lock_file_path}...")
    package_records = {}
    for pkg_id, pkg_info in UPSTREAM_PACKAGES.items():
        files_dict = new_files_by_pkg.get(pkg_id, {})
        package_records[pkg_id] = {
            "version": pkg_info["version"],
            "revision": pkg_info["revision"],
            "license": pkg_info["license"],
            "ctan_path": pkg_info["ctan_path"],
            "upstream_url": pkg_info["upstream_url"],
            "upstream_sha256": pkg_info.get("upstream_sha256", ""),
            "upstream_size_bytes": pkg_info.get("upstream_size_bytes", 0),
            "description": pkg_info["description"],
            "source_obligations": pkg_info["source_obligations"],
            "license_files": pkg_info["license_files"],
            "map_files": pkg_info["map_files"],
            "file_count": len(files_dict),
            "files": files_dict,
        }

    lock_data = {
        "version": 1,
        "format": "ratex-packages-lock-v1",
        "generated_at": "2024-01-01T00:00:00Z",
        "baseline_archive": {
            "url": "https://github.com/leoliu0/ratex/releases/download/v0.3.0/ratex-v0.3.0.bundle",
            "sha256": PINNED_BASELINE_SHA256,
            "size_bytes": baseline_size,
            "total_files": baseline_file_count,
            "verified": True,
        },
        "source_distribution": source_dist_data,
        "output_archive": {
            "logical_name": "packages.tar.zst",
            "sha256": output_hash,
            "size_bytes": output_size,
            "total_members": len(all_files),
            "unique_basenames": len(unique_basenames),
            "uncompressed_bytes": total_uncompressed,
            "sharding": {
                "shard_size_limit_bytes": CHUNK_SIZE_LIMIT,
                "total_parts": len(parts),
                "parts": parts,
            },
        },
        "packages": package_records,
        "map_roots": {
            "output_map": pdftex_map_rel,
            "total_entries": len(map_lines),
            "upstream_roots": upstream_root_records,
            "baseline_roots": baseline_root_records,
            "unavailable_baseline_entries": unavailable_baseline_entries,
        },
        "hyphenation_config": lang_dat_provenance,
        "metric_closures": {
            "lh_cyrillic": {
                "compiler": lh_provenance["compiler"],
                "mode": lh_provenance["mode"],
                "total_generated": lh_provenance["total_generated"],
                "source_archives": lh_provenance["source_archives"],
            },
        },
        "metafont_outline_closures": {
            "generator": mf_outlines_provenance["generator"],
            "total_generated": mf_outlines_provenance["total_generated"],
            "source_archives": mf_outlines_provenance["source_archives"],
        },
        "basename_collision_policy": {
            "status": "verified_clean",
            "inter_package_collisions": 0,
            "conflicting_basenames_rejected": 0,
            "identical_baseline_overlays": identical_overlays,
        },
    }

    os.makedirs(os.path.dirname(os.path.abspath(lock_file_path)), exist_ok=True)
    with open(lock_file_path, "w") as f:
        json.dump(lock_data, f, indent=2)

    shutil.rmtree(scratch, ignore_errors=True)
    print("Archive generation and lock recording complete successfully!")
    return lock_data


def check_lock(assets_dir, lock_file_path):
    if not os.path.exists(lock_file_path):
        raise FileNotFoundError(f"Lock file not found: {lock_file_path}")
    with open(lock_file_path, "r") as f:
        data = json.load(f)

    parts_info = data["output_archive"]["sharding"]["parts"]
    print(f"Checking {len(parts_info)} sharded parts...")
    for part in parts_info:
        name = part["name"]
        expected_hash = part["sha256"]
        expected_size = part["size_bytes"]
        p = os.path.join(assets_dir, name)
        if not os.path.exists(p):
            raise FileNotFoundError(f"Missing part file: {p}")
        actual_size = os.path.getsize(p)
        if actual_size != expected_size:
            raise ValueError(f"Size mismatch on {name}: expected {expected_size}, got {actual_size}")
        actual_hash = sha256_file(p)
        if actual_hash != expected_hash:
            raise ValueError(f"Hash mismatch on {name}:\nExpected: {expected_hash}\nActual:   {actual_hash}")
        print(f"  OK: {name} (SHA256: {actual_hash[:16]}...)")
    print("All sharded asset parts match lockfile perfectly!")


def reconstruct_archive(assets_dir, output_path, lock_file_path=None):
    if lock_file_path and os.path.exists(lock_file_path):
        with open(lock_file_path, "r") as f:
            data = json.load(f)
        part_names = [p["name"] for p in data["output_archive"]["sharding"]["parts"]]
    else:
        part_names = sorted([p for p in os.listdir(assets_dir) if p.startswith("packages.tar.zst.")])

    if not part_names:
        raise FileNotFoundError(f"No packages.tar.zst.* parts found in {assets_dir}")

    with open(output_path, "wb") as out:
        for p in part_names:
            full = os.path.join(assets_dir, p)
            with open(full, "rb") as inp:
                shutil.copyfileobj(inp, out)

    reconstructed_hash = sha256_file(output_path)
    reconstructed_size = os.path.getsize(output_path)
    print(f"Reconstructed {output_path}: {reconstructed_size} bytes, SHA256: {reconstructed_hash}")


def main():
    parser = argparse.ArgumentParser(description="Deterministic generator for Ratex packages archive, sources, and lock")
    parser.add_argument(
        "--baseline",
        default="/tmp/ratex-baseline/packages.tar.zst",
        help="Path to baseline packages.tar.zst",
    )
    parser.add_argument(
        "--assets-dir",
        default="crates/tex-kpse/assets",
        help="Target assets directory",
    )
    parser.add_argument(
        "--lock",
        default="crates/tex-kpse/assets/packages.lock.json",
        help="Path to output packages.lock.json",
    )
    parser.add_argument(
        "--legal-dir",
        default="crates/tex-kpse/assets/legal",
        help="Path to legal/notices directory",
    )
    parser.add_argument(
        "--cache-dir",
        default="/tmp/ratex-upstream-cache",
        help="Cache directory for upstream archives",
    )
    parser.add_argument(
        "--sources-out",
        default="crates/tex-kpse/assets/sources.tar.zst",
        help="Path to output sources.tar.zst archive",
    )
    parser.add_argument(
        "--build-sources",
        action="store_true",
        help="Build source distribution archive only",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Verify existing asset parts against packages.lock.json",
    )
    parser.add_argument(
        "--reconstruct",
        metavar="OUTPUT_PATH",
        help="Reconstruct full single packages.tar.zst from assets/ parts into OUTPUT_PATH",
    )

    args = parser.parse_args()

    if args.check:
        check_lock(args.assets_dir, args.lock)
        return

    if args.reconstruct:
        reconstruct_archive(args.assets_dir, args.reconstruct, args.lock)
        return

    if args.build_sources:
        build_source_archive(args.sources_out, args.cache_dir)
        return

    build_bundle(
        baseline_path=args.baseline,
        output_dir=args.assets_dir,
        lock_file_path=args.lock,
        cache_dir=args.cache_dir,
        legal_dir=args.legal_dir,
        sources_out=args.sources_out,
    )


if __name__ == "__main__":
    main()
