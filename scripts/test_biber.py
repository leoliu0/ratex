#!/usr/bin/env python3
"""Compare Biber output byte-for-byte against committed Biber 2.22 fixtures.

Examples:
  python scripts/test_biber.py --biber /path/to/rust/biber
  python scripts/test_biber.py --biber /path/to/rust/biber --committed-bcf
  python scripts/test_biber.py --regen

Normal runs never invoke the oracle. If /usr/bin/pdflatex is unavailable, they
use committed control files. --regen requires pdflatex and the pinned oracle;
only main.bcf and expected.bbl are written back to the fixture directories.
"""

import argparse
import difflib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "biber"
ORACLE = Path("/home/leo/rv-build/biber-oracle/bin/x86_64-linux/biber")
PDFLATEX = Path("/usr/bin/pdflatex")


def run(command, directory, timeout):
    result = subprocess.run(command, cwd=directory, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, timeout=timeout)
    if result.returncode:
        output = result.stdout.decode("utf-8", errors="replace")
        raise RuntimeError(f"{command[0]} exited {result.returncode}\n{output[-12000:]}")
    return result.stdout


def compare(expected, actual):
    if expected == actual:
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
    candidate = Path(found or value).resolve()
    if not candidate.is_file():
        raise RuntimeError(f"Biber executable not found: {value}")
    return candidate


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
                    if source.suffix in (".tex", ".bib", ".bcf"):
                        shutil.copyfile(source, work / source.name)
                if use_tex:
                    (work / "main.bcf").unlink(missing_ok=True)
                    run([str(PDFLATEX), "-interaction=nonstopmode", "-halt-on-error",
                         "-no-shell-escape", "main.tex"], work, args.timeout)
                if not (work / "main.bcf").is_file():
                    raise RuntimeError("Missing main.bcf")
                run([str(binary), "main"], work, args.timeout)
                actual = (work / "main.bbl").read_bytes()
                if args.regen:
                    (fixture / "main.bcf").write_bytes((work / "main.bcf").read_bytes())
                    (fixture / "expected.bbl").write_bytes(actual)
                    print(f"REGEN {fixture.name}")
                elif compare((fixture / "expected.bbl").read_bytes(), actual):
                    print(f"PASS {fixture.name}")
                else:
                    failed += 1
                    print(f"FAIL {fixture.name}")
        except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
            failed += 1
            print(f"ERROR {fixture.name}: {error}")
    print(f"{len(cases) - failed}/{len(cases)} cases "
          f"{'regenerated' if args.regen else 'passed'}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
