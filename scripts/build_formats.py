#!/usr/bin/env python3
"""Regenerate the embedded LaTeX formats (crates/tex-cli/assets/*.fmt.zst).

Each format is produced the way users do it: the single `texres` binary,
invoked under the engine's name, runs `<engine> -ini <engine>.ini` in an empty
directory with hermetic resource lookup (embedded packages only) and a fixed
`SOURCE_DATE_EPOCH`, so the dump is a pure function of the sources. The raw
dump is recompressed with `zstd -19` in 1 MiB frames that the engine decodes
in parallel (see `compress_frames`) and written next to the other assets.

Usage:
  scripts/build_formats.py [--binary PATH] [--engine pdflatex|xelatex|lualatex ...]
                           [--check] [--epoch N] [--keep DIR]

`--check` rebuilds each format twice and compares the two dumps (determinism)
and the result against the committed asset; it never writes into the tree.
Without `--check` the assets are rewritten. Commit regenerated assets in a
commit of their own ("regenerate formats (drop at merge)"): they change on
every dumped-layout change and conflict on every merge.
"""

import argparse
import contextlib
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
    "xelatex": "xelatex.fmt.zst",
    "lualatex": "lualatex.fmt.zst",
}
# extra `-ini` options per engine, as TeX Live's fmtutil.cnf passes them:
# pdfTeX formats carry the cp227 character translation (printable 8-bit
# characters); the XeTeX format is made in e-TeX mode
# (`xelatex xetex language.dat -etex xelatex.ini`).
EXTRA_ARGS = {
    "pdflatex": ["-translate-file=cp227.tcx"],
    "xelatex": ["-etex"],
}
DEFAULT_EPOCH = "1700000000"
ZSTD_MAGIC = b"\x28\xb5\x2f\xfd"
# A zstd skippable frame holding the frame index of a format that is stored
# as independently decodable frames (tex_core::format::compress_format_frames).
SKIPPABLE_INDEX_MAGIC = b"\x5e\x2a\x4d\x18"
FRAME_INDEX_TAG = b"TeXresFI"
FORMAT_FRAME_BYTES = 1 << 20


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def zstd(args, data: bytes) -> bytes:
    exe = shutil.which("zstd")
    if exe is None:
        sys.exit("build_formats: the `zstd` command line tool is required")
    return subprocess.run([exe, "-q", *args], input=data, check=True,
                          stdout=subprocess.PIPE).stdout


def compress_frames(raw: bytes) -> bytes:
    """`raw` as texres stores a compressed format: a skippable frame with
    the compressed and decompressed size of every frame, then one `zstd -19`
    frame per FORMAT_FRAME_BYTES, which the engine decodes in parallel. Any
    zstd decoder reads the whole as `raw`."""
    frames = [(zstd(["-19", "-c"], raw[i:i + FORMAT_FRAME_BYTES]),
               len(raw[i:i + FORMAT_FRAME_BYTES]))
              for i in range(0, len(raw), FORMAT_FRAME_BYTES)]
    index = FRAME_INDEX_TAG + len(frames).to_bytes(4, "little") + b"".join(
        len(frame).to_bytes(4, "little") + size.to_bytes(4, "little")
        for frame, size in frames)
    return (SKIPPABLE_INDEX_MAGIC + len(index).to_bytes(4, "little") + index
            + b"".join(frame for frame, _ in frames))


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
        [str(program), "-ini", "-interaction=nonstopmode",
         *EXTRA_ARGS.get(engine, []), f"{engine}.ini"],
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
    compressed = data.startswith(ZSTD_MAGIC) or data.startswith(SKIPPABLE_INDEX_MAGIC)
    return zstd(["-d", "-c"], data) if compressed else data


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--binary", type=Path,
                        default=REPO / "target" / "release" / "texres",
                        help="the texres binary (default: target/release/texres)")
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
                 "(cargo build --release --locked --bin texres)")
    failures = []
    if args.keep:
        args.keep.mkdir(parents=True, exist_ok=True)
    for engine in args.engine or sorted(FORMATS):
        asset = ASSETS / FORMATS[engine]
        prefix = f"formats-{engine}-"
        workdir = (contextlib.nullcontext(tempfile.mkdtemp(prefix=prefix, dir=args.keep))
                   if args.keep else tempfile.TemporaryDirectory(prefix=prefix))
        with workdir as scratch:
            first = dump_format(binary, engine, args.epoch, Path(scratch) / "a")
            compressed = compress_frames(first)
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
