#!/usr/bin/env python3
"""Fetch the public (real-world) half of the 100-document benchmark corpus.

Sources are listed in public_manifest.json: arXiv e-prints (pinned by versioned
id) and GitHub repositories (pinned by commit). Third-party sources are NOT
committed; they are downloaded into a cache, unpacked under
scripts/bench/corpus_public/<name>/ (git-ignored) and verified against a
sha256 over the unpacked tree (sorted "path NUL sha256(file)" lines), so the
check does not depend on how the archive happened to be compressed.

    python3 scripts/bench/fetch_public.py              # fetch + verify all
    python3 scripts/bench/fetch_public.py --pin        # fill in missing tree hashes
    python3 scripts/bench/fetch_public.py --cache DIR --only arxiv_bert,gh_tufte_book

Needs network access to arxiv.org and codeload.github.com.
"""

import argparse
import gzip
import hashlib
import io
import json
import os
import shutil
import sys
import tarfile
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
MANIFEST = HERE / "public_manifest.json"
DEST = HERE / "corpus_public"
UA = "texres-bench/1.0 (https://github.com; benchmark corpus fetch)"


def url_of(d):
    if d["kind"] == "arxiv":
        return f"https://arxiv.org/e-print/{d['id']}"
    return f"https://codeload.github.com/{d['repo']}/tar.gz/{d['commit']}"


def download(d, cache: Path) -> bytes:
    cache.mkdir(parents=True, exist_ok=True)
    f = cache / (d["name"] + ".src")
    if f.exists() and f.stat().st_size:
        return f.read_bytes()
    for attempt in range(4):
        try:
            req = urllib.request.Request(url_of(d), headers={"User-Agent": UA})
            data = urllib.request.urlopen(req, timeout=180).read()
            if d["kind"] == "arxiv":
                time.sleep(3)  # be polite to arXiv
            f.write_bytes(data)
            return data
        except Exception as e:  # noqa: BLE001
            print(f"  {d['name']}: download attempt {attempt + 1} failed: {e}", file=sys.stderr)
            time.sleep(5 * (attempt + 1))
    raise SystemExit(f"cannot download {url_of(d)}")


def unpack(d, data: bytes, out: Path):
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    strip = d["kind"] == "github"
    try:
        with tarfile.open(fileobj=io.BytesIO(data)) as tf:
            for m in tf.getmembers():
                if strip:
                    parts = m.name.split("/", 1)
                    if len(parts) < 2 or not parts[1]:
                        continue
                    m.name = parts[1]
                if m.isfile() or m.isdir():
                    tf.extract(m, out, filter="data")
    except tarfile.ReadError:
        # arXiv serves single-file submissions as a bare gzip of the .tex
        (out / "main.tex").write_bytes(gzip.decompress(data))


def tree_hash(root: Path) -> str:
    lines = []
    for p in sorted(root.rglob("*")):
        if p.is_file() and not p.is_symlink():
            lines.append(f"{p.relative_to(root).as_posix()}\0{hashlib.sha256(p.read_bytes()).hexdigest()}\n")
    return hashlib.sha256("".join(lines).encode()).hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--cache", default=os.environ.get("BENCH_CACHE", str(Path.home() / ".cache" / "texres-bench")))
    ap.add_argument("--dest", default=str(DEST))
    ap.add_argument("--pin", action="store_true", help="write tree_sha256 for entries that lack one")
    ap.add_argument("--only", default="")
    args = ap.parse_args()
    man = json.loads(MANIFEST.read_text())
    only = set(filter(None, args.only.split(",")))
    bad = 0
    for d in man["docs"]:
        if only and d["name"] not in only:
            continue
        out = Path(args.dest) / d["name"]
        if out.exists() and d.get("tree_sha256") and tree_hash(out) == d["tree_sha256"]:
            print(f"ok      {d['name']} (already unpacked)")
            continue
        unpack(d, download(d, Path(args.cache)), out)
        h = tree_hash(out)
        if not (out / d["main"]).is_file():
            print(f"MISSING {d['name']}: {d['main']} not in tree", file=sys.stderr)
            bad += 1
        elif d.get("tree_sha256") is None:
            if args.pin:
                d["tree_sha256"] = h
                print(f"pinned  {d['name']} {h[:16]}")
            else:
                print(f"unpinned {d['name']} {h} (run with --pin)")
        elif h != d["tree_sha256"]:
            print(f"MISMATCH {d['name']}: got {h}, manifest {d['tree_sha256']}", file=sys.stderr)
            bad += 1
        else:
            print(f"ok      {d['name']}")
    if args.pin:
        MANIFEST.write_text(json.dumps(man, indent=1) + "\n")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
