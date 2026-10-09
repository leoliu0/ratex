#!/usr/bin/env python3
"""Generate crates/tex-format/src/packages/screened.rs.

`texres fmt` formats a project only when every class and package it loads is
known not to read document text verbatim in ways the formatter cannot see
(crates/tex-format/src/packages.rs). This script lists the TeX Live classes
and packages that cannot do so, found mechanically: a `.sty` or `.cls` file
is listed when neither it nor any file it loads (`\\RequirePackage`,
`\\usepackage`, `\\LoadClass`, `\\input` and friends, followed through the
whole tree; files of the hand-checked lists in packages.rs are trusted as
they are) contains, outside comments:

- a control sequence whose name contains `catcode`, `makeother`,
  `dospecials`, `sanitize`, `verbatim`, `obey`, `listing`, `minted`,
  `rescan`, `scantokens`, `endlinechar`, `everyeof`, `shortverb`, `cctab`,
  `alltt`, `filecontents`, `urldef`, `urlcommand`, `docinput`, `directlua`,
  `inputlineno`, ... (see RISKY_SUBSTRINGS), contains `verb` (other than in
  `verbose` and `overbrace`), starts with `lst`, `FV@`, `fvset` or
  `Url@`, or is `\\url`, `\\href`, `\\index`, `\\comment` and the like;
- `\\begin{...}` of a verbatim-like environment, an xparse `v` argument,
  a Lua input-buffer callback, a catcode table, tcolorbox's `saveto`;
- a load whose name is computed (`\\RequirePackage{\\x}`, `\\input{#1}`);
- a load of a file whose effect the formatter applies only when the project
  loads it itself (endfloat, showlabels, the ltxdoc/l3doc classes).

Every way of reading text with other category codes needs one of these, so
a file without them reads its arguments and environment bodies as TeX
does, and the formatter's token check covers them. The patterns err on the
side of listing too little: a flagged package is simply not listed (it can
be checked by hand and added to packages.rs, or named in `known-packages`).

usage: generate_fmt_screened_packages.py [TEXMF-DIST]   (default: kpsewhich)
"""

import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PACKAGES_RS = ROOT / "crates/tex-format/src/packages.rs"
OUT = ROOT / "crates/tex-format/src/packages/screened.rs"

RISKY_SUBSTRINGS = [
    "catcode", "makeother", "dospecials", "verbatim", "rescan", "scantokens",
    "scantextokens", "endlinechar", "everyeof", "shortverb", "cctab", "obey",
    "listing", "filecontents", "urldef", "urlcommand", "docinput", "directlua",
    "luaexec", "luadirect", "ltxexample", "scontents", "newenvsc", "pythontex",
    "processcomment", "excludecomment", "includecomment", "specialcomment",
    "sanitize", "minted", "inputlineno", "alltt",
]
RISKY_NAMES = {"verb", "url", "href", "nolinkurl", "index", "comment", "endcomment", "mint"}
RISKY_PREFIX = re.compile(r"(verb|@verb|Verb|lst|FV@|fvset|newmint|Url@|@sverb|@xverb)")
RISKY_TEXT = re.compile(
    r"process_input_buffer|setcatcode|catcodetable|saveto|savelowerto|\.lua\b"
    r"|\\begin\s*\{\s*\w*(?:erbatim|isting|inted|omment|ontents|alltt|xample)\w*\*?\s*\}"
)
XPARSE_SPEC = re.compile(
    r"DocumentCommand\s*(?:\{[^{}]*\}|\\[A-Za-z@_:]+)\s*\{([^{}]*(?:\{[^{}]*\}[^{}]*)*)\}"
)
LOAD = re.compile(
    r"\\(RequirePackage|RequirePackageWithOptions|usepackage|LoadClass|LoadClassWithOptions)"
    r"\s*(?:\[[^\]]*\]\s*)*\{([^}]*)\}"
)
INPUT = re.compile(
    r"\\(?:input|@input|@input@|InputIfFileExists|file_input:n|IfFileExists|@obsoletefile"
    r"|file_if_exist_input:n)\s*\{([^}]*)\}|\\input\s+([A-Za-z0-9_.\-/]+)"
)
CONTROL_WORD = re.compile(r"\\([A-Za-z@_:]+)")
COMMENT = re.compile(r"(?<!\\)%.*")
# Loading these changes how the project is formatted only when the project
# loads them itself (format.rs), so a file that loads them is not listed.
EFFECT = {"endfloat.sty", "showlabels.sty", "ltxdoc.cls", "ltxguide.cls", "source2edoc.cls",
          "l3doc.cls", "l3in2edoc.cls"}
SCANNED = (".sty", ".cls", ".def", ".cfg", ".tex", ".clo", ".fd", ".ldf")


def risky(name):
    lower = name.lower()
    if any(s in lower for s in RISKY_SUBSTRINGS) or name in RISKY_NAMES:
        return True
    if RISKY_PREFIX.match(name):
        return True
    return "verb" in lower and not any(x in lower for x in ("verbose", "overbrace", "overbracket"))


def scan(paths):
    """Names a file (all files of that name) loads, and whether it is risky."""
    deps, bad = set(), False
    for path in paths:
        text = COMMENT.sub("", path.read_text(encoding="latin-1"))
        for m in LOAD.finditer(text):
            ext = ".cls" if "Class" in m.group(1) else ".sty"
            for name in (n.strip() for n in m.group(2).split(",")):
                if re.search(r"[\\#]", name):
                    bad = True
                elif name:
                    deps.add(name + ext)
        for m in INPUT.finditer(text):
            name = (m.group(1) or m.group(2) or "").strip()
            if re.search(r"[\\#]", name):
                bad = True
            elif name:
                base = os.path.basename(name)
                deps.add(base if "." in base else base + ".tex")
        if any(risky(m.group(1)) for m in CONTROL_WORD.finditer(text)) or RISKY_TEXT.search(text):
            bad = True
        for m in XPARSE_SPEC.finditer(text):
            if "v" in re.sub(r"\{[^{}]*\}", "", m.group(1)):
                bad = True
    return deps, bad


def hand_checked():
    text = PACKAGES_RS.read_text()
    classes = text[text.index("const CLASSES"):text.index("const PACKAGES")]
    packages = text[text.index("const PACKAGES"):text.index("pub(crate) fn known_class")]
    return ({c + ".cls" for c in re.findall(r'"([^"]+)"', classes)}
            | {p + ".sty" for p in re.findall(r'"([^"]+)"', packages)})


def main():
    dist = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(
        subprocess.run(["kpsewhich", "-var-value", "TEXMFDIST"], capture_output=True,
                       text=True, check=True).stdout.strip())
    files = {}
    for root, _, names in os.walk(dist / "tex"):
        for name in names:
            if name.endswith(SCANNED):
                files.setdefault(name, []).append(Path(root) / name)
    trusted = hand_checked()
    facts = {name: scan(paths) for name, paths in files.items()}

    def clean(name):
        seen, stack = set(), [name]
        while stack:
            f = stack.pop()
            if f in seen:
                continue
            seen.add(f)
            if f != name and f in EFFECT:
                return False
            if (f != name and f in trusted) or f not in facts:
                continue
            deps, bad = facts[f]
            if bad:
                return False
            stack.extend(deps)
        return True

    listed = sorted(n for n in files if n.endswith((".sty", ".cls")) and n not in trusted
                    and re.fullmatch(r"[A-Za-z0-9._+\-]+", n) and clean(n))
    classes = [n[:-4] for n in listed if n.endswith(".cls")]
    packages = [n[:-4] for n in listed if n.endswith(".sty")]

    def rust_list(name, items):
        body = "".join(f'    "{i}",\n' for i in sorted(set(items)))
        return f"pub(super) const {name}: &[&str] = &[\n{body}];\n"

    OUT.parent.mkdir(exist_ok=True)
    OUT.write_text(
        "//! TeX Live classes and packages screened mechanically for verbatim\n"
        "//! material. Generated by scripts/generate_fmt_screened_packages.py from\n"
        "//! TeX Live 2026; do not edit.\n\n"
        "/// Classes (sorted).\n" + rust_list("CLASSES", classes)
        + "\n/// Packages (sorted).\n" + rust_list("PACKAGES", packages)
    )
    print(f"{len(classes)} classes, {len(packages)} packages -> {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
