#!/usr/bin/env python3
"""CI regression test: download and compile 10 diverse arXiv papers with ratex."""

import argparse
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
from pathlib import Path

PAPERS = [
    ("1407.1141", "physics"),
    ("1506.04619", "physics"),
    ("1610.01020", "physics"),
    ("1703.05000", "physics"),
    ("1801.04247", "physics (RevTeX)"),
    ("1802.06597", "physics"),
    ("2004.06003", "cs (IEEE)"),
    ("2008.08796", "cs (Times)"),
    ("2201.05139", "econ (Large)"),
    ("2305.03149", "stat (Bbl)"),
]

USER_AGENT = "Mozilla/5.0 (X11; Linux x86_64) TeX-Benchmark/1.0"


def find_main_tex(folder: Path) -> Path:
    prio = ["main.tex", "paper.tex", "ms.tex", "article.tex"]
    for name in prio:
        candidate = folder / name
        if candidate.is_file():
            return candidate
    for f in folder.glob("*.tex"):
        content = f.read_text(encoding="utf-8", errors="ignore")
        if "\\documentclass" in content or "\\documentstyle" in content:
            return f
    tex_files = list(folder.glob("*.tex"))
    if tex_files:
        return tex_files[0]
    raise RuntimeError(f"No .tex file found in {folder}")


def download_and_extract(paper_id: str, dest_dir: Path) -> Path:
    dest_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(suffix=".tar.gz", delete=False) as tmp:
        tmp_path = Path(tmp.name)
    try:
        cmd = [
            "curl", "-sL", "--fail", "--max-time", "30",
            "-A", USER_AGENT,
            f"https://arxiv.org/e-print/{paper_id}",
            "-o", str(tmp_path)
        ]
        subprocess.run(cmd, check=True)
        # Try extracting as tar
        try:
            with tarfile.open(tmp_path) as tar:
                tar.extractall(path=dest_dir)
        except tarfile.TarError:
            # Single tex file
            (dest_dir / f"{paper_id}.tex").write_bytes(tmp_path.read_bytes())
        return find_main_tex(dest_dir)
    finally:
        if tmp_path.exists():
            tmp_path.unlink()


def main():
    parser = argparse.ArgumentParser(description="Test 10 arXiv papers with ratex in CI.")
    parser.add_argument("--ratex", default="ratex", help="Path to ratex executable")
    args = parser.parse_args()

    ratex_bin = shutil.which(args.ratex) or str(Path(args.ratex).resolve())
    print(f"==> Testing 10 real-world arXiv papers using: {ratex_bin}")
    print("=" * 70)

    work_dir = Path(tempfile.mkdtemp(prefix="ratex-arxiv-ci-"))
    passed = 0
    failed = []

    try:
        for idx, (paper_id, desc) in enumerate(PAPERS, 1):
            paper_dir = work_dir / paper_id
            t0 = time.perf_counter()
            try:
                main_tex = download_and_extract(paper_id, paper_dir)
                compile_res = subprocess.run(
                    [ratex_bin, "-silent", main_tex.name],
                    cwd=paper_dir,
                    capture_output=True,
                    text=True,
                    timeout=60
                )
                elapsed = time.perf_counter() - t0
                pdf_file = main_tex.with_suffix(".pdf")
                if compile_res.returncode == 0 and pdf_file.is_file() and pdf_file.stat().st_size > 1000:
                    passed += 1
                    print(f"[{idx:2d}/10] ✓ {paper_id} ({desc:16s}): PASS in {elapsed:.2f}s ({pdf_file.stat().st_size/1024:.1f} KB)")
                else:
                    err = (compile_res.stderr or compile_res.stdout or "").strip().splitlines()
                    msg = err[0] if err else f"Exit code {compile_res.returncode}"
                    failed.append((paper_id, msg))
                    print(f"[{idx:2d}/10] ✗ {paper_id} ({desc:16s}): FAIL in {elapsed:.2f}s - {msg[:80]}")
            except Exception as e:
                failed.append((paper_id, str(e)))
                print(f"[{idx:2d}/10] ✗ {paper_id} ({desc:16s}): ERROR - {e}")

        print("=" * 70)
        print(f"Result: {passed}/10 papers compiled cleanly to valid PDFs.")
        if failed:
            print("\nFailed papers:")
            for pid, err in failed:
                print(f"  - {pid}: {err}")
            sys.exit(1)
        else:
            print("\nAll 10 arXiv papers passed with 100% success!")
    finally:
        shutil.rmtree(work_dir, ignore_errors=True)


if __name__ == "__main__":
    main()
