#!/usr/bin/env python3
"""
Download and unpack a diverse corpus of real-world TeX projects from arXiv for benchmarking.
Uses polite concurrency within arXiv's guidelines.
"""

import argparse
import concurrent.futures
import gzip
import json
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.parse
import xml.etree.ElementTree as ET
from pathlib import Path

ARCHIVES = ["cs", "math", "physics", "stat", "econ", "q-fin", "q-bio"]
USER_AGENT = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (TeX-Benchmark/1.0; mailto:leo@dd)"
OAI_URL = "https://oaipmh.arxiv.org/oai"
OAI_NS = "http://www.openarchives.org/OAI/2.0/"
ARXIV_RAW_NS = "http://arxiv.org/OAI/arXivRaw/"
MODERN_ID_RE = re.compile(r"\d{4}\.\d{4,5}")


def harvest_candidates(target: int, from_date: str):
    """Harvest a balanced, deduplicated candidate pool from arXiv OAI-PMH."""
    candidates_by_arch = {arch: [] for arch in ARCHIVES}
    # A buffer absorbs cross-list duplicates and archives with fewer records.
    per_archive = max(1, (target + len(ARCHIVES) - 1) // len(ARCHIVES) + 300)
    for arch in ARCHIVES:
        token = None
        while len(candidates_by_arch[arch]) < per_archive:
            if token:
                url = f"{OAI_URL}?verb=ListRecords&resumptionToken={token}"
            else:
                params = {
                    "verb": "ListRecords",
                    "metadataPrefix": "arXivRaw",
                    "set": arch,
                    "from": from_date,
                }
                url = f"{OAI_URL}?{urllib.parse.urlencode(params)}"
            cmd = [
                "curl", "-sL", "--fail", "--max-time", "90",
                "--retry", "4", "--retry-all-errors", "--retry-delay", "3",
                "-A", USER_AGENT, url,
            ]
            res = subprocess.run(cmd, capture_output=True, text=True)
            if res.returncode != 0:
                print(f"  Warning: failed to harvest {arch}", file=sys.stderr)
                break
            try:
                root = ET.fromstring(res.stdout)
            except ET.ParseError:
                print(f"  Warning: invalid OAI response for {arch}", file=sys.stderr)
                break
            seen = set(candidates_by_arch[arch])
            added = 0
            for node in root.findall(f".//{{{ARXIV_RAW_NS}}}id"):
                aid = (node.text or "").strip()
                if MODERN_ID_RE.fullmatch(aid) and aid not in seen:
                    seen.add(aid)
                    candidates_by_arch[arch].append(aid)
                    added += 1
            token_node = root.find(f".//{{{OAI_NS}}}resumptionToken")
            token = (
                (token_node.text or "").strip()
                if token_node is not None
                else ""
            )
            print(
                f"  Harvested {len(candidates_by_arch[arch])} IDs from {arch}"
            )
            if not token or added == 0:
                break
            time.sleep(3.0)
    interleaved = []
    max_len = max(len(v) for v in candidates_by_arch.values()) if candidates_by_arch else 0
    for i in range(max_len):
        for arch in ARCHIVES:
            if i < len(candidates_by_arch[arch]):
                interleaved.append((candidates_by_arch[arch][i], arch))
    final_list = []
    seen_ids = set()
    for aid, arch in interleaved:
        if aid not in seen_ids:
            seen_ids.add(aid)
            final_list.append((aid, arch))
            if len(final_list) == target:
                break
    return final_list


def find_main_tex(project_dir: Path):
    tex_files = list(project_dir.rglob("*.tex"))
    if not tex_files:
        return None

    candidates = []
    for tf in tex_files:
        try:
            content = tf.read_text(encoding="utf-8", errors="ignore")
            if "\\documentclass" in content or "\\documentstyle" in content:
                candidates.append(tf)
        except Exception:
            pass

    if not candidates:
        return tex_files[0].relative_to(project_dir).as_posix()

    prio_names = ["main.tex", "paper.tex", "ms.tex", "article.tex", "manuscript.tex"]
    for p in prio_names:
        for c in candidates:
            if c.name.lower() == p and c.parent == project_dir:
                return c.relative_to(project_dir).as_posix()

    root_candidates = [c for c in candidates if c.parent == project_dir]
    manuscripts = [c for c in root_candidates if "supp" not in c.name.lower()]
    if manuscripts:
        chosen = sorted(manuscripts, key=lambda c: ("manu" not in c.name.lower(), c.name))[0]
        return chosen.relative_to(project_dir).as_posix()
    if root_candidates:
        return root_candidates[0].relative_to(project_dir).as_posix()

    return candidates[0].relative_to(project_dir).as_posix()


def download_project(aid: str, arch: str, out_dir: Path, max_bytes: int):
    target_dir = out_dir / aid
    meta_file = target_dir / ".project_meta.json"
    if target_dir.is_dir() and meta_file.is_file():
        try:
            return json.loads(meta_file.read_text())
        except Exception:
            pass

    with tempfile.NamedTemporaryFile(suffix=".eprint", delete=False) as tmp:
        tmp_path = Path(tmp.name)

    try:
        cmd = [
            "curl", "-sL", "--max-time", "25",
            "--max-filesize", str(max_bytes),
            "-A", USER_AGENT,
            f"https://arxiv.org/e-print/{aid}",
            "-o", str(tmp_path)
        ]
        res = subprocess.run(cmd, capture_output=True, timeout=30)
        if res.returncode != 0 or not tmp_path.is_file():
            return None

        size = tmp_path.stat().st_size
        if size == 0 or size > max_bytes:
            return None

        with open(tmp_path, "rb") as f:
            head = f.read(1024)

        # PDF check
        if head.startswith(b"%PDF"):
            return None

        if target_dir.exists():
            shutil.rmtree(target_dir)
        target_dir.mkdir(parents=True, exist_ok=True)

        is_extracted = False
        if head.startswith(b"\x1f\x8b"):
            try:
                with tarfile.open(tmp_path, mode="r:gz") as tar:
                    for member in tar.getmembers():
                        if ".." in member.name or member.name.startswith("/"):
                            continue
                        tar.extract(member, path=target_dir)
                is_extracted = True
            except Exception:
                try:
                    with gzip.open(tmp_path, "rb") as gz:
                        decomp = gz.read()
                        if b"\\documentclass" in decomp or b"\\documentstyle" in decomp or b"\\begin" in decomp:
                            (target_dir / f"{aid}.tex").write_bytes(decomp)
                            is_extracted = True
                except Exception:
                    pass
        else:
            try:
                with tarfile.open(tmp_path, mode="r:") as tar:
                    for member in tar.getmembers():
                        if ".." in member.name or member.name.startswith("/"):
                            continue
                        tar.extract(member, path=target_dir)
                is_extracted = True
            except Exception:
                with open(tmp_path, "rb") as f:
                    all_bytes = f.read()
                if b"\\documentclass" in all_bytes or b"\\documentstyle" in all_bytes:
                    (target_dir / f"{aid}.tex").write_bytes(all_bytes)
                    is_extracted = True

        if not is_extracted:
            if target_dir.exists():
                shutil.rmtree(target_dir)
            return None

        main_tex = find_main_tex(target_dir)
        if not main_tex:
            if target_dir.exists():
                shutil.rmtree(target_dir)
            return None

        all_files = [f for f in target_dir.rglob("*") if f.is_file()]
        total_disk = sum(f.stat().st_size for f in all_files)

        meta = {
            "id": aid,
            "archive": arch,
            "main_tex": main_tex,
            "file_count": len(all_files),
            "download_bytes": size,
            "unpacked_bytes": total_disk,
        }
        meta_file.write_text(json.dumps(meta, indent=2))
        return meta

    except Exception:
        if target_dir.exists():
            shutil.rmtree(target_dir)
        return None
    finally:
        if tmp_path.exists():
            tmp_path.unlink()


def main():
    parser = argparse.ArgumentParser(description="Download TeX benchmark projects from arXiv")
    parser.add_argument("--target", type=int, default=100, help="Number of projects to download")
    parser.add_argument("--out-dir", type=Path, default=Path("corpus"), help="Output directory")
    parser.add_argument("--max-mb", type=float, default=20.0, help="Max download size per project in MB")
    parser.add_argument("--workers", type=int, default=3, help="Concurrent workers")
    parser.add_argument("--from-date", default="2025-01-01",
                        help="Earliest OAI datestamp to harvest (YYYY-MM-DD)")
    parser.add_argument("--candidate-factor", type=float, default=2.0,
                        help="Candidate-pool size as a multiple of --target")
    args = parser.parse_args()

    args.out_dir.mkdir(parents=True, exist_ok=True)
    max_bytes = int(args.max_mb * 1024 * 1024)

    print("Harvesting candidate papers across arXiv archives...")
    candidate_target = max(args.target, int(args.target * args.candidate_factor))
    candidates = harvest_candidates(candidate_target, args.from_date)
    print(f"Total unique candidate IDs harvested: {len(candidates)}")

    successful = []
    for meta_path in args.out_dir.glob("*/.project_meta.json"):
        try:
            meta = json.loads(meta_path.read_text())
            successful.append(meta)
        except Exception:
            pass

    print(f"Already have {len(successful)} valid projects in {args.out_dir}.")

    if len(successful) >= args.target:
        manifest_path = args.out_dir / "manifest.json"
        manifest_path.write_text(json.dumps(successful[:args.target], indent=2))
        print(f"Target already satisfied ({len(successful)} >= {args.target})")
        return

    # Filter out already downloaded
    existing_ids = {p["id"] for p in successful}
    pending = [(aid, arch) for aid, arch in candidates if aid not in existing_ids]

    print(f"Starting downloads with {args.workers} workers (target: {args.target})...")

    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as executor:
        future_map = {}
        pending_iter = iter(pending)

        # Prime the executor
        for _ in range(args.workers * 2):
            try:
                aid, arch = next(pending_iter)
                f = executor.submit(download_project, aid, arch, args.out_dir, max_bytes)
                future_map[f] = (aid, arch)
            except StopIteration:
                break

        while future_map and len(successful) < args.target:
            done, _ = concurrent.futures.wait(future_map.keys(), return_when=concurrent.futures.FIRST_COMPLETED)
            for f in done:
                aid, arch = future_map.pop(f)
                try:
                    meta = f.result()
                    if meta:
                        successful.append(meta)
                        print(
                            f"[{len(successful)}/{args.target}] {aid} ({arch}): "
                            f"main={meta['main_tex']} files={meta['file_count']} "
                            f"size={meta['download_bytes']/1024:.1f}KB"
                        )
                    else:
                        print(f"[-] Skipped {aid} ({arch})")
                except Exception as e:
                    print(f"[-] Error {aid} ({arch}): {e}")

                # Submit next if target not reached
                if len(successful) < args.target:
                    try:
                        next_aid, next_arch = next(pending_iter)
                        new_f = executor.submit(download_project, next_aid, next_arch, args.out_dir, max_bytes)
                        future_map[new_f] = (next_aid, next_arch)
                    except StopIteration:
                        pass

                # Polite delay between completions
                time.sleep(0.1)

    manifest_path = args.out_dir / "manifest.json"
    manifest_path.write_text(json.dumps(successful[:args.target], indent=2))

    print("\n" + "=" * 50)
    print(f"Completed: {len(successful[:args.target])} projects in {args.out_dir}/")
    by_cat = {}
    total_unpacked = 0
    for p in successful[:args.target]:
        by_cat[p["archive"]] = by_cat.get(p["archive"], 0) + 1
        total_unpacked += p.get("unpacked_bytes", 0)
    for cat, count in sorted(by_cat.items()):
        print(f"  {cat:8s}: {count:2d} projects")
    print(f"Total disk usage: {total_unpacked / (1024*1024):.1f} MB")
    print(f"Manifest written to {manifest_path}")


if __name__ == "__main__":
    main()
