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
  P/bin/baseline/pdflatex[.fmt]  archived binary and optional external format
  P/bin/candidate/               filled by the caller after rebuilds
  P/baseline-pdfs/*.pdf          pre-optimization Rust PDFs, zero-diff refs
  P/bib.bib                      shared cluster bibliography (../../bib)
  root/{name}-rust|reference     prepared four-document fixtures
  root/.seeds/{name}/            stable aux bytes restored before every trial
  root/inputs.json               four-document manifest (pages null if oracle failed)
  P/cluster-preflight.json       cluster oracle/preflight record (kept on failure)
  P/prepare-status.json          retained per-document failure list
  P/results/LABEL/               timings.json plus bounded failure logs by default
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
from pathlib import Path

import pymupdf

try:
    from bounded_capture import DEFAULT_MAX_CAPTURE_BYTES, capture_file, run_bounded
except ModuleNotFoundError:  # support `python -m scripts.bench_cold`
    from scripts.bounded_capture import (
        DEFAULT_MAX_CAPTURE_BYTES,
        capture_file,
        run_bounded,
    )

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
TRANSIENT_EXTS = ["log", "blg", "fls", "fdb_latexmk", "synctex.gz",
                  "depcache", "pagecache"]
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


def clear_job_transients(work: Path, job: str) -> None:
    for ext in TRANSIENT_EXTS:
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


def format_provenance(binary: Path) -> dict:
    fmt = require_fmt(binary)
    if fmt is None:
        return {"kind": "embedded", "path": None, "sha256": None}
    return {"kind": "external", "path": str(fmt), "sha256": sha256(fmt)}


def _capture_text(path: Path) -> str:
    try:
        return path.read_bytes().decode("utf-8", "replace")
    except OSError:
        return ""


def run_logged(cmd: list[str], cwd: Path, env: dict | None, timeout: float,
               log_path: Path,
               max_capture_bytes: int = DEFAULT_MAX_CAPTURE_BYTES) -> dict:
    return run_bounded(
        cmd, cwd=cwd, env=env, output_path=log_path, timeout=timeout,
        max_bytes=max_capture_bytes,
    )


# ---------------------------------------------------------------- prepare

def copy_tree(src: Path, dst: Path, ignore=None) -> None:
    shutil.copytree(src, dst, ignore=ignore or shutil.ignore_patterns(),
                    symlinks=True, dirs_exist_ok=False)


def compile_until_stable(exe: Path, work: Path, job: str, env: dict,
                         log_dir: Path, tag: str,
                         max_passes: int = 5,
                         max_capture_bytes: int = DEFAULT_MAX_CAPTURE_BYTES) -> dict:
    """Run the Rust binary uncached until aux content hashes stop changing.

    Convergence never claims success on rc != 0 or timeout. Output is streamed
    into bounded captures; successful pass logs are discarded and a failure
    retains only its final pass log.
    """
    log_dir.mkdir(parents=True, exist_ok=True)
    cur = aux_hashes(work, job)
    prev: dict | None = None
    passes, last_rc, timed_out = 0, None, False
    pass_records: list[dict] = []
    log_paths: list[Path] = []
    for i in range(1, max_passes + 1):
        clear_job_outputs(work, job)
        passes = i
        log_path = log_dir / f"{tag}-pass{i}.log"
        child = run_bounded(
            [str(exe), "-interaction=nonstopmode", f"{job}.tex"],
            cwd=work, env=env, output_path=log_path,
            timeout=RUST_TIMEOUT_S, max_bytes=max_capture_bytes,
        )
        log_paths.append(log_path)
        last_rc = None if child["timed_out"] else child["returncode"]
        timed_out = child["timed_out"]
        pass_records.append({
            "pass": i, "rc": last_rc, "timed_out": timed_out,
            "spawn_error": child["spawn_error"],
            "capture": child["capture"], "log": str(log_path),
        })
        prev = cur
        cur = aux_hashes(work, job)
        if cur == prev and last_rc == 0:
            break
        if timed_out:
            break
        if child["spawn_error"]:
            break
    converged = cur == prev and last_rc == 0
    for old in (log_paths if converged else log_paths[:-1]):
        old.unlink(missing_ok=True)
    for record in pass_records:
        if not Path(record["log"]).is_file():
            record["log"] = None
    record: dict = {"passes": passes, "converged": converged,
                    "last_rc": last_rc, "timed_out": timed_out,
                    "aux_hashes": cur, "logs": str(log_dir),
                    "pass_records": pass_records}
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
    ref_log = log_dir / f"{name}-reference-latexmk.log"
    r = run_logged(
        ["/usr/bin/latexmk", "-pdf", "-interaction=nonstopmode",
         "-halt-on-error", f"{job}.tex"],
        ref, None, LATEX_TIMEOUT_S, ref_log,
    )
    if r["returncode"] != 0 or r["timed_out"] or not (ref / f"{job}.pdf").is_file():
        raise RuntimeError(
            f"reference rebuild failed (rc={r['returncode']} "
            f"timeout={r['timed_out']}); see {ref_log}")
    ref_log.unlink(missing_ok=True)
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
    baseline_manifest = {
        "origin": str(binary), "origin_fmt": str(fmt) if fmt else None,
        "pdflatex": sha256(base / "pdflatex"),
        "format": format_provenance(binary),
        "pdflatex.fmt": None,
    }
    if fmt is not None:
        shutil.copy2(fmt, base / "pdflatex.fmt")
        baseline_manifest["pdflatex.fmt"] = sha256(base / "pdflatex.fmt")
    (base / "manifest.json").write_text(json.dumps(baseline_manifest, indent=2))
    os.chmod(base / "pdflatex", 0o555)
    if fmt is not None:
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

        # Complete system oracle. Child streams are always bounded; successful
        # logs are discarded after the oracle metrics have been recorded.
        ref = root / "cluster_ceo-reference"
        oracle_err = None
        oracle_logs: list[Path] = []
        first_log = logs / "cluster-ref-pass1.log"
        oracle_logs.append(first_log)
        r = run_logged(
            ["/usr/bin/latexmk", "-pdf", "-interaction=nonstopmode",
             "-halt-on-error", "main.tex"],
            ref, None, LATEX_TIMEOUT_S, first_log,
        )
        blg = _capture_text(ref / "main.blg")
        if r["returncode"] != 0 and "couldn't open database file" in blg.lower():
            # BibTeX refused the parent-relative database: retry confined to
            # these subprocesses only, never globally relaxing TeX.
            envb = dict(os.environ, BIBINPUTS=f"{parent}:", openin_any="a")
            retry_tex = logs / "cluster-ref-retry-pdflatex.log"
            retry_bib = logs / "cluster-ref-retry-bibtex.log"
            second_log = logs / "cluster-ref-pass2.log"
            oracle_logs.extend((retry_tex, retry_bib, second_log))
            run_logged(
                ["/usr/bin/pdflatex", "-interaction=nonstopmode", "main.tex"],
                ref, envb, LATEX_TIMEOUT_S, retry_tex,
            )
            run_logged(
                ["/usr/bin/bibtex", "main"], ref, envb, LATEX_TIMEOUT_S,
                retry_bib,
            )
            r = run_logged(
                ["/usr/bin/latexmk", "-pdf", "-interaction=nonstopmode",
                 "-halt-on-error", "main.tex"],
                ref, envb, LATEX_TIMEOUT_S, second_log,
            )
        tex_log_capture = logs / "cluster-ref-tex.log"
        if (ref / "main.log").is_file():
            capture_file(ref / "main.log", tex_log_capture,
                         max_bytes=DEFAULT_MAX_CAPTURE_BYTES)
            oracle_logs.append(tex_log_capture)
        log = _capture_text(tex_log_capture)
        if r["timed_out"]:
            oracle_err = f"reference latexmk timed out after {LATEX_TIMEOUT_S}s"
        elif r["returncode"] != 0:
            oracle_err = f"reference latexmk exited {r['returncode']}"
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
        if oracle_err:
            cluster_section["oracle_error"] = oracle_err
            preflight["oracle"] = oracle_err
            failures.append(
                f"cluster_ceo oracle: {oracle_err} (oracle logs under {logs}; "
                "manifest and retained fixtures stay for independent benchmarks)")
        else:
            preflight["oracle"] = "ok"
            for log_path in oracle_logs:
                log_path.unlink(missing_ok=True)
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
        for side in ("rust", "reference"):
            fixture = root / f"{name}-{side}"
            if fixture.is_dir():
                clear_job_transients(fixture, spec["job"])

    (parent / "prepare-status.json").write_text(json.dumps(
        {"ok": not failures, "failures": failures,
         "removed_diagnostics": diag_removed}, indent=2))
    print(f"BENCH: prepared {root}; cluster oracle pages="
          f"{manifest_out.get('cluster_ceo', {}).get('pages')}")
    if not failures:
        try:
            logs.rmdir()
        except OSError:
            pass
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
                  log_path: Path,
                  max_capture_bytes: int = DEFAULT_MAX_CAPTURE_BYTES
                  ) -> tuple[float, int | None, dict]:
    clear_job_outputs(work, job)
    pdf = work / f"{job}.pdf"
    if pdf.exists():
        raise RuntimeError(f"stale output survived clearing: {pdf}")
    child = run_bounded(
        [str(binary), "-interaction=nonstopmode", f"{job}.tex"],
        cwd=work,
        env=env,
        output_path=log_path,
        timeout=TIMER_TIMEOUT_S,
        max_bytes=max_capture_bytes,
    )
    capture = dict(child["capture"])
    capture.update(timed_out=child["timed_out"],
                   spawn_error=child["spawn_error"])
    return (child["elapsed_seconds"],
            None if child["timed_out"] else child["returncode"],
            capture)


def _stats(samples: list, valid: bool) -> dict:
    secs = [s["seconds"] for s in samples if s["ok"]] if valid else []
    return {"ok_trials": sum(1 for s in samples if s["ok"]),
            "total_trials": len(samples), "valid": valid,
            "median_seconds": statistics.median(secs) if secs else None,
            "min_seconds": min(secs) if secs else None,
            "max_seconds": max(secs) if secs else None}


def run(root: Path, binary: Path, label: str, n: int,
        compare: Path | None, retain: str = "failures",
        keep_work: bool = False,
        max_capture_bytes: int = DEFAULT_MAX_CAPTURE_BYTES) -> None:
    check_label(label)
    if n < 1:
        die(f"--runs must be >= 1, got {n}")
    if max_capture_bytes < 0:
        die(f"--max-capture-bytes must be non-negative, got {max_capture_bytes}")
    root = root.resolve()
    if not (root / "inputs.json").is_file():
        die(f"prepared manifest missing: run prepare first ({root / 'inputs.json'})")
    binaries = [resolve_binary(binary, "binary")]
    tags = ["a"]
    if compare is not None:
        binaries.append(resolve_binary(compare, "compare binary"))
        tags.append("b")
    formats = {str(b): format_provenance(b) for b in binaries}
    external_hashes = [record["sha256"] for record in formats.values()
                       if record["kind"] == "external"]
    if len(set(external_hashes)) > 1:
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
        "formats": formats,
        "adjacent_format_hashes": {
            binary: record["sha256"] for binary, record in formats.items()
            if record["kind"] == "external"
        },
        "inputs_manifest_sha": sha256(root / "inputs.json"),
        "runs": n, "timeout_seconds": TIMER_TIMEOUT_S,
        "retain": retain, "keep_work": keep_work,
        "max_capture_bytes": max_capture_bytes,
        "artifact_policy": "bounded-retention-v1",
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
            if not keep_work:
                for workdir in workdirs:
                    shutil.rmtree(workdir, ignore_errors=True)
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
                          "pages": None, "producer": None, "ok": False,
                          "capture": None, "log": None}
                try:
                    restore_seeds(root / ".seeds" / name, slots[si], job)
                    log_path = results / f"{name}-{tag}-{trial}.log"
                    elapsed, rc, capture = timed_compile(
                        binaries[si], slots[si], job, env,
                        log_path, max_capture_bytes)
                    sample["seconds"], sample["rc"] = elapsed, rc
                    sample["capture"] = capture
                    pdf = slots[si] / f"{job}.pdf"
                    ok = rc == 0 and pdf.is_file()
                    if ok:
                        sample["pages"] = pdf_pages(pdf)
                        sample["producer"] = pdf_producer(pdf)
                        ok = (sample["producer"] == "tex-rs"
                              and (oracle == "unavailable"
                                   or sample["pages"] == expected))
                    sample["ok"] = ok
                    sample["log"] = str(log_path)
                    if not ok:
                        errors.append(f"{tag}#{trial}: rc={rc} "
                                      f"pages={sample['pages']} "
                                      f"producer={sample['producer']}")
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
            logged = [sample for sample in samples[tag] if sample.get("log")]
            keep_sample = None
            if retain == "failures" and not valid and logged:
                failed_samples = [sample for sample in logged if not sample["ok"]]
                keep_sample = (failed_samples or logged)[-1]
            for sample in logged:
                if retain == "all" or sample is keep_sample:
                    continue
                Path(sample["log"]).unlink(missing_ok=True)
                sample["log"] = None
            if not valid:
                any_failure = True
        if not keep_work:
            for workdir in workdirs:
                shutil.rmtree(workdir, ignore_errors=True)
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

def profile(root: Path, binary: Path, label: str,
            keep_work: bool = False,
            max_capture_bytes: int = DEFAULT_MAX_CAPTURE_BYTES) -> None:
    check_label(label)
    root = root.resolve()
    binary = resolve_binary(binary, "binary")
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
        "format": format_provenance(binary),
        "inputs_manifest_sha": sha256(root / "inputs.json"),
        "removed_diagnostics": removed, "runs_per_document": 3,
        "note": "profile durations never feed the speed verdict",
    }, indent=2))
    perf = shutil.which("perf")
    failures: list[str] = []
    for name, spec in DOCS.items():
      job = spec["job"]
      work: Path | None = None
      try:
        work = make_work(root, name, f"profile-{label}")
        runs = []
        for i in range(3):
            restore_seeds(root / ".seeds" / name, work, job)
            clear_job_outputs(work, job)
            entry: dict = {"trial": i, "rc": None, "timeout": False}
            phase_log = pdir / f"{name}-phase-{i}.log"
            child = run_logged(
                [str(binary), "-interaction=nonstopmode", f"{job}.tex"],
                work, env, RUST_TIMEOUT_S, phase_log, max_capture_bytes,
            )
            entry["rc"] = child["returncode"]
            entry["timeout"] = child["timed_out"]
            entry["capture"] = child["capture"]
            entry["timing_lines"] = [
                line for line in _capture_text(phase_log).splitlines()
                if line.startswith(("TIMING:", "PHASE_TIMING "))
            ]
            if child["returncode"] == 0 and not child["timed_out"]:
                phase_log.unlink(missing_ok=True)
            else:
                entry["log"] = str(phase_log)
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
            perf_log = pdir / f"{name}-perf-attempt{mi + 1}.log"
            child = run_logged(
                cmd, work, env, RUST_TIMEOUT_S, perf_log, max_capture_bytes,
            )
            note = (f"rc={child['returncode']} timeout={child['timed_out']}\n"
                    + _capture_text(perf_log))
            if child["returncode"] == 0 and not child["timed_out"] and out.is_file():
                perf_log.unlink(missing_ok=True)
                break
            if out.exists():
                out.unlink()
            if mi == len(modes) - 1:
                (pdir / f"{name}.perf-note").write_text(
                    "hardware and cpu-clock perf both unavailable:\n" + note)
            perf_log.unlink(missing_ok=True)
      except Exception as exc:
        failures.append(f"{name}: profile failed: {exc}")
        (pdir / f"{name}-phases.json").write_text(json.dumps(
            {"error": str(exc)}, indent=2))
      finally:
        if work is not None and not keep_work:
            shutil.rmtree(work, ignore_errors=True)
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
    rc_v, pd_v = llvm_versions()
    if rc_v != pd_v:
        die(f"LLVM mismatch: rustc llvm-version {rc_v} vs llvm-profdata "
            f"{pd_v}; never mix profiles from a different compiler")

    def cargo(target_dir: Path, flags: str) -> Path:
        env = dict(os.environ, CARGO_TARGET_DIR=str(target_dir), RUSTFLAGS=flags)
        cargo_log = prof / f"cargo-{target_dir.name}.log"
        child = run_logged(
            ["cargo", "build", "--release", "-p", "tex-cli"],
            repo, env, 3600, cargo_log,
        )
        if child["returncode"] != 0 or child["timed_out"]:
            die(f"cargo build failed:\n{_capture_text(cargo_log)[-4000:]}")
        cargo_log.unlink(missing_ok=True)
        exe = target_dir / "release" / "pdflatex"
        if not exe.is_file():
            die(f"cargo produced no {exe}")
        dst_fmt = exe.parent / "pdflatex.fmt"
        if dst_fmt.exists():
            os.chmod(dst_fmt, 0o644)  # a previous PGO pass may have left it
            dst_fmt.unlink()
        if base_fmt.is_file():
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
        train_log = prof / f"train-{name}.log"
        child = run_logged(
            [str(gen), "-interaction=nonstopmode", f"{job}.tex"],
            work, env, RUST_TIMEOUT_S, train_log,
        )
        rc = None if child["timed_out"] else child["returncode"]
        fresh = (work / f"{job}.pdf").is_file()
        if rc != 0 or not fresh:
            # an incomplete training set invalidates the merged profile: fatal
            die(f"PGO training failed for {name} (rc={rc} fresh_pdf={fresh}); "
                f"see {prof / f'train-{name}.log'}")
        training[name] = {"rc": rc, "pages_expected": manifest.get(name, {}).get("pages")}
        train_log.unlink(missing_ok=True)
        shutil.rmtree(work, ignore_errors=True)
    tdir = parent / "pgo-train-plain"
    tdir.mkdir(parents=True)
    (tdir / "train.tex").write_text(PLAIN_TRAIN)
    plain_log = prof / "train-plain.log"
    plain = run_logged(
        [str(gen), "-plain", "train.tex"], tdir, env, TIMER_TIMEOUT_S,
        plain_log,
    )
    if (plain["returncode"] != 0 or plain["timed_out"]
            or not (tdir / "train.pdf").is_file()):
        die(f"plain training compile failed (rc={plain['returncode']} "
            f"timeout={plain['timed_out']}); "
            f"see {prof / 'train-plain.log'}")
    plain_log.unlink(missing_ok=True)
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
    merge_log = prof / "llvm-profdata.log"
    merge = run_logged(
        ["/usr/bin/llvm-profdata", "merge", "-o", str(merged),
         *[str(x) for x in raws]],
        repo, None, 300, merge_log,
    )
    if (merge["returncode"] != 0 or merge["timed_out"]
            or not merged.is_file() or merged.stat().st_size == 0):
        die(f"llvm-profdata merge failed:\n{_capture_text(merge_log)[-2000:]}")
    merge_log.unlink(missing_ok=True)
    shutil.rmtree(tdir, ignore_errors=True)
    use = cargo(parent / "target-pgo-use",
                f"-C target-cpu=native -C profile-use={merged}")
    (parent / "pgo-provenance.json").write_text(json.dumps({
        "training_docs": training,
        "rustc_llvm": rc_v2, "profdata_llvm": pd_v2,
        "removed_diagnostics": removed,
        "generate_exe_sha": sha256(gen),
        "generate_format": format_provenance(gen),
        "pgo_exe_sha": sha256(use),
        "pgo_format": format_provenance(use),
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
    r.add_argument("--retain", choices=("failures", "all", "none"),
                   default="failures",
                   help="Keep bounded failure logs by default, all logs, or none")
    r.add_argument("--keep-work", action="store_true",
                   help="Keep copied benchmark workspaces")
    r.add_argument("--max-capture-bytes", type=int,
                   default=DEFAULT_MAX_CAPTURE_BYTES,
                   help="Retained bytes per child output; 0 keeps all")
    f = sub.add_parser("profile")
    f.add_argument("--root", required=True, type=Path)
    f.add_argument("--binary", required=True, type=Path)
    f.add_argument("--label", required=True)
    f.add_argument("--keep-work", action="store_true")
    f.add_argument("--max-capture-bytes", type=int,
                   default=DEFAULT_MAX_CAPTURE_BYTES)
    g = sub.add_parser("pgo")
    g.add_argument("--root", required=True, type=Path)
    args = ap.parse_args()
    if args.cmd == "prepare":
        prepare(args.root, args.binary)
    elif args.cmd == "run":
        run(args.root, args.binary, args.label, args.runs, args.compare_binary,
            args.retain, args.keep_work, args.max_capture_bytes)
    elif args.cmd == "profile":
        profile(args.root, args.binary, args.label, args.keep_work,
                args.max_capture_bytes)
    else:
        pgo(args.root)


if __name__ == "__main__":
    main()
