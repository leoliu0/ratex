#!/usr/bin/env python3
"""Compare Biber output byte-for-byte against committed Biber 2.22 fixtures.

Examples:
  python scripts/test_biber.py --biber /path/to/rust/biber
  python scripts/test_biber.py --biber /path/to/rust/biber --committed-bcf
  python scripts/test_biber.py --regen

Normal runs never invoke the oracle. If /usr/bin/pdflatex is unavailable, they
use committed control files. --regen requires pdflatex and the pinned oracle;
only main.bcf and expected.bbl are written back to the fixture directories.
A fixture's expected-c.* (if present) is additionally compared/regenerated with
the same arguments under LC_ALL=C.
"""

import argparse
import difflib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "biber"
ORACLE = Path("/home/leo/rv-build/biber-oracle/bin/x86_64-linux/biber")
PDFLATEX = Path("/usr/bin/pdflatex")


def run(command, directory, timeout, env=None):
    result = subprocess.run(command, cwd=directory, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, timeout=timeout, env=env)
    if result.returncode:
        output = result.stdout.decode("utf-8", errors="replace")
        raise RuntimeError(f"{command[0]} exited {result.returncode}\n{output[-12000:]}")
    return result.stdout


def brace_end(data, opening):
    depth = 0
    index = opening
    while index < len(data):
        byte = data[index]
        if byte == ord("\\"):
            index += 2
            continue
        if byte == ord("{"):
            depth += 1
        elif byte == ord("}"):
            depth -= 1
            if depth == 0:
                return index
        index += 1
    return None


def unordered_options(data, keys):
    """Canonicalize only explicitly identified, oracle-generated clone fields."""
    keys = {key.encode("utf-8") for key in keys}
    field_pattern = re.compile(rb"\boptions\s*=\s*\{", re.IGNORECASE)
    edits = []
    for entry in re.finditer(rb"@[A-Za-z]+\s*\{\s*([^,\s]+)\s*,", data):
        if entry[1] not in keys:
            continue
        opening = data.find(b"{", entry.start(), entry.end())
        end = brace_end(data, opening)
        if end is None:
            continue
        index, depth = entry.end(), 1
        while index < end:
            field = field_pattern.match(data, index) if depth == 1 else None
            if field:
                start = field.end() - 1
                close = brace_end(data, start)
                if close is None or close > end:
                    break
                content = data[start + 1:close]
                parts, last, nesting = [], 0, 0
                cursor = 0
                while cursor < len(content):
                    byte = content[cursor]
                    if byte == ord("\\"):
                        cursor += 2
                        continue
                    if byte in b"{[(":
                        nesting += 1
                    elif byte in b"}])":
                        nesting -= 1
                    elif byte == ord(",") and nesting == 0:
                        parts.append(content[last:cursor].strip())
                        last = cursor + 1
                    cursor += 1
                parts.append(content[last:].strip())
                edits.append((start + 1, close, b",".join(sorted(parts))))
                index = close + 1
                continue
            if data[index] == ord("\\"):
                index += 2
                continue
            if data[index] == ord("{"):
                depth += 1
            elif data[index] == ord("}"):
                depth -= 1
            index += 1
    for start, end, replacement in reversed(edits):
        data = data[:start] + replacement + data[end:]
    return data


def stdout_equivalence(data):
    banner = rb"(?m)^(INFO - This is )(?:TeXres )?Biber [^\s\r\n]+"
    data = re.sub(banner, rb"\1Biber <version>", data)
    lines, result, unordered = data.splitlines(keepends=True), [], []
    for line in lines:
        if line.startswith(b"INFO - Overriding locale '"):
            unordered.append(line)
        else:
            result.extend(sorted(unordered))
            unordered.clear()
            result.append(line)
    result.extend(sorted(unordered))
    return b"".join(result)


def compare(expected, actual, options=None, comparison=None):
    if expected == actual:
        return True
    canonical_expected, canonical_actual = expected, actual
    approved = []
    if comparison and (keys := comparison.get("unordered_options_entries")):
        canonical_expected = unordered_options(canonical_expected, keys)
        canonical_actual = unordered_options(canonical_actual, keys)
        approved.append("generated clone OPTIONS order")
    if options and options.get("output_file") == "-" and not options.get("quiet"):
        canonical_expected = stdout_equivalence(canonical_expected)
        canonical_actual = stdout_equivalence(canonical_actual)
        approved.append("stdout version banner / Perl-hash override INFO order")
    if approved and canonical_expected == canonical_actual:
        print("  approved equivalence: " + "; ".join(approved))
        return True
    common = min(len(expected), len(actual))
    offset = next((i for i in range(common) if expected[i] != actual[i]), common)
    print(f"  first differing byte: {offset}; expected {len(expected)} bytes, "
          f"actual {len(actual)} bytes")
    print(f"  expected bytes: {expected[offset:offset + 80]!r}")
    print(f"  actual bytes:   {actual[offset:offset + 80]!r}")
    diff = difflib.unified_diff(
        expected.decode("utf-8", errors="replace").splitlines(keepends=True),
        actual.decode("utf-8", errors="replace").splitlines(keepends=True),
        fromfile="expected.bbl", tofile="actual.bbl")
    for index, line in enumerate(diff):
        if index == 250:
            print("  ... diff truncated after 250 lines")
            break
        print(line, end="" if line.endswith("\n") else "\n")
    return False


def resolve_binary(value):
    found = shutil.which(value)
    # absolute(), not resolve(): a `biber -> texres` symlink selects the
    # personality by its own name.
    candidate = Path(found or value).absolute()
    if not candidate.is_file():
        raise RuntimeError(f"Biber executable not found: {value}")
    return candidate


def fixture_options(fixture):
    path = fixture / "options.json"
    return json.loads(path.read_text()) if path.is_file() else {}


def option_args(options):
    result = []
    for key, value in options.items():
        name = "--" + key.replace("_", "-")
        if isinstance(value, bool) and key not in ("sortcase", "sortupper"):
            if value:
                result.append(name)
        else:
            result.append(name + "=" + (str(value).lower() if isinstance(value, bool) else str(value)))
    return result


def warnings(log):
    # Perl's converted datasource filename contains a random temporary path.
    # Compare warning content and line numbers, not that nondeterministic path.
    return [re.sub(r"^(BibTeX subsystem: ).*?\.utf8(, line \d+)", r"\1<converted datasource>\2",
                   line.split("WARN - ", 1)[1])
            for line in log.splitlines() if "WARN - " in line]


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--biber", help="Biber executable to compare (required unless --regen)")
    parser.add_argument("--regen", action="store_true", help="Regenerate BCF/BBL with pinned oracle")
    parser.add_argument("--committed-bcf", action="store_true", help="Skip pdflatex and use committed BCF")
    parser.add_argument("--case", action="append", default=[], help="Case name or glob; repeatable")
    parser.add_argument("--timeout", type=int, default=120, help="Per-command timeout in seconds")
    args = parser.parse_args()
    if args.regen and args.committed_bcf:
        parser.error("--regen requires freshly generated BCF; omit --committed-bcf")
    if not args.regen and not args.biber:
        parser.error("--biber is required unless --regen")
    if args.regen and not PDFLATEX.is_file():
        parser.error(f"--regen requires {PDFLATEX}")
    try:
        binary = resolve_binary(str(ORACLE) if args.regen else args.biber)
        if args.regen:
            version = run([str(binary), "--version"], FIXTURES, args.timeout)
            if version.strip() != b"biber version: 2.22":
                raise RuntimeError(f"Expected Biber 2.22 oracle, got {version!r}")
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        parser.error(str(error))
    cases = sorted(path for path in FIXTURES.iterdir() if path.is_dir()
                   and (not args.case or any(path.match(pattern) for pattern in args.case)))
    if not cases:
        parser.error("No matching fixtures")
    use_tex = not args.committed_bcf and PDFLATEX.is_file()
    if not use_tex:
        print("Using committed BCF files (no pdflatex invocation).")
    failed = 0
    for fixture in cases:
        try:
            with tempfile.TemporaryDirectory(prefix=f"biber-{fixture.name}-") as tmp:
                work = Path(tmp)
                for source in fixture.iterdir():
                    if source.suffix in (".tex", ".bib", ".bcf", ".xml", ".bltxml", ".conf", ".dbx"):
                        shutil.copyfile(source, work / source.name)
                options = fixture_options(fixture)
                comparison_path = fixture / "comparison.json"
                comparison = json.loads(comparison_path.read_text()) if comparison_path.is_file() else {}
                tool = bool(options.get("tool"))
                if not tool and use_tex and (work / "main.tex").is_file():
                    (work / "main.bcf").unlink(missing_ok=True)
                    run([str(PDFLATEX), "-interaction=nonstopmode", "-halt-on-error",
                         "-no-shell-escape", "main.tex"], work, args.timeout)
                if not tool and not (work / "main.bcf").is_file():
                    raise RuntimeError("Missing main.bcf")
                if not tool and options.get("validate_datamodel"):
                    # Retain the option for the offline Rust corpus API too.
                    bcf = (work / "main.bcf").read_text()
                    option = '<bcf:options component="biber" type="global"><bcf:option type="singlevalued"><bcf:key>validate_datamodel</bcf:key><bcf:value>1</bcf:value></bcf:option></bcf:options>'
                    (work / "main.bcf").write_text(bcf.replace("</bcf:controlfile>", option + "</bcf:controlfile>"))
                if tool:
                    source = "main.bltxml" if options.get("input_format") == "biblatexml" else "main.bib"
                    suffix = "bltxml" if options.get("output_format") == "biblatexml" else "bib"
                    output = options.get("output_file", f"main_bibertool.{suffix}")
                    logfile = f"{source}.blg"
                    expected = fixture / f"expected.{suffix}"
                else:
                    source = "main"
                    output = options.get("output_file", "main.bbl")
                    logfile = "main.blg"
                    expected = fixture / "expected.bbl"
                if directory := options.get("output_directory"):
                    (work / directory).mkdir(parents=True, exist_ok=True)
                    if output != "-":
                        output = str(Path(directory) / output)
                    logfile = str(Path(directory) / logfile)
                # expected-c.* is the same case run under LC_ALL=C (Perl's /l
                # and locale-dependent behaviour); expected.* uses the caller's locale.
                variants = [(expected, None)]
                if (variant := expected.with_name(expected.name.replace("expected.", "expected-c.", 1))).is_file():
                    variants.append((variant, {**os.environ, "LC_ALL": "C"}))
                for expected, env in variants:
                    label = fixture.name if env is None else f"{fixture.name} [LC_ALL=C]"
                    stdout = run([str(binary), *option_args(options), source], work, args.timeout, env)
                    actual_warnings = warnings((work / logfile).read_text())
                    actual = stdout if output == "-" else (work / output).read_bytes()
                    warning_path = fixture / ("expected.warnings" if env is None else "expected-c.warnings")
                    if args.regen:
                        if not tool and env is None:
                            (fixture / "main.bcf").write_bytes((work / "main.bcf").read_bytes())
                        expected.write_bytes(actual)
                        if warning_path.is_file() or actual_warnings:
                            warning_path.write_text("\n".join(actual_warnings) + ("\n" if actual_warnings else ""))
                        print(f"REGEN {label}")
                    elif compare(expected.read_bytes(), actual, options, comparison):
                        if warning_path.is_file() and sorted(warning_path.read_text().splitlines()) != sorted(actual_warnings):
                            failed += 1
                            print(f"FAIL {label}: warnings differ")
                            print(f"  expected: {warning_path.read_text().splitlines()!r}")
                            print(f"  actual:   {actual_warnings!r}")
                            break
                        else:
                            print(f"PASS {label}")
                    else:
                        failed += 1
                        print(f"FAIL {label}")
                        break
        except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
            failed += 1
            print(f"ERROR {fixture.name}: {error}")
    print(f"{len(cases) - failed}/{len(cases)} cases "
          f"{'regenerated' if args.regen else 'passed'}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
