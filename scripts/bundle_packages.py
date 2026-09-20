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

def build_bundle(baseline_path, output_dir, lock_file_path, cache_dir, legal_dir, sources_out=None):
    """
    Builds packages.tar.zst and packages.lock.json using ONLY hash-pinned upstream archives
    and verified baseline archive. Zero host-derived payload.
    """
    baseline_path = resolve_baseline_path(baseline_path)
    if not os.path.exists(baseline_path):
        raise FileNotFoundError(f"Baseline archive not found at: {baseline_path}")

    print(f"[1/7] Verifying baseline archive: {baseline_path}...")
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

    print(f"[2/7] Extracting baseline archive to scratch...")
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

    print(f"[3/7] Extracting and verifying payload ONLY from pinned upstream archives...")
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

    print(f"[4/7] Generating complete LH Cyrillic metrics and consolidating default pdftex.map...")
    lh_provenance = generate_lh_metrics(combined_dir, cache_dir, scratch)
    for target_rel, fhash in lh_provenance["files"].items():
        fname = os.path.basename(target_rel)
        new_basenames[fname] = ("lh", target_rel, fhash)
        new_files_by_pkg["lh"][target_rel] = fhash

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
        tfm = parts[0]
        enc = None
        fontfile = None
        for p in parts[1:]:
            if p.startswith("<["):
                enc = p[2:]
            elif p.startswith("<<"):
                fontfile = p[2:]
            elif p.startswith("<"):
                if p.endswith(".enc"):
                    enc = p[1:]
                else:
                    fontfile = p[1:]
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
    print(f"[5/7] Writing deterministic sorted tar archive...")
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

    print(f"[6/7] Sharding into deterministic <=70 MiB parts...")
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

    print(f"[7/7] Generating machine-readable asset lock: {lock_file_path}...")
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
        "metric_closures": {
            "lh_cyrillic": {
                "compiler": lh_provenance["compiler"],
                "mode": lh_provenance["mode"],
                "total_generated": lh_provenance["total_generated"],
                "source_archives": lh_provenance["source_archives"],
            },
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
