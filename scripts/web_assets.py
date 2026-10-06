#!/usr/bin/env python3
"""Split the package archive of a `lazy-assets` WebAssembly build into
content-addressed files for the website (see scripts/build-web.sh).

tex-kpse's `external-packages` build leaves `packages.bin` (the concatenated
zstd frames) and `package_chunks.bin` (u32 offset, length, decoded length per
chunk) in its OUT_DIR. Each chunk is verified, recompressed at a high zstd
level (any frame decoding to the same bytes is valid for the runtime) and
written as `packages/<sha256-prefix>.bin`. The manifest lists the files in
chunk-index order. Recompression results are memoized next to the manifest,
so a rebuild only touches changed chunks.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
import shutil
import struct
import sys
from compression import zstd
from pathlib import Path

LEVEL = 19


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()[:16]


def recompress(job):
    raw, decoded_length = job
    data = zstd.decompress(raw)
    if len(data) != decoded_length:
        raise ValueError(f"chunk decodes to {len(data)} bytes, expected {decoded_length}")
    best = zstd.compress(data, level=LEVEL)
    return best if len(best) < len(raw) else raw


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kpse-out", required=True, type=Path, help="tex-kpse OUT_DIR")
    parser.add_argument("--formats", required=True, type=Path, help="directory with *.fmt.zst")
    parser.add_argument("--dist", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path, help="JSON summary to write")
    args = parser.parse_args()

    table = (args.kpse_out / "package_chunks.bin").read_bytes()
    chunks = list(struct.iter_unpack("<III", table))
    packages = args.kpse_out / "packages.bin"
    out = args.dist / "packages"
    out.mkdir(parents=True, exist_ok=True)
    # Next to the manifest, outside the published site.
    memo_path = args.manifest.with_name(args.manifest.stem + "-memo.json")
    memo = json.loads(memo_path.read_text()) if memo_path.exists() else {}

    names = [None] * len(chunks)
    sizes = [0] * len(chunks)
    pending = []
    with open(packages, "rb") as archive:
        for index, (offset, length, decoded) in enumerate(chunks):
            archive.seek(offset)
            raw = archive.read(length)
            if len(raw) != length:
                sys.exit(f"packages.bin is truncated at chunk {index}")
            key = digest(raw)
            name = memo.get(key)
            if name and (out / f"{name}.bin").exists():
                names[index] = name
                sizes[index] = (out / f"{name}.bin").stat().st_size
            else:
                pending.append((index, key, raw, decoded))

    if pending:
        print(f"web_assets: recompressing {len(pending)} of {len(chunks)} chunks", file=sys.stderr)
        with concurrent.futures.ProcessPoolExecutor(max_workers=os.cpu_count()) as pool:
            results = pool.map(recompress, ((raw, decoded) for _, _, raw, decoded in pending), chunksize=8)
            for (index, key, _, _), data in zip(pending, results):
                name = digest(data)
                path = out / f"{name}.bin"
                if not path.exists():
                    path.write_bytes(data)
                memo[key] = name
                names[index] = name
                sizes[index] = len(data)
        memo_path.write_text(json.dumps(memo))

    # Drop files no longer referenced by this build.
    live = set(names)
    for path in out.glob("*.bin"):
        if path.stem not in live:
            path.unlink()

    formats = {}
    (args.dist / "formats").mkdir(exist_ok=True)
    for path in (args.dist / "formats").glob("*.bin"):
        path.unlink()
    # LuaLaTeX is not offered: luaotfload needs a writable cache directory,
    # which the WebAssembly sandbox lacks.
    for engine, file in [("xelatex", "xelatex.fmt.zst")]:
        data = (args.formats / file).read_bytes()
        target = f"formats/{engine}-{digest(data)}.bin"
        shutil.copyfile(args.formats / file, args.dist / target)
        formats[engine] = {"url": target, "size": len(data)}

    args.manifest.write_text(json.dumps({
        "chunks": names,
        "sizes": sizes,
        "formats": formats,
    }))
    total = sum(sizes)
    print(f"web_assets: {len(chunks)} chunks, {total / 1e6:.1f} MB "
          f"(build archive {sum(c[1] for c in chunks) / 1e6:.1f} MB)", file=sys.stderr)


if __name__ == "__main__":
    main()
