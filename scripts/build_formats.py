#!/usr/bin/env python3
"""Regenerate the embedded LaTeX formats (crates/tex-cli/assets/*.fmt.zst).

Each format is produced the way users do it: the single `ratex` binary,
invoked under the engine's name, runs `<engine> -ini <engine>.ini` in an empty
directory with hermetic resource lookup (embedded packages only) and a fixed
`SOURCE_DATE_EPOCH`, so the dump is a pure function of the sources. The raw
dump is recompressed with `zstd -19` and written next to the other assets.

Usage:
  scripts/build_formats.py [--binary PATH] [--engine pdflatex|lualatex ...]
                           [--check] [--epoch N] [--keep DIR]

`--check` rebuilds each format twice and compares the two dumps (determinism)
and the result against the committed asset; it never writes into the tree.
Without `--check` the assets are rewritten. Commit regenerated assets in a
commit of their own ("regenerate formats (drop at merge)"): they change on
every dumped-layout change and conflict on every merge.
"""

import argparse
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
ASSETS = REPO / "crates" / "tex-cli" / "assets"
# engine program name -> embedded asset
FORMATS = {
    "pdflatex": "default.fmt.zst",
    "lualatex": "lualatex.fmt.zst",
}
DEFAULT_EPOCH = "1700000000"
ZSTD_MAGIC = b"\x28\xb5\x2f\xfd"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def zstd(args, data: bytes) -> bytes:
    exe = shutil.which("zstd")
    if exe is None:
        sys.exit("build_formats: the `zstd` command line tool is required")
    return subprocess.run([exe, "-q", *args], input=data, check=True,
                          stdout=subprocess.PIPE).stdout


def dump_format(binary: Path, engine: str, epoch: str, work: Path) -> bytes:
    """Run `<engine> -ini <engine>.ini` and return the raw format bytes."""
    work.mkdir(parents=True, exist_ok=True)
    program = work / engine
    if program.is_symlink() or program.exists():
        program.unlink()
    program.symlink_to(binary)
    env = dict(os.environ)
    env.update({
        "SOURCE_DATE_EPOCH": epoch,
        "FORCE_SOURCE_DATE": "1",
        "TEX_RS_HERMETIC": "1",
        "TEX_RS_CACHE_DIR": str(work / "cache"),
    })
    result = subprocess.run(
        [str(program), "-ini", "-interaction=nonstopmode", f"{engine}.ini"],
        cwd=work, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, timeout=1800)
    fmt = work / f"{engine}.fmt"
    if result.returncode != 0 or not fmt.is_file():
        sys.stdout.write(result.stdout[-4000:])
        sys.exit(f"build_formats: `{engine} -ini {engine}.ini` failed "
                 f"(exit {result.returncode}); no {fmt.name} written")
    data = fmt.read_bytes()
    # `-ini` writes a quickly compressed dump; the asset is the same
    # payload recompressed at the highest ratio.
    return zstd(["-d", "-c"], data) if data.startswith(ZSTD_MAGIC) else data


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--binary", type=Path,
                        default=REPO / "target" / "release" / "ratex",
                        help="the ratex binary (default: target/release/ratex)")
    parser.add_argument("--engine", action="append", choices=sorted(FORMATS),
                        help="format to build (default: all)")
    parser.add_argument("--epoch", default=DEFAULT_EPOCH,
                        help=f"SOURCE_DATE_EPOCH (default {DEFAULT_EPOCH})")
    parser.add_argument("--check", action="store_true",
                        help="verify determinism and the committed assets")
    parser.add_argument("--keep", type=Path,
                        help="keep the working directories under this path")
    args = parser.parse_args()

    binary = args.binary.resolve()
    if not binary.is_file():
        sys.exit(f"build_formats: {binary} does not exist; build it first "
                 "(cargo build --release --locked --bin ratex)")
    failures = []
    for engine in args.engine or sorted(FORMATS):
        asset = ASSETS / FORMATS[engine]
        with tempfile.TemporaryDirectory(prefix=f"formats-{engine}-",
                                         dir=args.keep) as scratch:
            first = dump_format(binary, engine, args.epoch, Path(scratch) / "a")
            compressed = zstd(["-19", "-c"], first)
            print(f"{engine}: {len(first)} bytes -> {len(compressed)} "
                  f"zstd, sha256 {sha256(first)[:16]}")
            if not args.check:
                asset.write_bytes(compressed)
                continue
            second = dump_format(binary, engine, args.epoch, Path(scratch) / "b")
            if first != second:
                failures.append(f"{engine}: two runs produced different dumps")
            if not asset.is_file():
                failures.append(f"{engine}: {asset} is missing")
            elif zstd(["-d", "-c"], asset.read_bytes()) != first:
                failures.append(f"{engine}: {asset.relative_to(REPO)} is stale")
    if failures:
        sys.exit("build_formats: " + "; ".join(failures))
    if args.check:
        print("build_formats: formats are deterministic and up to date")


if __name__ == "__main__":
    main()
