#!/usr/bin/env python3
"""Four-document cold-compile benchmark driver.

Subcommands:
  prepare  build an isolated four-document benchmark root (inputs + oracles)
  run      timed fresh-process uncached single-pass compilation
  profile  untimed phase-timed + perf-recorded compilations
  pgo      LLVM profile-guided release build (train -> merge -> reuse)

Timing counts process start -> exit only. Preparation, aux restoration,
rasterization, bibliography generation and cargo compilation are outside the
timed interval. This harness never modifies manuscript sources and never
falls back to system pdflatex where a supplied Rust binary is required.

Layout produced by prepare (root must be empty; P = root.parent):
  P/bin/baseline/pdflatex[.fmt]  archived pre-optimization binary (write-once)
  P/bin/candidate/               filled by the caller after rebuilds
  P/baseline-pdfs/*.pdf          pre-optimization Rust PDFs, zero-diff refs
  P/bib.bib                      shared cluster bibliography (../../bib)
  root/{name}-rust|reference     prepared four-document fixtures
  root/.seeds/{name}/            stable aux bytes restored before every trial
  root/inputs.json               four-document manifest (pages null if oracle failed)
  P/cluster-preflight.json       cluster oracle/preflight record (kept on failure)
  P/prepare-status.json          retained per-document failure list
  P/results/LABEL/               per-trial logs, timings.json (labels are one-shot)
"""

import argparse
import hashlib
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

import pymupdf

DOCS = {
    "trust": {"job": "main"},
    "beamer": {"job": "ch12_trading_strategies"},
    "ai": {"job": "main"},
    "cluster_ceo": {"job": "main"},
}
GATED = ("trust", "beamer", "ai")
LIVE = Path("/tmp/tex-parity-live")
LIVE_MANIFEST = LIVE / "inputs.json"
CLUSTER_SRC = Path("/home/leo/dd/cluster_ceo")
SHARED_BIB = Path("/home/leo/da/Dropbox/WingWah-Leo/bib.bib")
# prescribed original sources for the rebuild fallback (plan step 2)
RECORDED_SOURCES = {
    "trust": Path("/home/leo/da/Dropbox/Apps/Overleaf/trust_own"),
    "beamer": Path("/home/leo/da/Dropbox/teaching/Futures_and_Options/ch12_beamer"),
    "ai": Path("/home/leo/dd/ai_patent_test"),
}
AUX_EXTS = ["aux", "out", "toc", "nav", "snm", "bbl"]
CACHE_EXTS = ["pdf", "depcache", "pagecache"]
# Diagnostic variables read by the engine. Remove them, rather than assigning
# empty strings, so both binaries run without timing output or serial-PDF overrides.
DIAGNOSTIC_VARS = ["TEXDEBUG", "PHASE_TIMING"]
TIMER_TIMEOUT_S = 120   # plan: 120-second timeout on every timed compile
RUST_TIMEOUT_S = 120    # same bound for preflight/rebuild/training compiles
LATEX_TIMEOUT_S = 600   # system oracle builds and perf may run longer


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def die(msg: str) -> None:
    print(f"BENCH: FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def check_label(label: str) -> str:
    """Labels become directory-name suffixes; refuse anything path-shaped."""
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", label):
        die(f"label must match [A-Za-z0-9][A-Za-z0-9._-]* (no '/'): {label!r}")
    return label


def pdf_pages(path: Path) -> int:
    with pymupdf.open(path) as doc:
        return len(doc)


def pdf_producer(path: Path) -> str:
    with pymupdf.open(path) as doc:
        return doc.metadata.get("producer") or ""


def clear_job_outputs(work: Path, job: str) -> None:
    for ext in CACHE_EXTS:
        (work / f"{job}.{ext}").unlink(missing_ok=True)


def aux_hashes(work: Path, job: str) -> dict:
    return {e: sha256(work / f"{job}.{e}")
            for e in AUX_EXTS if (work / f"{job}.{e}").is_file()}


def save_seeds(work: Path, job: str, dest: Path) -> None:
    """Stable aux bytes live under an explicit list, not a directory scan."""
    dest.mkdir(parents=True, exist_ok=True)
    names = [f"{job}.{e}" for e in AUX_EXTS if (work / f"{job}.{e}").is_file()]
    for n in names:
        shutil.copy2(work / n, dest / n)
    (dest / "seed-list.json").write_text(json.dumps(names, indent=2))


def restore_seeds(seeds: Path, work: Path, job: str) -> None:
    """Restore the identical seeded bytes; delete job aux absent from the seed
    set (a run may create e.g. main.bbl that the seed does not carry)."""
    listing = json.loads((seeds / "seed-list.json").read_text())
    for name in listing:
        shutil.copy2(seeds / name, work / name)
    seeded = set(listing)
    for ext in AUX_EXTS:
        f = work / f"{job}.{ext}"
        if f.name not in seeded and f.is_file():
            f.unlink()


def timing_env(extra: dict[str, str] | None = None) -> tuple[dict, list]:
    """Child environment for every compilation: diagnostics removed identically
    for both binaries; input-search settings and locale preserved."""
    env = dict(os.environ)
    removed = []
    for key in DIAGNOSTIC_VARS:
        if env.pop(key, None) is not None:
            removed.append(key)
    if extra:
        env.update(extra)
    return env, removed


def resolve_binary(path: Path, what: str) -> Path:
    """Absolute before any child cwd changes; missing is an explicit failure."""
    b = Path(path).resolve()
    if not b.is_file():
        die(f"{what} missing or not a file: {b}")
    return b


def require_fmt(binary: Path) -> Path | None:
    fmt = binary.parent / "pdflatex.fmt"
    return fmt if fmt.is_file() else None


# ---------------------------------------------------------------- prepare

def copy_tree(src: Path, dst: Path, ignore=None) -> None:
    shutil.copytree(src, dst, ignore=ignore or shutil.ignore_patterns(),
                    symlinks=True, dirs_exist_ok=False)


def compile_until_stable(exe: Path, work: Path, job: str, env: dict,
                         log_dir: Path, tag: str,
                         max_passes: int = 5) -> dict:
    """Run the Rust binary uncached until aux content hashes stop changing.

    Convergence never claims success on rc != 0 or timeout. All pass logs
    are retained even when the loop fails.
    """
    log_dir.mkdir(parents=True, exist_ok=True)
    cur = aux_hashes(work, job)
    prev: dict | None = None
    passes, last_rc, timed_out = 0, None, False
    for i in range(1, max_passes + 1):
        clear_job_outputs(work, job)
        passes = i
        try:
            r = subprocess.run([str(exe), "-interaction=nonstopmode", f"{job}.tex"],
                               cwd=work, env=env, capture_output=True,
                               text=True, timeout=RUST_TIMEOUT_S)
            last_rc = r.returncode
            (log_dir / f"{tag}-pass{i}.log").write_text(
                (r.stdout or "") + (r.stderr or ""))
        except subprocess.TimeoutExpired as exc:
            last_rc, timed_out = None, True
            parts = (exc.stdout, exc.stderr)
            partial = "".join(p.decode(errors="replace") if isinstance(p, bytes)
                              else (p or "") for p in parts)
            (log_dir / f"{tag}-pass{i}.log").write_text(
                f"BENCH: timeout after {RUST_TIMEOUT_S}s\n{partial}")
        prev = cur
        cur = aux_hashes(work, job)
        if cur == prev and last_rc == 0:
            break
    converged = cur == prev and last_rc == 0
    record: dict = {"passes": passes, "converged": converged,
                    "last_rc": last_rc, "timed_out": timed_out,
                    "aux_hashes": cur, "logs": str(log_dir)}
    pdf = work / f"{job}.pdf"
    if pdf.is_file():
        try:
            record["pages"] = pdf_pages(pdf)
            record["producer"] = pdf_producer(pdf)
        except Exception as exc:  # unreadable PDF is a recorded failure
            record["pdf_error"] = f"unreadable pdf: {exc}"
    else:
        record["pdf_error"] = "no pdf produced"
    return record


def rebuild_from_source(name: str, root: Path, base_bin: Path, env: dict,
                        log_dir: Path) -> dict:
    """Prescribed fallback when a live snapshot is absent or its recorded
    source hashes disagree: rebuild BOTH sides from the recorded original
    source. Raises RuntimeError on any unusable input; never falls back to
    system pdflatex for the Rust side."""
    src = RECORDED_SOURCES[name]
    if not src.is_dir():
        raise RuntimeError(f"recorded source missing: {src}")
    job = DOCS[name]["job"]
    rust = root / f"{name}-rust"
    ref = root / f"{name}-reference"
    for d in (rust, ref):
        if d.exists():
            shutil.rmtree(d)
    copy_tree(src, rust)
    copy_tree(src, ref)
    r = subprocess.run(["/usr/bin/latexmk", "-pdf", "-interaction=nonstopmode",
                        "-halt-on-error", f"{job}.tex"],
                       cwd=ref, capture_output=True, text=True,
                       timeout=LATEX_TIMEOUT_S)
    (log_dir / f"{name}-reference-latexmk.log").write_text(
        f"rc={r.returncode}\n" + (r.stdout or "") + (r.stderr or ""))
    if r.returncode != 0 or not (ref / f"{job}.pdf").is_file():
        raise RuntimeError(
            f"reference rebuild failed (rc={r.returncode}); see "
            f"{log_dir / f'{name}-reference-latexmk.log'}")
    stable = compile_until_stable(base_bin, rust, job, env, log_dir,
                                  f"{name}-rebuild")
    if not stable["converged"]:
        raise RuntimeError(f"rust rebuild did not converge in {stable['passes']} "
                           f"passes (rc={stable['last_rc']}); see {stable['logs']}")
    # refresh the hash map (same manifest schema) from the reconstructed inputs
    old_files: dict = {}
    if LIVE_MANIFEST.is_file():
        old_files = json.loads(LIVE_MANIFEST.read_text()).get(name, {}).get("files", {})
    files = {}
    for rel in old_files:
        f = rust / rel
        if f.is_file():
            files[rel] = sha256(f)
    if not files:
        # no old-manifest guidance: hash every non-generated file
        for f in sorted(rust.rglob("*")):
            if not f.is_file():
                continue
            rel = str(f.relative_to(rust))
            if rel.endswith((".log", ".blg", ".fls", ".fdb_latexmk", ".synctex.gz",
                             ".depcache", ".pagecache")) or rel == f"{job}.pdf":
                continue
            if rel.endswith(tuple(f".{e}" for e in AUX_EXTS)):
                continue
            files[rel] = sha256(f)
    return {"source": str(src), "pages": pdf_pages(ref / f"{job}.pdf"),
            "files": files, "rebuilt": True, "rust_stability": stable}


def snapshot_hash_ok(side: Path, files: dict) -> list:
    """Manifest paths whose bytes disagree with (or are missing from) a snapshot."""
    bad = []
    for rel, digest in files.items():
        f = side / rel
        if not f.is_file():
            bad.append(f"{rel} (missing)")
        elif sha256(f) != digest:
            bad.append(f"{rel} (hash mismatch)")
    return bad


def prepare(root: Path, binary: Path) -> None:
    root = root.resolve()
    binary = resolve_binary(binary, "rust binary")
    fmt = require_fmt(binary)
    if fmt is None:
        die(f"format missing beside binary: {binary.parent / 'pdflatex.fmt'}")
    if root.exists():
        if not root.is_dir():
            die(f"root path is a file, not a directory: {root}")
        if any(root.iterdir()):
            die(f"refusing occupied root: {root}")
    parent = root.parent
    base = parent / "bin" / "baseline"
    if (base / "pdflatex").exists():
        die(f"refusing to overwrite archived baseline slot: {base / 'pdflatex'}")
    for slot in ("baseline", "candidate"):
        (parent / "bin" / slot).mkdir(parents=True, exist_ok=True)
    failures: list[str] = []

    # write-once baseline slot, made read-only for immutability
    shutil.copy2(binary, base / "pdflatex")
    shutil.copy2(fmt, base / "pdflatex.fmt")
    (base / "manifest.json").write_text(json.dumps({
        "origin": str(binary), "origin_fmt": str(fmt),
        "pdflatex": sha256(base / "pdflatex"),
        "pdflatex.fmt": sha256(base / "pdflatex.fmt"),
    }, indent=2))
    os.chmod(base / "pdflatex", 0o555)
    os.chmod(base / "pdflatex.fmt", 0o444)

    root.mkdir(parents=True, exist_ok=True)
    logs = parent / "prepare-logs"
    logs.mkdir(exist_ok=True)
    manifest_out: dict = {}
    old = json.loads(LIVE_MANIFEST.read_text()) if LIVE_MANIFEST.is_file() else {}
    env0, diag_removed = timing_env()

    # established three: verify BOTH snapshot sides against the manifest
    # hashes; on disagreement rebuild both sides from the recorded source
    (parent / "baseline-pdfs").mkdir(exist_ok=True)
    for name in ("trust", "beamer", "ai"):
        job = DOCS[name]["job"]
        try:
            details = old.get(name) or {}
            files = details.get("files", {})
            bad: list[str] = []
            if not details:
                bad.append("no manifest entry (absent old manifest is the "
                           "rebuild trigger)")
            elif not files:
                bad.append("manifest records no hashes to verify")
            else:
                bad = (snapshot_hash_ok(LIVE / f"{name}-rust", files)
                       + ["reference/" + b for b in
                          snapshot_hash_ok(LIVE / f"{name}-reference", files)])
            rebuilt = False
            if not bad:
                copy_tree(LIVE / f"{name}-rust", root / f"{name}-rust")
                copy_tree(LIVE / f"{name}-reference", root / f"{name}-reference")
                source = details.get("source", str(RECORDED_SOURCES[name]))
            else:
                print(f"BENCH: {name}: snapshot unusable ({bad[:4]}); "
                      "rebuilding both sides from recorded source",
                      file=sys.stderr)
                details = rebuild_from_source(name, root, base / "pdflatex",
                                              env0, logs)
                rebuilt = True
                source = details["source"]
                bad = ["rebuilt: " + b for b in bad]
            rust, ref = root / f"{name}-rust", root / f"{name}-reference"
            base_pdf = parent / "baseline-pdfs" / f"{name}.pdf"
            src_pdf = rust / f"{job}.pdf"
            if base_pdf.exists():
                # re-prepare under the same parent: identical bytes only
                if not src_pdf.is_file() or sha256(base_pdf) != sha256(src_pdf):
                    failures.append(
                        f"{name}: baseline PDF exists at {base_pdf} and the "
                        "snapshot PDF differs; refusing to overwrite the "
                        "archived pre-optimization reference")
            elif src_pdf.is_file():
                shutil.copy2(src_pdf, base_pdf)
                os.chmod(base_pdf, 0o444)
            else:
                failures.append(
                    f"{name}: pre-optimization rust PDF missing after copy; "
                    "no zero-diff baseline archived for this document")
            clear_job_outputs(rust, job)
            entry = {"source": str(source),
                     "pages": pdf_pages(ref / f"{job}.pdf"),
                     "files": details["files"]}
            if rebuilt:
                entry["rebuilt_from_source"] = True
                entry["snapshot_problems"] = bad
            manifest_out[name] = entry
            os.chmod(ref / f"{job}.pdf", 0o444)  # oracle is immutable
        except Exception as exc:
            failures.append(f"{name}: preparation failed: {exc}")
            continue

    # cluster_ceo: active inputs only, no generated job state
    try:
        excl = shutil.ignore_patterns(
            ".git", "archive", "Submission", ".ipynb_checkpoints", "flock",
            "main.pdf", "main.log", "main.aux", "main.out", "main.fls",
            "main.fdb_latexmk", "main.bbl", "main.blg", "main.synctex.gz",
            "main.nav", "main.snm", "main.toc", "main.xdv", "ref.pdf",
            "texput.log", "*.fdb_latexmk", "*.fls", "*.synctex.gz",
        )
        for side in ("rust", "reference"):
            dst = root / f"cluster_ceo-{side}"
            if dst.exists():
                shutil.rmtree(dst)
            copy_tree(CLUSTER_SRC, dst, ignore=excl)
        if SHARED_BIB.is_file():
            shutil.copy2(SHARED_BIB, parent / "bib.bib")
        elif not (parent / "bib.bib").is_file():
            raise RuntimeError(
                f"shared bibliography missing: {SHARED_BIB} (do not substitute "
                "the incomplete local cluster_ceo/bib.bib)")
        # the unchanged ../../bib in main.tex resolves to P/bib.bib from both
        # root/cluster_ceo-rust and root/cluster_ceo-reference: same depth
        cluster_rust = root / "cluster_ceo-rust"
        cluster_files = {}
        for f in sorted(cluster_rust.rglob("*")):
            if f.is_file():
                cluster_files[str(f.relative_to(cluster_rust))] = sha256(f)
        cluster_section: dict = {
            "source": str(CLUSTER_SRC), "pages": None,
            "files": cluster_files,
            "external_files": {"bib.bib": sha256(parent / "bib.bib")},
        }
        preflight: dict = {}

        # complete system oracle; every log retained even when it fails
        ref = root / "cluster_ceo-reference"
        oracle_err = None
        try:
            r = subprocess.run(
                ["/usr/bin/latexmk", "-pdf", "-interaction=nonstopmode",
                 "-halt-on-error", "main.tex"],
                cwd=ref, capture_output=True, text=True,
                timeout=LATEX_TIMEOUT_S)
            (logs / "cluster-ref-pass1.log").write_text(
                f"rc={r.returncode}\n" + (r.stdout or "") + (r.stderr or ""))
            blg = (ref / "main.blg").read_text(errors="replace") \
                if (ref / "main.blg").exists() else ""
            if r.returncode != 0 and "couldn't open database file" in blg.lower():
                # BibTeX refused the parent-relative database: retry confined
                # to these subprocesses only, never globally relaxing TeX
                envb = dict(os.environ, BIBINPUTS=f"{parent}:", openin_any="a")
                r2 = subprocess.run(["/usr/bin/pdflatex",
                                     "-interaction=nonstopmode", "main.tex"],
                                    cwd=ref, env=envb, capture_output=True,
                                    text=True, timeout=LATEX_TIMEOUT_S)
                r3 = subprocess.run(["/usr/bin/bibtex", "main"], cwd=ref,
                                    env=envb, capture_output=True, text=True,
                                    timeout=LATEX_TIMEOUT_S)
                (logs / "cluster-ref-bibtex-retry.log").write_text(
                    f"pdflatex rc={r2.returncode}\n" + (r2.stdout or "")
                    + (r2.stderr or "") + f"\nbibtex rc={r3.returncode}\n"
                    + (r3.stdout or "") + (r3.stderr or ""))
                r = subprocess.run(
                    ["/usr/bin/latexmk", "-pdf", "-interaction=nonstopmode",
                     "-halt-on-error", "main.tex"],
                    cwd=ref, env=envb, capture_output=True, text=True,
                    timeout=LATEX_TIMEOUT_S)
                (logs / "cluster-ref-pass2.log").write_text(
                    f"rc={r.returncode}\n" + (r.stdout or "") + (r.stderr or ""))
            log = (ref / "main.log").read_text(errors="replace") \
                if (ref / "main.log").is_file() else ""
            if r.returncode != 0:
                oracle_err = f"reference latexmk exited {r.returncode}"
            elif not (ref / "main.bbl").is_file():
                oracle_err = "reference produced no main.bbl"
            elif not (ref / "main.pdf").is_file():
                oracle_err = "no oracle main.pdf"
            else:
                undef = re.findall(r"(Citation|Reference) \S+ undefined", log)
                if undef:
                    oracle_err = (
                        f"{len(undef)} undefined citations/references in the "
                        "oracle despite the matched bibliography — genuine "
                        "input blocker, no fabricated entries")
                else:
                    cluster_section["pages"] = pdf_pages(ref / "main.pdf")
                    os.chmod(ref / "main.pdf", 0o444)  # oracle immutable
        except subprocess.TimeoutExpired as exc:
            (logs / "cluster-ref-timeout.log").write_text(
                f"timeout after {LATEX_TIMEOUT_S}s\n{exc.stdout or ''}")
            oracle_err = f"reference latexmk timed out after {LATEX_TIMEOUT_S}s"
        if oracle_err:
            cluster_section["oracle_error"] = oracle_err
            preflight["oracle"] = oracle_err
            failures.append(
                f"cluster_ceo oracle: {oracle_err} (oracle logs under {logs}; "
                "manifest and retained fixtures stay for independent benchmarks)")
        else:
            preflight["oracle"] = "ok"
            # seed the rust copy from the reference with the generated
            # bibliography and stable aux bytes identically, then run until
            # aux hashes converge
            rust = cluster_rust
            for ext in AUX_EXTS:
                f = ref / f"main.{ext}"
                if f.is_file():
                    shutil.copy2(f, rust / f"main.{ext}")
            clear_job_outputs(rust, "main")
            stable = compile_until_stable(base / "pdflatex", rust, "main",
                                          env0, logs, "cluster-rust")
            stable["reference_pages"] = cluster_section["pages"]
            preflight["rust"] = stable
            clear_job_outputs(rust, "main")
            if not stable["converged"]:
                failures.append(
                    "cluster_ceo rust preflight did NOT converge "
                    f"(passes={stable['passes']} rc={stable['last_rc']} "
                    f"timeout={stable['timed_out']} pdf="
                    f"{stable.get('pages') or stable.get('pdf_error')}); "
                    f"logs kept under {stable['logs']}")
            elif stable.get("producer") != "tex-rs":
                failures.append(
                    f"cluster_ceo rust preflight: producer "
                    f"{stable.get('producer')!r} is not tex-rs")
            elif stable.get("pages") != cluster_section["pages"]:
                failures.append("cluster_ceo Rust page count differs from its oracle")
                preflight["page_mismatch"] = {
                    "rust": stable.get("pages"),
                    "reference": cluster_section["pages"]}
                print(f"BENCH: cluster rust {stable.get('pages')} pages vs "
                      f"oracle {cluster_section['pages']} (recorded; parity "
                      "gate compares against the oracle)", file=sys.stderr)
        (parent / "cluster-preflight.json").write_text(
            json.dumps(preflight, indent=2))
        manifest_out["cluster_ceo"] = cluster_section
    except Exception as exc:
        failures.append(f"cluster_ceo: preparation failed: {exc}")
        manifest_out.setdefault("cluster_ceo", {
            "source": str(CLUSTER_SRC), "pages": None, "files": {},
            "external_files": {}, "oracle_error": str(exc)})

    # all four names are mandatory in the manifest (doc_parity gate contract)
    for name in DOCS:
        manifest_out.setdefault(name, {"source": "", "pages": None,
                                       "files": {}, "manifest_error":
                                       "preparation failed for this document"})
    (root / "inputs.json").write_text(json.dumps(manifest_out, indent=2))

    # aux seeds: last, from the settled rust directories
    for name, spec in DOCS.items():
        seed_dir = root / ".seeds" / name
        if seed_dir.exists():
            shutil.rmtree(seed_dir)
        if (root / f"{name}-rust").is_dir():
            save_seeds(root / f"{name}-rust", spec["job"], seed_dir)

    (parent / "prepare-status.json").write_text(json.dumps(
        {"ok": not failures, "failures": failures,
         "removed_diagnostics": diag_removed}, indent=2))
    print(f"BENCH: prepared {root}; cluster oracle pages="
          f"{manifest_out.get('cluster_ceo', {}).get('pages')}")
    if failures:
        for f in failures:
            print(f"BENCH: RETAINED FAILURE: {f}", file=sys.stderr)
        sys.exit(1)


# ---------------------------------------------------------------- run

def make_work(root: Path, name: str, tag: str) -> Path:
    """Independent copy of the prepared rust dir, at the SAME path depth
    beneath root so ../../bib and other relative inputs stay valid."""
    job = DOCS[name]["job"]
    work = root / f"{name}-rust__{tag}"
    if work.exists():
        shutil.rmtree(work)
    copy_tree(root / f"{name}-rust", work)
    clear_job_outputs(work, job)
    return work


def timed_compile(binary: Path, work: Path, job: str, env: dict,
                  log_path: Path) -> tuple[float, int | None]:
    clear_job_outputs(work, job)
    pdf = work / f"{job}.pdf"
    if pdf.exists():
        raise RuntimeError(f"stale output survived clearing: {pdf}")
    t0 = time.perf_counter_ns()
    try:
        r = subprocess.run([str(binary), "-interaction=nonstopmode", f"{job}.tex"],
                           cwd=work, env=env, capture_output=True,
                           timeout=TIMER_TIMEOUT_S)
    except subprocess.TimeoutExpired as exc:
        elapsed = (time.perf_counter_ns() - t0) / 1e9
        log_path.write_bytes((exc.stdout or b"") + (exc.stderr or b"")
                             + f"\nBENCH: timeout after {TIMER_TIMEOUT_S}s\n".encode())
        return elapsed, None
    elapsed = (time.perf_counter_ns() - t0) / 1e9
    log_path.write_bytes(r.stdout + r.stderr)  # written after the interval
    return elapsed, r.returncode


def _stats(samples: list, valid: bool) -> dict:
    secs = [s["seconds"] for s in samples if s["ok"]] if valid else []
    return {"ok_trials": sum(1 for s in samples if s["ok"]),
            "total_trials": len(samples), "valid": valid,
            "median_seconds": statistics.median(secs) if secs else None,
            "min_seconds": min(secs) if secs else None,
            "max_seconds": max(secs) if secs else None}


def run(root: Path, binary: Path, label: str, n: int,
        compare: Path | None) -> None:
    check_label(label)
    if n < 1:
        die(f"--runs must be >= 1, got {n}")
    root = root.resolve()
    if not (root / "inputs.json").is_file():
        die(f"prepared manifest missing: run prepare first ({root / 'inputs.json'})")
    binaries = [resolve_binary(binary, "binary")]
    tags = ["a"]
    if compare is not None:
        binaries.append(resolve_binary(compare, "compare binary"))
        tags.append("b")
    fmt_hashes = {}
    for b in binaries:
        fmt = require_fmt(b)
        if fmt is None:
            die(f"pdflatex.fmt missing beside {b}: a missing format is an "
                "explicit failure, never an implicit system-pdflatex switch")
        fmt_hashes[str(b)] = sha256(fmt)
    if len(set(fmt_hashes.values())) != 1:
        die("paired binaries must use identical generic formats")
    results = root.parent / "results" / label
    if results.exists():
        die(f"refusing to overwrite label dir: {results}")
    results.mkdir(parents=True)
    manifest = json.loads((root / "inputs.json").read_text())
    env, removed = timing_env()
    (results / "env.json").write_text(json.dumps({
        "removed_diagnostics": removed,
        "binaries": [str(b) for b in binaries],
        "binary_hashes": [sha256(b) for b in binaries],
        "adjacent_format_hashes": fmt_hashes,
        "inputs_manifest_sha": sha256(root / "inputs.json"),
        "runs": n, "timeout_seconds": TIMER_TIMEOUT_S,
    }, indent=2))
    report: dict = {}
    any_failure = False
    for name, spec in DOCS.items():
        job = spec["job"]
        seed_dir = root / ".seeds" / name
        if not (seed_dir / "seed-list.json").is_file():
            errors_pre = [f"missing aux seeds for {name}: {seed_dir}"]
        else:
            errors_pre = []
        workdirs = []
        try:
            workdirs.append(make_work(root, name, label))
            if compare is not None:
                workdirs.append(make_work(root, name, f"{label}-cmp"))
        except (OSError, RuntimeError) as exc:
            errors_pre.append(f"{name}: fixture copy failed: {exc}")
        if errors_pre:
            # keep iterating the other documents; this one can never be valid
            for tag in tags:
                report.setdefault(name, {})[tag] = {
                    "expected_pages": manifest.get(name, {}).get("pages"),
                    "oracle": "unavailable",
                    "errors": errors_pre, "samples": [], "ok_trials": 0,
                    "total_trials": n, "valid": False,
                    "median_seconds": None, "min_seconds": None,
                    "max_seconds": None}
                any_failure = True
            continue
        slots = workdirs
        expected = manifest.get(name, {}).get("pages")
        oracle = "manifest"
        if expected is None:
            # retained prepare failure: the run is measured but page parity
            # cannot be claimed; prepare-status.json is the record
            oracle = "unavailable"
        samples: dict[str, list] = {t: [] for t in tags}
        errors: list[str] = []
        for trial in range(n):
            for si in (range(len(tags)) if trial % 2 == 0 else reversed(range(len(tags)))):
                tag = tags[si]
                sample = {"trial": trial, "seconds": None, "rc": None,
                          "pages": None, "producer": None, "ok": False}
                try:
                    restore_seeds(root / ".seeds" / name, slots[si], job)
                    elapsed, rc = timed_compile(
                        binaries[si], slots[si], job, env,
                        results / f"{name}-{tag}-{trial}.log")
                    sample["seconds"], sample["rc"] = elapsed, rc
                    pdf = slots[si] / f"{job}.pdf"
                    ok = rc == 0 and pdf.is_file()
                    if ok:
                        sample["pages"] = pdf_pages(pdf)
                        sample["producer"] = pdf_producer(pdf)
                        ok = (sample["producer"] == "tex-rs"
                              and (oracle == "unavailable"
                                   or sample["pages"] == expected))
                    sample["ok"] = ok
                    if not ok:
                        errors.append(f"{tag}#{trial}: rc={rc} "
                                      f"pages={sample['pages']} "
                                      f"producer={sample['producer']}")
                except subprocess.TimeoutExpired:
                    errors.append(f"{tag}#{trial}: timeout "
                                  f">{TIMER_TIMEOUT_S}s")
                except Exception as exc:
                    errors.append(f"{tag}#{trial}: {exc}")
                samples[tag].append(sample)
        for tag in tags:
            valid = all(s["ok"] for s in samples[tag])
            if valid and oracle != "manifest":
                valid = False
                errors.append(f"{tag}: document lacks an oracle page count")
            entry = {"expected_pages": expected, "oracle": oracle,
                     "errors": [e for e in errors if e.startswith((f"{tag}#", f"{tag}:"))],
                     "samples": samples[tag]}
            entry.update(_stats(samples[tag], valid))
            if valid and name in GATED and entry["median_seconds"] is not None:
                entry["under_500ms"] = entry["median_seconds"] < 0.5
            report.setdefault(name, {})[tag] = entry
            if not valid:
                any_failure = True
    report["_verdict"] = {
        "valid": not any_failure,
        "note": ("valid means complete successful compilation timings; raster "
                 "parity is checked separately; failed trials never yield medians"),
        "gated_under_500ms": {
            name: {tag: report[name][tag].get("under_500ms", False)
                   for tag in report[name]}
            for name in GATED if name in report},
    }
    (results / "timings.json").write_text(json.dumps(report, indent=2))
    summary = {name: {tag: {k: v for k, v in report[name][tag].items()
                            if k != "samples"}
                      for tag in report[name]}
               for name in report if name != "_verdict"}
    summary["_verdict"] = report["_verdict"]
    print(json.dumps(summary, indent=2))
    if any_failure:
        die("one or more trials failed; medians above are not a valid speed "
            "verdict; see results dir")


# ---------------------------------------------------------------- profile

def profile(root: Path, binary: Path, label: str) -> None:
    check_label(label)
    root = root.resolve()
    binary = resolve_binary(binary, "binary")
    if require_fmt(binary) is None:
        die(f"pdflatex.fmt missing beside {binary}")
    if not (root / "inputs.json").is_file():
        die(f"prepared manifest missing: run prepare first ({root})")
    pdir = root.parent / "profile" / label
    if pdir.exists():
        die(f"refusing to overwrite profile label dir: {pdir}")
    pdir.mkdir(parents=True)
    manifest = json.loads((root / "inputs.json").read_text())
    env, removed = timing_env({"PHASE_TIMING": "1"})
    (pdir / "provenance.json").write_text(json.dumps({
        "binary": str(binary), "binary_sha": sha256(binary),
        "format_sha": sha256(binary.parent / "pdflatex.fmt"),
        "inputs_manifest_sha": sha256(root / "inputs.json"),
        "removed_diagnostics": removed, "runs_per_document": 3,
        "note": "profile durations never feed the speed verdict",
    }, indent=2))
    perf = shutil.which("perf")
    failures: list[str] = []
    for name, spec in DOCS.items():
      job = spec["job"]
      try:
        work = make_work(root, name, f"profile-{label}")
        runs = []
        for i in range(3):
            restore_seeds(root / ".seeds" / name, work, job)
            clear_job_outputs(work, job)
            entry: dict = {"trial": i, "rc": None, "timeout": False}
            try:
                r = subprocess.run(
                    [str(binary), "-interaction=nonstopmode", f"{job}.tex"],
                    cwd=work, env=env, capture_output=True, text=True,
                    timeout=RUST_TIMEOUT_S)
                entry["rc"] = r.returncode
                entry["timing_lines"] = [l for l in (r.stdout + r.stderr).splitlines()
                                         if l.startswith(("TIMING:", "PHASE_TIMING "))]
            except subprocess.TimeoutExpired:
                entry["timeout"] = True
            runs.append(entry)
        (pdir / f"{name}-phases.json").write_text(json.dumps(
            {"pages_expected": manifest.get(name, {}).get("pages"),
             "runs": runs}, indent=2))
        if not perf:
            (pdir / f"{name}.perf-note").write_text("perf binary unavailable")
            continue
        modes = [
            ["record", "-F", "999", "-g", "--call-graph", "fp"],
            ["record", "-e", "cpu-clock", "-F", "999", "-g", "--call-graph", "fp"],
        ]
        out = pdir / f"{name}.perf"
        for mi, mode in enumerate(modes):
            # fresh caches immediately before every perf attempt
            restore_seeds(root / ".seeds" / name, work, job)
            clear_job_outputs(work, job)
            if out.exists():
                out.unlink()
            cmd = [perf, *mode, "-o", str(out), "--", str(binary),
                   "-interaction=nonstopmode", f"{job}.tex"]
            note = ""
            try:
                r = subprocess.run(cmd, cwd=work, env=env, capture_output=True,
                                   text=True, timeout=RUST_TIMEOUT_S)
                note = f"rc={r.returncode}\n{r.stderr}"
                if r.returncode == 0 and out.is_file():
                    break
            except subprocess.TimeoutExpired:
                note = f"perf timed out after {RUST_TIMEOUT_S}s"
            if out.exists():
                out.unlink()
            if mi == len(modes) - 1:
                (pdir / f"{name}.perf-note").write_text(
                    "hardware and cpu-clock perf both unavailable:\n" + note)
      except Exception as exc:
        failures.append(f"{name}: profile failed: {exc}")
        (pdir / f"{name}-phases.json").write_text(json.dumps(
            {"error": str(exc)}, indent=2))
    print(f"BENCH: profile written under {pdir}")
    if failures:
        for f in failures:
            print(f"BENCH: RETAINED FAILURE: {f}", file=sys.stderr)
        sys.exit(1)


# ---------------------------------------------------------------- pgo

PLAIN_TRAIN = r"""\pdfoutput=1
\font\tenrm=cmr10 \tenrm \textfont0=\tenrm
\font\mathi=cmmi10 \textfont1=\mathi
\font\mathsy=cmsy10 \textfont2=\mathsy
\def\phrase#1{The compiler expands #1 without storing a finished page. }
\def\labeltext{ordinary tokens}
\shipout\vbox{\hsize=220pt
  \phrase{macros}\phrase{\labeltext}\phrase{arguments}\par
  \hbox{$x+1=2$}}
\end
"""


def llvm_versions() -> tuple[str, str]:
    r = subprocess.run(["rustc", "-vV"], capture_output=True, text=True,
                       timeout=30)
    if r.returncode != 0:
        die("rustc -vV failed")
    m = re.search(r"LLVM version:\s*(\S+)", r.stdout)
    if not m:
        die(f"no LLVM version in rustc -vV:\n{r.stdout}")
    rustc_llvm = m.group(1)
    r = subprocess.run(["/usr/bin/llvm-profdata", "--version"],
                       capture_output=True, text=True, timeout=30)
    if r.returncode != 0:
        die("llvm-profdata --version failed")
    m = re.search(r"LLVM version\s*(\S+)", r.stdout)
    if not m:
        die(f"no LLVM version in llvm-profdata output:\n{r.stdout}")
    return rustc_llvm, m.group(1)


def pgo(root: Path) -> None:
    root = root.resolve()
    parent = root.parent
    if not (root / "inputs.json").is_file():
        die(f"prepared manifest missing: run prepare first ({root})")
    prof = parent / "profile-pgo"
    occupied = [prof, parent / "pgo-train-plain"]
    occupied.extend(root / f"{name}-rust__pgo-train" for name in DOCS)
    if any(path.exists() for path in occupied):
        die("refusing to overwrite an existing PGO campaign; prepare a fresh root")
    prof.mkdir(parents=True)
    repo = Path(__file__).resolve().parent.parent
    base_fmt = parent / "bin" / "baseline" / "pdflatex.fmt"
    if not base_fmt.is_file():
        die(f"fixed baseline format missing: {base_fmt}")
    rc_v, pd_v = llvm_versions()
    if rc_v != pd_v:
        die(f"LLVM mismatch: rustc llvm-version {rc_v} vs llvm-profdata "
            f"{pd_v}; never mix profiles from a different compiler")

    def cargo(target_dir: Path, flags: str) -> Path:
        env = dict(os.environ, CARGO_TARGET_DIR=str(target_dir), RUSTFLAGS=flags)
        r = subprocess.run(["cargo", "build", "--release", "-p", "tex-cli"],
                           cwd=repo, env=env, capture_output=True,
                           text=True, timeout=3600)
        if r.returncode != 0:
            die(f"cargo build failed:\n{r.stderr[-4000:]}")
        exe = target_dir / "release" / "pdflatex"
        if not exe.is_file():
            die(f"cargo produced no {exe}")
        dst_fmt = exe.parent / "pdflatex.fmt"
        if dst_fmt.exists():
            os.chmod(dst_fmt, 0o644)  # a previous PGO pass may have left it
            dst_fmt.unlink()
        shutil.copy2(base_fmt, dst_fmt)
        return exe

    gen = cargo(parent / "target-pgo-generate",
                f"-C target-cpu=native -C profile-generate={prof}")
    env, removed = timing_env()
    env["LLVM_PROFILE_FILE"] = str(prof / "%m-%p.profraw")
    manifest = json.loads((root / "inputs.json").read_text())
    training: dict = {}
    for name, spec in DOCS.items():
        job = spec["job"]
        try:
            work = make_work(root, name, "pgo-train")
            restore_seeds(root / ".seeds" / name, work, job)
            clear_job_outputs(work, job)
        except (OSError, RuntimeError, ValueError) as exc:
            die(f"PGO training setup failed for {name}: {exc}")
        try:
            r = subprocess.run([str(gen), "-interaction=nonstopmode",
                                f"{job}.tex"], cwd=work, env=env,
                               capture_output=True, text=True,
                               timeout=RUST_TIMEOUT_S)
            rc, out = r.returncode, (r.stdout or "") + (r.stderr or "")
        except subprocess.TimeoutExpired:
            rc, out = None, f"timeout after {RUST_TIMEOUT_S}s"
        (prof / f"train-{name}.log").write_text(f"rc={rc}\n{out}")
        fresh = (work / f"{job}.pdf").is_file()
        if rc != 0 or not fresh:
            # an incomplete training set invalidates the merged profile: fatal
            die(f"PGO training failed for {name} (rc={rc} fresh_pdf={fresh}); "
                f"see {prof / f'train-{name}.log'}")
        training[name] = {"rc": rc, "pages_expected": manifest.get(name, {}).get("pages")}
    tdir = parent / "pgo-train-plain"
    tdir.mkdir(parents=True)
    (tdir / "train.tex").write_text(PLAIN_TRAIN)
    r = subprocess.run([str(gen), "-plain", "train.tex"], cwd=tdir, env=env,
                       capture_output=True, text=True, timeout=TIMER_TIMEOUT_S)
    (prof / "train-plain.log").write_text(
        f"rc={r.returncode}\n" + (r.stdout or "") + (r.stderr or ""))
    if r.returncode != 0 or not (tdir / "train.pdf").is_file():
        die(f"plain training compile failed (rc={r.returncode}); "
            f"see {prof / 'train-plain.log'}")
    raws = sorted(prof.glob("*.profraw"))
    if not raws:
        die("no .profraw captured")
    empty = [str(p) for p in raws if p.stat().st_size == 0]
    if empty:
        die(f"empty .profraw files (profile capture broken): {empty}")
    # re-confirm compiler identity immediately before merging
    rc_v2, pd_v2 = llvm_versions()
    if (rc_v2, pd_v2) != (rc_v, pd_v):
        die(f"LLVM version changed during PGO: {rc_v}/{pd_v} -> {rc_v2}/{pd_v2}; "
            "refusing to merge a possibly foreign profile set")
    merged = prof / "merged.profdata"
    r = subprocess.run(["/usr/bin/llvm-profdata", "merge", "-o", str(merged),
                        *[str(x) for x in raws]], capture_output=True,
                       text=True, timeout=300)
    if r.returncode != 0 or not merged.is_file() or merged.stat().st_size == 0:
        die(f"llvm-profdata merge failed:\n{r.stderr[-2000:]}")
    use = cargo(parent / "target-pgo-use",
                f"-C target-cpu=native -C profile-use={merged}")
    (parent / "pgo-provenance.json").write_text(json.dumps({
        "training_docs": training,
        "rustc_llvm": rc_v2, "profdata_llvm": pd_v2,
        "removed_diagnostics": removed,
        "generate_exe_sha": sha256(gen),
        "generate_fmt_sha": sha256(gen.parent / "pdflatex.fmt"),
        "pgo_exe_sha": sha256(use),
        "pgo_fmt_sha": sha256(use.parent / "pdflatex.fmt"),
        "profraw_files": [p.name for p in raws],
        "flags": {"generate": f"-C target-cpu=native -C profile-generate={prof}",
                  "use": f"-C target-cpu=native -C profile-use={merged}"},
    }, indent=2))
    print(f"BENCH: PGO binary at {use} (sha {sha256(use)}); "
          "compare with run --compare-binary before selecting")


# ---------------------------------------------------------------- main

def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("prepare")
    p.add_argument("--root", required=True, type=Path)
    p.add_argument("--binary", required=True, type=Path)
    r = sub.add_parser("run")
    r.add_argument("--root", required=True, type=Path)
    r.add_argument("--binary", required=True, type=Path)
    r.add_argument("--label", required=True)
    r.add_argument("--runs", required=True, type=int)
    r.add_argument("--compare-binary", type=Path)
    f = sub.add_parser("profile")
    f.add_argument("--root", required=True, type=Path)
    f.add_argument("--binary", required=True, type=Path)
    f.add_argument("--label", required=True)
    g = sub.add_parser("pgo")
    g.add_argument("--root", required=True, type=Path)
    args = ap.parse_args()
    if args.cmd == "prepare":
        prepare(args.root, args.binary)
    elif args.cmd == "run":
        run(args.root, args.binary, args.label, args.runs, args.compare_binary)
    elif args.cmd == "profile":
        profile(args.root, args.binary, args.label)
    else:
        pgo(args.root)


if __name__ == "__main__":
    main()
