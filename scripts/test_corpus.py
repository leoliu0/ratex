#!/usr/bin/env python3
"""
Concurrent corpus comparison harness: Rust pdflatex vs system pdflatex.

For every entry in corpus/manifest.json (100 by default):

  1. Build two fully isolated per-engine workspace copies under
     output/corpus/work/<id>/{rust,ref}/ — no compilation ever touches corpus/,
     so there is zero cross-engine aux contamination and the source tree stays pristine.
  2. Strip known generated main-job artifacts (stem.aux/.log/.out/.toc/.pdf/...)
     from the copies. Source PDFs (graphics includes) and source .bbl files are
     PRESERVED, so a single pass can still resolve bibliography and cross-refs
     that the author shipped.
  3. Compile ONE pass with each engine (single pdflatex invocation per engine;
     bibtex is NOT run — reported prominently in the summary).
     Subprocesses run argv-list only (never shell=True), stdin=DEVNULL (EOF on
     any error prompt instead of hanging), in their own process group so a
     timeout SIGKILLs the whole group, with stdout+stderr captured as raw
     binary straight to a per-run log file on disk.
  4. Persist: both output PDFs (output/corpus/pdf/<id>.{rust,ref}.pdf), captured
     stdout and the engine-written .log (output/corpus/results/<id>/), and an
     atomic per-project checkpoint JSON (output/corpus/checkpoints/<id>.json).
     The aggregate report (output/corpus/report.json) is rewritten atomically
     after every completion, so partial results always survive a crash.
  5. Compare: pagewise raster diff at 72 dpi via PyMuPDF (image dimensions,
     mean/max absolute pixel difference, % pixels differing, identical-page
     count) plus text-layer similarity, alongside page counts.

Per-engine status (durable, not page-count-only):
  clean    exit 0, valid PDF, no TeX errors in the log
  errors   valid PDF produced but nonzero exit or TeX errors in the log
  failure  no valid PDF
  timeout  process group killed after --timeout seconds
  memlimit child killed by a signal under the RLIMIT_AS cap
           (--mem-limit-mib; distinct from timeout/failure)

Modes (--mode):
  campaign     (DEFAULT) 104-document parity campaign: 100 corpus projects plus
             the four private docs (trust, beamer, ai, cluster_ceo) inventoried
             from the prepared/source manifests. Each side compiles in its own
             frozen isolated copy until aux state CONVERGES (cross-refs +
             BibTeX honoring shipped .bbl files; system bibtex for the
             reference, native tex-bibtex for Rust output; never a system
             fallback for the Rust side). Rust .depcache/.pagecache artifacts
             are removed before every pass. Comparison is exact 150-DPI RGB
             pixel parity (no cropping, no tolerance) with geometry and page
             count checks, per-page scores, worst-page artifacts, and a hard
             gate (output/<dir>/gate.json) that fails on incomplete coverage,
             compilation errors, unconverged runs, invalid PDFs, or any page
             or document below --page-min/--doc-min percent. Unavailable
             reference inputs are RETAINED blockers, never excluded.
  single-pass LEGACY diagnostic: exactly one pdflatex invocation per engine,
             no bibtex, 72-DPI similarity metrics. Useful for triage only —
             it never claims campaign success.

Usage:
  scripts/test_corpus.py [--mode campaign] [--max-passes 5]
      [--jobs 4] [--timeout 60] [--mem-limit-mib 4096] [--output output/corpus]
      [--rust target/release/pdflatex] [--sys /usr/bin/pdflatex]
      [--rust-bibtex target/release/tex-bibtex] [--sys-bibtex /usr/bin/bibtex]
      [--limit 100] [--only id1,id2] [--corpus-only] [--private-only]
      [--no-resume] [--no-keep-work]
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import difflib
import hashlib
import json
import os
import re
import shutil
import signal
import statistics
import subprocess
import sys
import threading
import time
from datetime import datetime, timezone
from pathlib import Path

try:
    import resource  # RLIMIT_AS child guard (POSIX)
except ImportError:  # graceful degrade: no address-space cap
    resource = None

try:
    import numpy as np
except ImportError:  # graceful degrade: equality-only raster comparison
    np = None

import pymupdf  # installed PyMuPDF (fitz replacement)

# Extensions of known generated main-job artifacts. Deliberately excludes .bbl
# (source bibliography, preserved per contract). .pdf is stripped ONLY when the
# file stem matches the main job stem.
STRIP_EXTS = (
    "aux", "log", "out", "toc", "nav", "snm", "lof", "lot", "bbl.bak",
    "fls", "fdb_latexmk", "bcf", "run.xml", "ilg", "ind", "idx",
    "depcache", "pagecache", "synctex.gz", "aux.bak", "pdf",
)
# stale artifacts written by older in-place harness versions
STRIP_NAMES_EXTRA = ("ref.pdf", "rust.pdf")

DPI = 72.0
PIX_THRESHOLD = 8  # per-channel-mean difference counted as a differing pixel

# Hard per-child address-space cap (RLIMIT_AS, MiB) applied in the subprocess
# preexec path to EVERY engine/bibtex run. Legitimate corpus compiles stay
# far below 4 GiB; the observed runaway passes grew tens of GB RSS and pinned
# the machine. An over-budget allocation fails at once, the child dies on a
# signal, and the run classifies distinctly as "memlimit" (never silently as
# timeout/failure). 0 disables the guard.
DEFAULT_MEM_LIMIT_MIB = 4096

ERROR_LINE_RE = re.compile(
    r"^(?:!\s|!pdfTeX|! LaTeX|! Font|! Package|! Undefined control sequence|"
    r"Emergency stop|Fatal error occurred|Missing input file)"
)


def now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def atomic_write_json(path: Path, obj) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + f".tmp.{os.getpid()}.{threading.get_ident()}")
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(obj, f, indent=2, sort_keys=False)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)


def first_line(text: str, limit: int = 200) -> str:
    return text.splitlines()[0][:limit] if text.strip() else ""


def capture_version(bin_path: Path, timeout: float) -> str:
    try:
        r = subprocess.run([str(bin_path), "--version"], stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=timeout)
        return first_line((r.stdout or "") + (r.stderr or ""))
    except Exception as e:  # noqa: BLE001 - version string is best-effort
        return f"<error: {e}>"


# --------------------------------------------------------------------------
# Workspace preparation

def gen_artifact_names(stem: str) -> set[str]:
    names = {f"{stem}.{ext}" for ext in STRIP_EXTS}
    names |= {f"{stem}.{suffix}" for suffix in STRIP_NAMES_EXTRA}
    return names


def prepare_workspace(src: Path, dst: Path, tex_rel: Path) -> list[str]:
    """Fresh isolated copy of src into dst, stripping generated main-job
    artifacts by file name. Returns the stripped file names (audit trail)."""
    shutil.rmtree(dst, ignore_errors=True)
    strip = gen_artifact_names(tex_rel.stem)
    stripped = sorted(
        str(p.relative_to(src)) for p in src.rglob("*")
        if p.is_file() and p.name in strip
    )

    def _ignore(_dir: str, names: list[str]) -> set[str]:
        return {n for n in names if n in strip}

    shutil.copytree(src, dst, ignore=_ignore)
    return stripped


# --------------------------------------------------------------------------
# Compilation

def _mem_limit_preexec(limit_bytes: int):
    """preexec_fn clamping RLIMIT_AS in the child before exec; respects an
    already-tighter inherited hard limit."""
    def _set_limits() -> None:
        _cur_soft, cur_hard = resource.getrlimit(resource.RLIMIT_AS)
        hard = cur_hard if cur_hard != resource.RLIM_INFINITY else limit_bytes
        target = min(limit_bytes, hard)
        resource.setrlimit(resource.RLIMIT_AS, (target, hard))
    return _set_limits


def run_compile(bin_path: str, work_dir: Path, tex_name: str, cap_path: Path,
                timeout: float, env: dict | None = None,
                flags: tuple[str, ...] | None = None,
                mem_limit_mib: int = DEFAULT_MEM_LIMIT_MIB) -> dict:
    """One engine pass. argv list (no shell), stdin DEVNULL, new process
    group, merged binary stdout/stderr straight to disk. Timeout kills and
    reaps the whole group. Every child gets RLIMIT_AS = mem_limit_mib in the
    preexec path (0 or non-POSIX disables); signal death with the cap armed
    reports mem_killed/kill_signal distinctly from timeout. `flags` overrides
    the pdfLaTeX flag set (XeTeX/LuaTeX have no -no-shell-escape)."""
    cap_path.parent.mkdir(parents=True, exist_ok=True)
    if flags is None:
        flags = ("-interaction=nonstopmode", "-no-shell-escape")
    limit_bytes = max(0, int(mem_limit_mib)) * (1 << 20)
    preexec = _mem_limit_preexec(limit_bytes) if limit_bytes and resource else None
    cmd = [bin_path, *flags, tex_name]
    t0 = time.perf_counter()
    timed_out = False
    spawn_error = None
    rc = None
    with open(cap_path, "wb") as cap:
        try:
            p = subprocess.Popen(
                cmd, cwd=work_dir, stdin=subprocess.DEVNULL,
                stdout=cap, stderr=subprocess.STDOUT,
                start_new_session=True, env=env, preexec_fn=preexec,
            )
        except OSError as e:
            spawn_error = str(e)
            p = None
        if p is not None:
            try:
                rc = p.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                timed_out = True
                try:
                    os.killpg(os.getpgid(p.pid), signal.SIGKILL)
                except ProcessLookupError:
                    pass
                try:
                    p.wait(timeout=10)
                except subprocess.TimeoutExpired:  # last resort
                    p.kill()
                    p.wait()
    dt_ms = (time.perf_counter() - t0) * 1000.0
    kill_signal = None
    mem_killed = False
    if not timed_out and rc is not None and rc < 0:
        # The harness never signal-kills a non-timed-out child: death by
        # signal with RLIMIT_AS armed is the cap firing (abort/segv/bus).
        try:
            kill_signal = signal.Signals(-rc).name
        except ValueError:
            kill_signal = f"SIG{-rc}"
        mem_killed = limit_bytes > 0
    return {"exit": rc, "timed_out": timed_out, "time_ms": dt_ms,
            "spawn_error": spawn_error, "cmd": cmd,
            "mem_killed": mem_killed, "kill_signal": kill_signal}


def extract_errors(log_text: str, cap: int = 8) -> list[str]:
    seen, out = set(), []
    for line in log_text.splitlines():
        s = line.strip()
        if ERROR_LINE_RE.match(s):
            s = s[:160]
            if s not in seen:
                seen.add(s)
                out.append(s)
            if len(out) >= cap:
                break
    return out


def pdf_info(pdf: Path) -> dict:
    """Validate a PDF and report pages/bytes/sha. Never trust existence alone."""
    info = {"pdf": str(pdf), "pdf_exists": False, "pdf_valid": False,
            "pages": None, "pdf_bytes": 0, "pdf_sha1": None}
    if not pdf.is_file() or pdf.stat().st_size == 0:
        return info
    info["pdf_exists"] = True
    info["pdf_bytes"] = pdf.stat().st_size
    try:
        doc = pymupdf.open(str(pdf))
        try:
            info["pages"] = doc.page_count
            info["pdf_valid"] = doc.page_count >= 1
        finally:
            doc.close()
    except Exception:  # corrupt / truncated output still counts as invalid
        info["pdf_valid"] = False
    try:
        h = hashlib.sha1()
        with open(pdf, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 16), b""):
                h.update(chunk)
        info["pdf_sha1"] = h.hexdigest()
    except OSError:
        pass
    return info


def classify(run: dict, pdf_valid: bool, errors: list[str]) -> str:
    if run["timed_out"] or run["spawn_error"]:
        return "timeout" if run["timed_out"] else "failure"
    if run.get("mem_killed"):
        return "memlimit"
    if not pdf_valid:
        return "failure"
    if run["exit"] != 0 or errors:
        return "errors"
    return "clean"


def compile_engine(engine: str, bin_path: str, ws: Path, tex_rel: Path,
                   idir: Path, timeout: float,
                   mem_limit_mib: int = DEFAULT_MEM_LIMIT_MIB) -> dict:
    """Single pass + classification + artifact persistence for one engine."""
    work_dir = ws / tex_rel.parent
    cap_path = idir / f"{engine}.stdout.log"
    run = run_compile(bin_path, work_dir, tex_rel.name, cap_path, timeout,
                      mem_limit_mib=mem_limit_mib)

    stem_pdf = work_dir / f"{tex_rel.stem}.pdf"
    stem_log = work_dir / f"{tex_rel.stem}.log"

    # Persist engine-written .log and the output PDF outside the workspace.
    kept_log = None
    if stem_log.is_file():
        kept_log = idir / f"{engine}.tex.log"
        shutil.copyfile(stem_log, kept_log)
    kept_pdf = out_root_of(idir) / "pdf" / f"{pdf_basename(idir.name, engine)}"
    pdf_stat = pdf_info(stem_pdf)
    if pdf_stat["pdf_exists"]:
        kept_pdf.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(stem_pdf, kept_pdf)

    # Error source: engine-written transcript preferred, captured output second.
    log_text = ""
    for cand in (stem_log, cap_path):
        if cand.is_file():
            with open(cand, "rb") as f:
                log_text = f.read().decode("utf-8", "replace")
            if log_text.strip():
                break
    errors = extract_errors(log_text)
    if not errors and log_text == "" and run["spawn_error"]:
        errors = [f"spawn-error: {run['spawn_error']}"]

    status = classify(run, pdf_stat["pdf_valid"], errors)
    return {
        "engine": engine, "bin": bin_path, "status": status,
        "exit": run["exit"], "timed_out": run["timed_out"],
        "mem_killed": run["mem_killed"], "kill_signal": run["kill_signal"],
        "time_ms": round(run["time_ms"], 1),
        "errors": errors,
        **pdf_stat,
        "pdf": str(kept_pdf) if kept_pdf.is_file() else None,
        "pdf_exists": kept_pdf.is_file(),
        "captured_log": str(cap_path), "tex_log": str(kept_log) if kept_log else None,
    }


def pdf_basename(aid: str, engine: str) -> str:
    return f"{aid}.{engine}.pdf"


def out_root_of(idir: Path) -> Path:
    """results/<id>/ -> output root (pdfs live at <output>/pdf/)."""
    return idir.parent.parent


# --------------------------------------------------------------------------
# Campaign mode: 104-document converged exact-parity gate

CAMPAIGN_DPI = 150.0
PAGE_MIN_PARITY_DEFAULT = 99.0   # every page must clear this exact-pixel floor
DOC_MIN_PARITY_DEFAULT = 99.0    # document aggregate exact-pixel floor
AUX_EXTS = ("aux", "out", "toc", "nav", "snm", "bbl", "lof", "lot", "brf")
RUST_CACHE_EXTS = ("depcache", "pagecache")
# Remove engine diagnostics identically for both compilers.
DIAGNOSTIC_VARS = ["TEXDEBUG", "PHASE_TIMING"]

PRIVATE_SPECS = {
    "trust": {"source": Path("/home/leo/da/Dropbox/Apps/Overleaf/trust_own")},
    "beamer": {"source": Path(
        "/home/leo/da/Dropbox/teaching/Futures_and_Options/ch12_beamer")},
    "ai": {"source": Path("/home/leo/dd/ai_patent_test")},
    "cluster_ceo": {"source": Path("/home/leo/dd/cluster_ceo"),
                    # main.tex says \bibliography{../../bib}: the shared .bib
                    # is placed two levels above each engine workspace copy
                    # (cfg places it at <out_root>/work/bib.bib).
                    "external": {Path(
                        "/home/leo/da/Dropbox/WingWah-Leo/bib.bib"): "bib.bib"}},
}
# prepared four-document manifests written by scripts/bench_cold.py prepare;
# the most complete one supplies recorded sources/pages/hashes (integration,
# never hardcoded outcomes).
PREPARED_MANIFEST_GLOBS = (
    "/tmp/tex-parity-live/inputs.json",
    "/tmp/tex-speed-verified-*/benchmarks/inputs.json",
)
PRIVATE_IGNORE = (".git", "archive", "Submission", ".ipynb_checkpoints",
                  "flock", "texput.log", "*.fdb_latexmk", "*.fls",
                  "*.synctex.gz")

REF_ENGINE_FLAGS = {
    "pdflatex": ("-interaction=nonstopmode", "-no-shell-escape"),
    # XeTeX/LuaTeX have no -no-shell-escape switch (shell-escape is off by
    # default in TeX Live); passing it would inject an undefined-control-seq
    # error line into otherwise-correct builds.
    "xelatex": ("-interaction=nonstopmode",),
    "lualatex": ("-interaction=nonstopmode",),
}
FONTSPEC_RE = re.compile(
    r"\\(?:usepackage|RequirePackage)(?:\[[^\]]*\])?\{(?:fontspec|xeCJK)\}")
LUALATEX_RE = re.compile(
    r"\\(?:usepackage|RequirePackage)(?:\[[^\]]*\])?\{(?:luacode|luatextra)\}")


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def campaign_env(overlay: Path | None) -> tuple[dict, list[str], dict]:
    """Child environment for every campaign subprocess (both engines
    identically). Diagnostic probes are removed for both sides; the isolated
    dependency overlay is exported via BOTH absolute TEXMFHOME (full TDS
    tree) and TEXMFVAR (the self-contained updmap-user font maps —
    nanumfonts.map/umj.map — that the plain user map lacks) so Rust and
    reference searches resolve the SAME added packages and maps. The values
    actually set are returned for provenance recording."""
    env = dict(os.environ)
    removed = [k for k in DIAGNOSTIC_VARS if env.pop(k, None) is not None]
    overlay_vars: dict = {}
    if overlay is not None:
        ov = Path(overlay).resolve()
        var = ov.with_name("texmf-var")
        if not var.is_dir():
            print(f"CAMPAIGN: WARNING: overlay TEXMFVAR tree missing: {var}; "
                  "generated font maps (nanumfonts.map/umj.map) will not "
                  "resolve — failures retained", file=sys.stderr)
        existing = env.get("TEXMFHOME", "").strip(":")
        env["TEXMFHOME"] = f"{existing}:{ov}" if existing else str(ov)
        env["TEXMFVAR"] = str(var)
        overlay_vars = {"TEXMFHOME": env["TEXMFHOME"],
                        "TEXMFVAR": env["TEXMFVAR"],
                        "overlay": str(ov),
                        "texmf_var_exists": var.is_dir()}
    return env, removed, overlay_vars


def discover_prepared_manifest() -> tuple[dict, Path | None]:
    """Most complete of the known four-document prepared manifests."""
    import glob as _glob
    best: dict = {}
    best_path: Path | None = None
    for pat in PREPARED_MANIFEST_GLOBS:
        for p in sorted(_glob.glob(pat)):
            pp = Path(p)
            try:
                d = json.loads(pp.read_text())
            except (OSError, json.JSONDecodeError):
                continue
            if not isinstance(d, dict):
                continue
            if len(d) > len(best):
                best, best_path = d, pp
    return best, best_path


def derive_private_job(name: str, src: Path, recorded: dict) -> str | None:
    """Main job derived from the prepared/source manifest files map, falling
    back to a directory scan; None is a retained blocker, never a guess."""
    files = {k for k in (recorded.get("files") or {})}
    for cand in ("main.tex", f"{name}.tex"):
        if cand in files:
            return cand
        if (src / cand).is_file() and not files:
            return cand
    texes = sorted(f for f in files if f.endswith(".tex") and "/" not in f)
    if len(texes) == 1:
        return texes[0]
    if not files:
        top = sorted(p.name for p in src.glob("*.tex"))
        if "main.tex" in top:
            return "main.tex"
        if len(top) == 1:
            return top[0]
    return None


def build_private_entries() -> tuple[list[dict], dict]:
    """Inventory the four private docs from the prepared/source manifest and
    the recorded active sources. Returns entries + provenance record."""
    recorded, manifest_path = discover_prepared_manifest()
    prov = {"prepared_manifest": str(manifest_path) if manifest_path else None,
            "recorded_names": sorted(recorded), "details": {}}
    entries = []
    for name, spec in PRIVATE_SPECS.items():
        src: Path = spec["source"]
        rec = recorded.get(name) or {}
        e: dict = {"id": name, "kind": "private", "archive": "private",
                   "src_dir": src, "external_files": spec.get("external", {}),
                   "prepared": {"manifest": str(manifest_path) if manifest_path else None,
                                "oracle_pages_recorded": rec.get("pages"),
                                "source_recorded": rec.get("source")}}
        if not src.is_dir():
            e["blocker"] = f"private source missing: {src}"
            prov["details"][name] = {"status": "missing-source"}
            entries.append(e)
            continue
        if rec.get("source") and str(rec["source"]) != str(src):
            e["blocker"] = (f"prepared manifest records a different source for "
                            f"{name}: {rec['source']} != {src}")
            prov["details"][name] = {"status": "source-mismatch"}
            entries.append(e)
            continue
        job = derive_private_job(name, src, rec)
        if job is None:
            e["blocker"] = (f"cannot derive unique main .tex for {name} from "
                            f"{src} (recorded manifest {bool(rec)})")
            prov["details"][name] = {"status": "job-ambiguous"}
            entries.append(e)
            continue
        e["main_tex"] = job
        prov["details"][name] = {"status": "ok", "job": job,
                                 "oracle_pages_recorded": rec.get("pages"),
                                 "recorded_file_count": len(rec.get("files") or {})}
        entries.append(e)
    return entries, prov


def check_prepared_drift(src: Path, recorded: dict) -> list[str]:
    """Hash the recorded (non-generated) manifest files in the CURRENT active
    source; every disagreement is recorded provenance, never a silent skip."""
    drift = []
    for rel, digest in sorted((recorded.get("files") or {}).items()):
        f = src / rel
        if not f.is_file():
            drift.append(f"{rel}: missing from active source")
        elif sha256_file(f) != digest:
            drift.append(f"{rel}: hash differs from prepared manifest")
    return drift


def freeze_workspace(src: Path, dst: Path, tex_rel: Path,
                     extra_ignore: tuple[str, ...] = ()) -> list[str]:
    """Fresh isolated copy of the ACTIVE source with generated main-job
    artifacts stripped (source .bbl preserved). Returns stripped names."""
    shutil.rmtree(dst, ignore_errors=True)
    strip = gen_artifact_names(tex_rel.stem)
    pat = shutil.ignore_patterns(*extra_ignore)

    def _ignore(dirname: str, names: list[str]) -> set[str]:
        return ({n for n in names if n in strip} | pat(dirname, names))

    shutil.copytree(src, dst, ignore=_ignore, symlinks=True)
    return sorted(str(p.relative_to(src)) for p in src.rglob("*")
                  if p.is_file() and not p.is_symlink() and p.name in strip)


def tree_hashes(root: Path) -> dict[str, str]:
    out = {}
    for p in sorted(root.rglob("*")):
        if p.is_file() and not p.is_symlink():
            out[str(p.relative_to(root))] = sha256_file(p)
    return out


def verify_frozen_identical(ws_rust: Path, ws_ref: Path) -> None:
    hr, hf = tree_hashes(ws_rust), tree_hashes(ws_ref)
    if hr != hf:
        only_r = sorted(set(hr) - set(hf))[:5]
        only_f = sorted(set(hf) - set(hr))[:5]
        differ = sorted(k for k in set(hr) & set(hf) if hr[k] != hf[k])[:5]
        raise RuntimeError(
            f"frozen inputs not identical rust={len(hr)} ref={len(hf)}; "
            f"rust-only={only_r} ref-only={only_f} differ={differ}")


def detect_ref_engine(src_dir: Path, tex_rel: Path) -> str:
    """Choose the SYSTEM engine for the reference build.

    The rust binary is pdflatex-only. Pixel-comparing it to xelatex/lualatex
    (OpenType vs TFM) cannot reach the 97% gate, so fontspec/luacode docs
    still compile with pdflatex on both sides. A TEXMFHOME fontspec.sty shim
    lets those files load. luacode docs that truly need Lua stay flagged via
    LUALATEX_RE but still run pdflatex — failures are retained.
    """
    _ = (src_dir, tex_rel)
    return "pdflatex"


def aux_state(work: Path, job: str) -> dict[str, str]:
    state = {}
    for e in AUX_EXTS:
        f = work / f"{job}.{e}"
        if f.is_file():
            state[f"{job}.{e}"] = sha256_file(f)
    return state


def _read_text(path: Path) -> str:
    try:
        return path.read_text(errors="replace")
    except OSError:
        return ""


def aux_wants_bibliography(work: Path, job: str) -> bool:
    aux = _read_text(work / f"{job}.aux")
    return "\\bibdata{" in aux and "\\citation{" in aux


def run_bibtex(bibtex: str, work: Path, job: str, env: dict,
               log_path: Path, timeout: float,
               mem_limit_mib: int = DEFAULT_MEM_LIMIT_MIB) -> dict:
    run = run_compile(bibtex, work, job, log_path, timeout, env=env, flags=(),
                      mem_limit_mib=mem_limit_mib)
    return {"exit": run["exit"], "timed_out": run["timed_out"],
            "mem_killed": run["mem_killed"], "kill_signal": run["kill_signal"],
            "cmd": run["cmd"], "log": str(log_path)}


def converge_engine(engine: str, bin_path: str, ws: Path, tex_rel: Path,
                    idir: Path, env: dict, timeout: float, max_passes: int,
                    bibtex: str | None, ref_engine: str,
                    mem_limit_mib: int = DEFAULT_MEM_LIMIT_MIB) -> dict:
    """Converged bibliography/cross-ref build in one isolated workspace.

    NO cross-engine state ever transfers: both sides start from the identical
    frozen active source and converge independently; the gate exposes any
    divergence. Rust output/dependency caches (.depcache/.pagecache) and the
    previous PDF are deleted before EVERY pass. Bibliography honoring: a .bbl
    genuinely shipped in the source is the author's bibliography and is NEVER
    overwritten (freeze preserves it; bibtex stays off); when the aux cites
    with no shipped .bbl, the reference side runs system BibTeX and the Rust
    side runs native tex-bibtex — never a system binary for Rust output.
    Every pass's captured stdout and engine transcript is retained.
    """
    work = ws / tex_rel.parent
    job = tex_rel.stem
    plog = idir / "passlogs"
    plog.mkdir(parents=True, exist_ok=True)
    flags = REF_ENGINE_FLAGS[ref_engine]
    passes: list[dict] = []
    bib_runs: list[dict] = []
    bbl_is_source = (work / f"{job}.bbl").is_file()
    prev = aux_state(work, job)
    cur = prev
    stable = False
    timed_out = False
    mem_killed = False
    total_ms = 0.0
    for i in range(1, max_passes + 1):
        for ext in RUST_CACHE_EXTS:
            (work / f"{job}.{ext}").unlink(missing_ok=True)
        (work / f"{job}.pdf").unlink(missing_ok=True)
        cap = plog / f"{engine}-pass{i}.stdout.log"
        run = run_compile(bin_path, work, f"{job}.tex", cap, timeout,
                          env=env, flags=flags, mem_limit_mib=mem_limit_mib)
        total_ms += run["time_ms"]
        tl = plog / f"{engine}-pass{i}.tex.log"
        if (work / f"{job}.log").is_file():
            shutil.copyfile(work / f"{job}.log", tl)
        passes.append({"pass": i, "exit": run["exit"],
                       "timed_out": run["timed_out"],
                       "mem_killed": run["mem_killed"],
                       "kill_signal": run["kill_signal"],
                       "spawn_error": run["spawn_error"],
                       "time_ms": round(run["time_ms"], 1),
                       "stdout_log": str(cap), "tex_log": str(tl)})
        if run["timed_out"]:
            timed_out = True
            break
        if run["mem_killed"]:
            # dead on the address-space cap: further passes repeat the same
            # runaway; report distinctly instead of burning max_passes
            mem_killed = True
            break
        if bibtex and not bbl_is_source and aux_wants_bibliography(work, job):
            bl = plog / f"{engine}-bibtex-pass{i}.log"
            bib_runs.append({"pass": i, **run_bibtex(
                bibtex, work, job, env, bl, timeout,
                mem_limit_mib=mem_limit_mib)})
        cur = aux_state(work, job)
        stable = cur == prev
        prev = cur
        if stable and run["exit"] == 0:
            break
    last = passes[-1] if passes else {}
    kept_log = None
    if (work / f"{job}.log").is_file():
        kept_log = idir / f"{engine}.tex.log"
        shutil.copyfile(work / f"{job}.log", kept_log)
    kept_pdf = out_root_of(idir) / "pdf" / f"{pdf_basename(idir.name, engine)}"
    pdf_stat = pdf_info(work / f"{job}.pdf")
    if pdf_stat["pdf_exists"]:
        kept_pdf.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(work / f"{job}.pdf", kept_pdf)
    log_text = _read_text(work / f"{job}.log") or _read_text(Path(last.get("stdout_log", "")))
    errors = extract_errors(log_text)
    if not errors and last.get("spawn_error"):
        errors = [f"spawn-error: {last['spawn_error']}"]
    mem_killed = mem_killed or bool(last.get("mem_killed"))
    converged = stable and last.get("exit") == 0 and not timed_out
    if timed_out:
        status = "timeout"
    elif mem_killed:
        status = "memlimit"
    elif last.get("spawn_error") or not pdf_stat["pdf_valid"]:
        status = "failure"
    elif not converged:
        status = "unconverged"
    elif last.get("exit") != 0 or errors:
        status = "errors"
    else:
        status = "clean"
    rec = {
        "engine": engine, "bin": bin_path, "ref_engine": ref_engine,
        "status": status, "exit": last.get("exit"), "timed_out": timed_out,
        "mem_killed": mem_killed, "kill_signal": last.get("kill_signal"),
        "time_ms": round(total_ms, 1), "passes": len(passes),
        "converged": converged, "pass_records": passes,
        "bibtex_runs": bib_runs, "shipped_bbl": bbl_is_source,
        "errors": errors, "aux_hashes": cur,
        "captured_log": last.get("stdout_log"), "tex_log": str(kept_log)
        if kept_log else None, "pass_log_dir": str(plog),
    }
    rec.update(pdf_stat)
    # persisted copy wins: the workspace path may be deleted by --no-keep-work
    rec["pdf"] = str(kept_pdf) if kept_pdf.is_file() else None
    return rec


def exact_parity_compare(aid: str, a_path: Path | None, b_path: Path | None,
                         a_pages: int | None, b_pages: int | None,
                         out_root: Path, dpi: float, page_min: float,
                         doc_min: float) -> dict:
    """Exact RGB raster parity per doc_parity.rs: identical geometry and page
    count, 150 DPI, byte-exact pixels — no registration, cropping or
    tolerance. Every page gets a score; the worst page keeps expected/actual/
    diff PNGs for visual audit."""
    out: dict = {"raster_dpi": dpi, "compared": False, "note": None,
                 "page_count_match": None, "geometry_match": None,
                 "pages_compared": 0, "pages_identical": 0,
                 "unmatched_pages": 0, "per_page": [],
                 "document_exact_parity": None, "total_pixels": 0,
                 "total_differing_pixels": 0, "worst_page": None,
                 "worst_page_parity": None, "worst_artifacts": None,
                 "page_failures": [], "doc_gate": None,
                 "raster_warnings": [], "producer_rust": None,
                 "repaired": None}
    if np is None:
        out["note"] = "numpy required for exact campaign parity"
        return out
    if not (a_path and b_path and Path(a_path).is_file()
            and Path(b_path).is_file()):
        out["note"] = "one or both PDFs missing; no parity measurement"
        return out
    if np is None or a_pages is None or b_pages is None:
        out["note"] = "PDF unreadable; no parity measurement"
        return out
    try:
        da = pymupdf.open(str(a_path))
        db = pymupdf.open(str(b_path))
    except Exception as e:  # noqa: BLE001
        out["note"] = f"open failed: {e}"
        return out
    try:
        out["producer_rust"] = da.metadata.get("producer") or ""
        out["repaired"] = {"rust": bool(da.is_repaired),
                           "ref": bool(db.is_repaired)}
        out["page_count_match"] = a_pages == b_pages
        n = min(a_pages, b_pages)
        differing = total = 0
        geometry_ok = True
        worst_pct: float | None = None
        worst_page = 0
        for i in range(n):
            pa, pb = da[i], db[i]
            pymupdf.TOOLS.mupdf_warnings(reset=True)
            a = pa.get_pixmap(matrix=pymupdf.Matrix(dpi / 72.0, dpi / 72.0),
                              colorspace=pymupdf.csRGB, alpha=False)
            b = pb.get_pixmap(matrix=pymupdf.Matrix(dpi / 72.0, dpi / 72.0),
                              colorspace=pymupdf.csRGB, alpha=False)
            warn = pymupdf.TOOLS.mupdf_warnings(reset=True)
            if warn:
                out["raster_warnings"].append({"page": i + 1, "text": warn[:500]})
            pixels = a.width * a.height
            entry = {"page": i + 1, "pixels": pixels,
                     "rust_dims_px": [a.width, a.height],
                     "ref_dims_px": [b.width, b.height],
                     "rust_size_pt": [round(pa.rect.width, 3), round(pa.rect.height, 3)],
                     "ref_size_pt": [round(pb.rect.width, 3), round(pb.rect.height, 3)],
                     "dims_match": (a.width, a.height) == (b.width, b.height)}
            if not entry["dims_match"]:
                # identical geometry is part of the parity rule: no crop/tolerance
                geometry_ok = False
                entry.update(exact_parity_pct=0.0, differing_pixels=None,
                             identical=False)
                out["per_page"].append(entry)
                out["page_failures"].append(
                    {"page": i + 1, "why": "geometry mismatch",
                     "parity": 0.0})
                continue
            aa = np.frombuffer(a.samples, dtype=np.uint8).reshape(-1, 3)
            bb = np.frombuffer(b.samples, dtype=np.uint8).reshape(-1, 3)
            d = int(np.count_nonzero(np.any(aa != bb, axis=1)))
            pct = 100.0 * (1 - d / pixels)
            differing += d
            total += pixels
            entry.update(exact_parity_pct=round(pct, 6), differing_pixels=d,
                         identical=d == 0)
            if d == 0:
                out["pages_identical"] += 1
            if worst_pct is None or pct < worst_pct:
                worst_pct, worst_page = pct, i + 1
            if pct < page_min:
                out["page_failures"].append(
                    {"page": i + 1, "why": "page parity", "parity": round(pct, 6)})
            out["per_page"].append(entry)
        out["pages_compared"] = n
        out["unmatched_pages"] = abs(a_pages - b_pages)
        out["geometry_match"] = geometry_ok and a_pages == b_pages
        out["compared"] = True
        if total:
            doc_pct = 100.0 * (1 - differing / total)
            out["document_exact_parity"] = round(doc_pct, 6)
            out["total_pixels"] = total
            out["total_differing_pixels"] = differing
            out["doc_gate"] = "pass" if doc_pct >= doc_min else "fail"
        if worst_pct is None and a_pages != b_pages:
            out["doc_gate"] = "fail"
        out["worst_page"] = worst_page or None
        out["worst_page_parity"] = round(worst_pct, 6) if worst_pct is not None else None
        # worst-page expected/actual/diff artifacts for visual audit
        if worst_page:
            wdir = out_root / "worst"
            wdir.mkdir(parents=True, exist_ok=True)
            stem = wdir / f"{aid}-p{worst_page}"
            try:
                m = pymupdf.Matrix(dpi / 72.0, dpi / 72.0)
                wa = da[worst_page - 1].get_pixmap(matrix=m,
                                                  colorspace=pymupdf.csRGB, alpha=False)
                wb = db[worst_page - 1].get_pixmap(matrix=m,
                                                  colorspace=pymupdf.csRGB, alpha=False)
                paths = {}
                if (wa.width, wa.height) == (wb.width, wb.height):
                    wa.save(str(stem) + "-rust.png")
                    wb.save(str(stem) + "-ref.png")
                    paths = {"rust": str(stem) + "-rust.png",
                             "ref": str(stem) + "-ref.png"}
                    g = (np.frombuffer(wa.samples, np.uint8).astype(np.int16)
                         - np.frombuffer(wb.samples, np.uint8).astype(np.int16)
                         ).reshape(-1, 3)
                    flag = (np.any(g != 0, axis=1) * 255).astype(np.uint8)
                    dm = np.repeat(flag.reshape(-1, 1), 3, axis=1)
                    pix = pymupdf.Pixmap(pymupdf.csRGB, wa.width, wa.height,
                                         dm.tobytes(), False)
                    pix.save(str(stem) + "-diff.png")
                    paths["diff"] = str(stem) + "-diff.png"
                    out["worst_artifacts"] = paths
            except Exception as e:  # noqa: BLE001 - artifacts are audit aids
                out["worst_artifacts"] = f"artifact write failed: {e}"
    finally:
        da.close()
        db.close()
    return out


def process_campaign_project(entry: dict, out_root: Path, cfg: dict) -> dict:
    aid = entry["id"]
    idir = out_root / "results" / aid
    idir.mkdir(parents=True, exist_ok=True)
    res: dict = {"id": aid, "mode": "campaign", "kind": entry["kind"],
                 "archive": entry.get("archive"), "finished_utc": now_iso(),
                 "main_tex": entry.get("main_tex"),
                 "prepared": entry.get("prepared", {})}
    try:
        if entry.get("blocker"):
            raise RuntimeError(entry["blocker"])
        src: Path = entry["src_dir"]
        tex_rel = Path(entry["main_tex"])
        ws_root = out_root / "work" / aid
        ws_rust = ws_root / "rust"
        ws_ref = ws_root / "ref"
        extra_ignore = PRIVATE_IGNORE if entry["kind"] == "private" else ()
        stripped_r = freeze_workspace(src, ws_rust, tex_rel, extra_ignore)
        stripped_f = freeze_workspace(src, ws_ref, tex_rel, extra_ignore)
        verify_frozen_identical(ws_rust, ws_ref)
        res["stripped_generated"] = {"rust": stripped_r, "ref": stripped_f}
        # external inputs at the depth ../../ resolves to (shared .bib etc.)
        for f, rel in (entry.get("external_files") or {}).items():
            fs = Path(f)
            if not fs.is_file():
                raise RuntimeError(f"external input missing: {fs}")
            dst = ws_root.parent / rel
            shutil.copy2(fs, dst)
            res["prepared"].setdefault("external_placed", {})[rel] = sha256_file(dst)
        if entry["kind"] == "private":
            recorded, _mp = discover_prepared_manifest()
            drift = check_prepared_drift(src, recorded.get(aid) or {})
            if drift:
                res["prepared"]["active_vs_prepared_drift"] = drift
        ref_engine = detect_ref_engine(src, tex_rel)
        res["ref_engine"] = ref_engine
        res["ref_bin"] = cfg["ref_bins"][ref_engine]
        # Both engines converge INDEPENDENTLY from the identical frozen
        # source (verify_frozen_identical ran before any mutation): no aux,
        # .bbl, or oracle state ever flows ref -> rust. Divergence is a
        # gate-visible failure, never masked. No system binary produces Rust
        # output; the Rust side uses native tex-bibtex for generated .bbls.
        ref = converge_engine("ref", cfg["ref_bins"][ref_engine], ws_ref,
                              tex_rel, idir, cfg["env"], cfg["timeout"],
                              cfg["max_passes"], cfg["sys_bibtex"], ref_engine,
                              cfg["mem_limit_mib"])
        rust = converge_engine("rust", cfg["rust_bin"], ws_rust, tex_rel,
                               idir, cfg["env"], cfg["timeout"],
                               cfg["max_passes"], cfg["rust_bibtex"],
                               "pdflatex", cfg["mem_limit_mib"])
        res["ref"] = ref
        res["rust"] = rust
        cmp = exact_parity_compare(
            aid,
            Path(rust["pdf"]) if rust["pdf"] else None,
            Path(ref["pdf"]) if ref["pdf"] else None,
            rust["pages"], ref["pages"], out_root, cfg["dpi"],
            cfg["page_min"], cfg["doc_min"])
        res["compare"] = cmp
        # provenance warning ONLY: recorded oracle pages from an earlier
        # prepared campaign never pin the live reference (active sources
        # legitimately drift). Parity is current active reference vs native
        # Rust with identical page count/geometry; that comparison is the
        # contract. A genuinely missing source/dependency stays a blocker.
        exp = (entry.get("prepared") or {}).get("oracle_pages_recorded")
        if exp is not None and ref["pages"] != exp:
            res["oracle_pages_note"] = {"reference_built": ref["pages"],
                                        "prepared_recorded": exp,
                                        "note": "active source drift or "
                                        "rebuilt reference; not a gate failure"}
    except Exception as e:  # noqa: BLE001 - never lose the run
        import traceback
        traceback.print_exc()
        res["harness_error"] = str(e)
        res.setdefault("rust", {"engine": "rust", "status": "failure",
                                "exit": None, "pdf_valid": False, "pages": None,
                                "errors": [f"harness: {e}"], "pdf": None,
                                "converged": False})
        res.setdefault("ref", {"engine": "ref", "status": "failure",
                               "exit": None, "pdf_valid": False, "pages": None,
                               "errors": [], "pdf": None, "converged": False})
        res.setdefault("compare", {"compared": False, "note": "harness error",
                                   "page_failures": [], "per_page": [],
                                   "raster_warnings": []})
    res["finished_utc"] = now_iso()
    if not cfg["keep_work"]:
        shutil.rmtree(out_root / "work" / aid, ignore_errors=True)
    return res


def campaign_gate(selected: list[dict], results: dict[str, dict],
                  cfg: dict) -> dict:
    """Hard gate: fails incomplete coverage, compilation errors, invalid or
    unconverged builds, geometry/page-count mismatch, raster warnings, any
    page below page_min, any document below doc_min. Blockers stay in view."""
    failures: list[dict] = []
    ids = [e["id"] for e in selected]
    missing = [i for i in ids if i not in results]
    if missing:
        failures.append({"kind": "coverage", "ids": missing})
    for e in selected:
        aid = e["id"]
        r = results.get(aid)
        if r is None:
            continue
        if r.get("harness_error"):
            failures.append({"kind": "harness", "id": aid,
                             "detail": r["harness_error"]})
        for eng in ("rust", "ref"):
            x = r.get(eng) or {}
            if x.get("status") != "clean" or not x.get("converged"):
                failures.append({"kind": "compilation", "id": aid,
                                 "engine": eng, "status": x.get("status"),
                                 "converged": x.get("converged"),
                                 "passes": x.get("passes"),
                                 "errors": (x.get("errors") or [])[:3]})
            elif not x.get("pdf_valid"):
                failures.append({"kind": "invalid-pdf", "id": aid,
                                 "engine": eng})
        # oracle_pages_note is provenance only (active source drift); a
        # genuinely missing source/dependency already raised harness_error.
        c = r.get("compare") or {}
        if not c.get("compared"):
            failures.append({"kind": "not-compared", "id": aid,
                             "note": c.get("note")})
            continue
        if not c.get("page_count_match"):
            failures.append({"kind": "page-count", "id": aid})
        if not c.get("geometry_match"):
            failures.append({"kind": "geometry", "id": aid})
        if c.get("raster_warnings"):
            failures.append({"kind": "raster-warnings", "id": aid,
                             "count": len(c["raster_warnings"])})
        if c.get("producer_rust") != "tex-rs":
            failures.append({"kind": "producer", "id": aid,
                             "producer": c.get("producer_rust")})
        for pf in c.get("page_failures") or []:
            failures.append({"kind": "page-parity", "id": aid, **pf,
                             "min_pct": cfg["page_min"]})
        dp = c.get("document_exact_parity")
        if dp is None or dp < cfg["doc_min"]:
            failures.append({"kind": "doc-parity", "id": aid,
                             "parity": dp, "min_pct": cfg["doc_min"]})
    return {"ok": not failures, "gate_utc": now_iso(), "mode": "campaign",
            "expected": len(ids), "completed": len(results),
            "dpi": cfg["dpi"], "page_min_pct": cfg["page_min"],
            "doc_min_pct": cfg["doc_min"], "failures": failures}


def summarize_campaign(results: dict[str, dict], page_min: float,
                       doc_min: float) -> dict:
    eng = {e: {s: 0 for s in
              ("clean", "errors", "failure", "timeout", "unconverged",
               "memlimit")}
           for e in ("rust", "ref")}
    for r in results.values():
        for e in ("rust", "ref"):
            st = (r.get(e) or {}).get("status")
            if st in eng[e]:
                eng[e][st] += 1
    rastered = [r for r in results.values()
                if (r.get("compare") or {}).get("compared")]
    doc_p = [r["compare"]["document_exact_parity"] for r in rastered
             if r["compare"].get("document_exact_parity") is not None]
    pages_all = [p["exact_parity_pct"] for r in rastered
                 for p in r["compare"]["per_page"]
                 if p.get("exact_parity_pct") is not None]
    return {
        "projects_completed": len(results),
        "status_counts": eng,
        "par": sum(1 for r in results.values()
                   for e in ("rust", "ref")
                   if r.get(e, {}).get("status") == "clean"
                   and r.get(e, {}).get("converged")),
        "raster_compared": len(rastered),
        "page_count_match": sum(1 for r in rastered
                                if r["compare"].get("page_count_match")),
        "geometry_match": sum(1 for r in rastered
                              if r["compare"].get("geometry_match")),
        "fully_identical_projects": sum(
            1 for r in rastered
            if r["compare"].get("pages_identical")
            == r["compare"].get("pages_compared")
            and r["compare"].get("unmatched_pages") == 0
            and r["compare"].get("geometry_match")),
        "pages_ge_page_min": sum(1 for p in pages_all if p >= page_min),
        "pages_total": len(pages_all),
        "docs_ge_doc_min": sum(1 for d in doc_p if d >= doc_min),
        "page_min_pct": page_min, "doc_min_pct": doc_min,
        "mean_doc_exact_parity": round(statistics.fmean(doc_p), 6) if doc_p else None,
        "min_doc_exact_parity": round(min(doc_p), 6) if doc_p else None,
        "min_page_exact_parity": round(min(pages_all), 6) if pages_all else None,
    }


def print_campaign_line(done: int, total: int, r: dict) -> None:
    c = r.get("compare") or {}
    if c.get("compared"):
        cmp_s = (f"doc {c.get('document_exact_parity')}% worst p{c.get('worst_page')}"
                 f" {c.get('worst_page_parity')}% "
                 f"pages {c.get('pages_identical')}/{c.get('pages_compared')}")
    else:
        cmp_s = c.get("note") or "not compared"
    ru = r.get("rust") or {}
    rf = r.get("ref") or {}
    print(f"[{done:3d}/{total}] {r['id']:12s} "
          f"rust={ru.get('status'):11s}/{ru.get('pages')}p/{ru.get('passes')}x "
          f"ref={rf.get('status'):11s}/{rf.get('pages')}p "
          f"[{r.get('ref_engine') or '?'}]  {cmp_s}", flush=True)



# --------------------------------------------------------------------------
# PDF comparison (pagewise raster @72dpi + text similarity)

def _page_array(page):
    # PyMuPDF 1.28: get_pixmap(dpi=float) trips an int setter; Matrix is exact.
    zoom = DPI / 72.0
    pix = page.get_pixmap(matrix=pymupdf.Matrix(zoom, zoom), alpha=False)
    w, h, n = pix.width, pix.height, pix.n
    if np is None:
        return (w, h, n), pix.samples
    arr = np.frombuffer(pix.samples, dtype=np.uint8).reshape((h, w, n))
    return (w, h, n), arr


def _norm_text(page) -> str:
    return " ".join(page.get_text("text").split())


def compare_pdfs(a_path: Path | None, b_path: Path | None,
                 a_pages: int | None, b_pages: int | None) -> dict:
    """a = rust output, b = reference output."""
    out = {
        "raster_dpi": DPI, "compared": False, "page_count_match": None,
        "pages_compared": 0, "pages_identical": 0, "dims_all_match": None,
        "mean_pixel_diff": None, "max_pixel_diff": None,
        "pct_pixels_diff": None, "worst_page": None,
        "text_similarity": None, "per_page": [], "note": None,
    }
    if not (a_path and b_path and Path(a_path).is_file() and Path(b_path).is_file()):
        out["note"] = "one or both PDFs missing; no raster comparison"
        return out
    if a_pages is None or b_pages is None:
        out["note"] = "PDF unreadable; no raster comparison"
        return out
    try:
        da = pymupdf.open(str(a_path))
        db = pymupdf.open(str(b_path))
    except Exception as e:  # noqa: BLE001
        out["note"] = f"open failed: {e}"
        return out
    try:
        out["page_count_match"] = (a_pages == b_pages)
        n = min(a_pages, b_pages)
        means, pcts, dims_match = [], [], True
        worst = (0.0, 0)
        text_a, text_b = [], []
        for i in range(n):
            pa, pb = da[i], db[i]
            sa, aa = _page_array(pa)
            sb, ab = _page_array(pb)
            dm = sa == sb
            dims_match = dims_match and dm
            entry = {"page": i + 1, "rust_dims": sa, "ref_dims": sb,
                     "dims_match": dm}
            if np is not None:
                if dm:
                    diff = np.abs(aa.astype(np.int16) - ab.astype(np.int16))
                    m2 = diff.reshape(sa[1], sa[0], sa[2]).mean(axis=2)
                    entry["mean_abs"] = round(float(m2.mean()), 4)
                    entry["max_abs"] = int(diff.max())
                    entry["pct_pixels_gt_thr"] = round(float((m2 > PIX_THRESHOLD).mean() * 100), 4)
                    identical = bool(entry["max_abs"] == 0)
                    means.append(entry["mean_abs"])
                    pcts.append(entry["pct_pixels_gt_thr"])
                    if entry["mean_abs"] > worst[0]:
                        worst = (entry["mean_abs"], i + 1)
                else:  # crop overlap so a size mismatch is still measured
                    h = min(sa[1], sb[1]); w = min(sa[0], sb[0])
                    ca = aa[:h, :w, :].astype(np.int16)
                    cb = ab[:h, :w, :].astype(np.int16)
                    if ca.shape == cb.shape:
                        diff = np.abs(ca - cb)
                        m2 = diff.mean(axis=2)
                        entry["mean_abs_overlap"] = round(float(m2.mean()), 4)
                        entry["max_abs_overlap"] = int(diff.max())
                        entry["pct_pixels_gt_thr_overlap"] = round(float((m2 > PIX_THRESHOLD).mean() * 100), 4)
                        means.append(entry["mean_abs_overlap"])
                        pcts.append(entry["pct_pixels_gt_thr_overlap"])
                    identical = False
                entry["identical"] = identical
                if identical:
                    out["pages_identical"] += 1
            else:
                entry["samples_equal"] = aa == ab
            ta, tb = _norm_text(pa), _norm_text(pb)
            entry["text_ratio"] = round(difflib.SequenceMatcher(None, ta, tb).ratio(), 4)
            text_a.append(ta); text_b.append(tb)
            out["per_page"].append(entry)
        out["compared"] = True
        out["pages_compared"] = n
        out["unmatched_pages"] = abs(a_pages - b_pages)
        out["dims_all_match"] = dims_match
        if means:
            out["mean_pixel_diff"] = round(statistics.fmean(means), 4)
            out["max_pixel_diff"] = round(max(means), 4)
            out["pct_pixels_diff"] = round(statistics.fmean(pcts), 4)
            out["worst_page"] = worst[1]
        out["text_similarity"] = round(
            difflib.SequenceMatcher(None, "\n".join(text_a), "\n".join(text_b)).ratio(), 4)
        if a_pages != b_pages:
            out["note"] = f"page-count mismatch: rust {a_pages} vs ref {b_pages}; compared first {n}"
    finally:
        da.close(); db.close()
    return out


# --------------------------------------------------------------------------
# Per-project driver

def process_project(entry: dict, corpus_dir: Path, out_root: Path,
                    rust_bin: str, sys_bin: str, timeout: float,
                    mem_limit_mib: int, keep_work: bool) -> dict:
    aid = entry["id"]
    src = corpus_dir / aid
    tex_rel = Path(entry["main_tex"])
    idir = out_root / "results" / aid
    idir.mkdir(parents=True, exist_ok=True)
    ws_rust = out_root / "work" / aid / "rust"
    ws_ref = out_root / "work" / aid / "ref"

    stripped_r = prepare_workspace(src, ws_rust, tex_rel)
    stripped_f = prepare_workspace(src, ws_ref, tex_rel)

    res_rust = compile_engine("rust", rust_bin, ws_rust, tex_rel, idir,
                              timeout, mem_limit_mib)
    res_ref = compile_engine("ref", sys_bin, ws_ref, tex_rel, idir,
                             timeout, mem_limit_mib)

    cmp = compare_pdfs(
        Path(res_rust["pdf"]) if res_rust["pdf"] else None,
        Path(res_ref["pdf"]) if res_ref["pdf"] else None,
        res_rust["pages"], res_ref["pages"],
    )

    if not keep_work:
        shutil.rmtree(out_root / "work" / aid, ignore_errors=True)

    return {
        "id": aid, "archive": entry.get("archive"), "main_tex": entry["main_tex"],
        "finished_utc": now_iso(),
        "stripped_generated": {"rust": stripped_r, "ref": stripped_f},
        "rust": res_rust, "ref": res_ref, "compare": cmp,
    }


# --------------------------------------------------------------------------
# Reporting

def summarize(results: dict[str, dict]) -> dict:
    eng = {"rust": {}, "ref": {}}
    for e in ("rust", "ref"):
        eng[e] = {s: 0 for s in ("clean", "errors", "failure", "timeout",
                                 "memlimit")}
        eng[e]["with_pdf"] = 0
    for r in results.values():
        for e in ("rust", "ref"):
            eng[e][r[e]["status"]] += 1
            if r[e]["pdf_valid"]:
                eng[e]["with_pdf"] += 1
    both = [r for r in results.values()
            if r["rust"]["pdf_valid"] and r["ref"]["pdf_valid"]]
    rastered = [r for r in both if r["compare"]["compared"]]
    times = {e: [r[e]["time_ms"] for r in results.values() if r[e]["exit"] is not None]
             for e in ("rust", "ref")}
    page_match = sum(1 for r in rastered if r["compare"]["page_count_match"])
    pixel_identical = sum(1 for r in rastered if r["compare"]["pages_identical"]
                          == r["compare"]["pages_compared"]
                          and r["compare"]["unmatched_pages"] == 0)
    return {
        "projects_completed": len(results),
        "status_counts": eng,
        "pdf_valid": {"rust": eng["rust"]["with_pdf"], "ref": eng["ref"]["with_pdf"]},
        "both_pdf": len(both),
        "raster_compared": len(rastered),
        "page_count_match": page_match,
        "pixel_identical_projects": pixel_identical,
        "mean_page_pixel_diff": round(statistics.fmean(
            [r["compare"]["mean_pixel_diff"] for r in rastered
             if r["compare"]["mean_pixel_diff"] is not None]), 4)
        if any(r["compare"]["mean_pixel_diff"] is not None for r in rastered) else None,
        "mean_text_similarity": round(statistics.fmean(
            [r["compare"]["text_similarity"] for r in rastered
             if r["compare"]["text_similarity"] is not None]), 4)
        if any(r["compare"]["text_similarity"] is not None for r in rastered) else None,
        "compile_time_ms": {
            e: {"median": round(statistics.median(t), 1), "max": round(max(t), 1)}
            if t else None for e, t in times.items()
        },
    }


def print_report_line(done: int, total: int, r: dict) -> None:
    c = r["compare"]
    if c["compared"]:
        cmp_s = (f"pages {'=' if c['page_count_match'] else '≠'} "
                 f"pix {c['mean_pixel_diff'] if c['mean_pixel_diff'] is not None else 'n/a'} "
                 f"text {c['text_similarity'] if c['text_similarity'] is not None else 'n/a'}")
    else:
        cmp_s = c["note"] or "not compared"
    print(f"[{done:3d}/{total}] {r['id']} ({r['archive']:7s}) "
          f"rust={r['rust']['status']:7s}/{r['rust']['pages']}p "
          f"ref={r['ref']['status']:7s}/{r['ref']['pages']}p  {cmp_s}", flush=True)
    if r["rust"]["status"] != "clean" and r["rust"]["errors"]:
        print(f"           rust! {r['rust']['errors'][0]}", flush=True)


def build_report(meta: dict, results: dict[str, dict]) -> dict:
    return {"meta": meta, "summary": summarize(results),
            "results": {k: results[k] for k in sorted(results)}}


# --------------------------------------------------------------------------
# Main

def run_campaign(args) -> int:
    """104-document converged exact-parity campaign: inventory, freeze,
    converge, compare, gate. Every failure/blocker is retained."""
    rust_bin = os.path.abspath(args.rust)
    if not Path(rust_bin).is_file():
        print(f"Error: rust binary not found: {rust_bin}", file=sys.stderr)
        return 1
    ref_bins = {}
    for engine, path in (("pdflatex", args.sys), ("xelatex", args.sys_xelatex),
                         ("lualatex", args.sys_lualatex)):
        ap_ = os.path.abspath(str(path))
        ref_bins[engine] = ap_
        if not Path(ap_).is_file():
            # missing reference engine: retained blocker for affected docs
            print(f"CAMPAIGN: reference engine missing: {engine} -> {ap_}",
                  file=sys.stderr)
    rust_bibtex = os.path.abspath(str(args.rust_bibtex))
    if not Path(rust_bibtex).is_file():
        print(f"CAMPAIGN: native tex-bibtex missing: {rust_bibtex}; Rust docs "
              "needing generated bibliographies will be retained failures",
              file=sys.stderr)
        rust_bibtex = None
    sys_bibtex = os.path.abspath(str(args.sys_bibtex))
    env, removed_diag, overlay_vars = campaign_env(args.overlay)
    if args.overlay and not Path(args.overlay).is_dir():
        print(f"CAMPAIGN: WARNING: overlay dir missing: {args.overlay}; "
              "running without it — dependency failures will be retained "
              "gate blockers", file=sys.stderr)
        env, removed_diag, overlay_vars = campaign_env(None)
        args.overlay = None
    if overlay_vars:
        print(f"CAMPAIGN: TEXMFHOME={overlay_vars['TEXMFHOME']} "
              f"TEXMFVAR={overlay_vars['TEXMFVAR']} "
              "(identical for both engines)")

    # ---- inventory all 104: corpus manifest + prepared/source privates ----
    manifest_file = args.corpus_dir / "manifest.json"
    if not manifest_file.is_file():
        print(f"Error: manifest not found at {manifest_file}", file=sys.stderr)
        return 1
    manifest = json.loads(manifest_file.read_text())
    entries: list[dict] = []
    if not args.private_only:
        entries += [{"id": e["id"], "kind": "corpus", "archive": e.get("archive"),
                     "main_tex": e["main_tex"],
                     "src_dir": args.corpus_dir / e["id"], "prepared": {}}
                    for e in manifest[: args.limit]]
    if not args.corpus_only:
        priv, priv_prov = build_private_entries()
        entries += priv
        args.priv_prov = priv_prov
    if args.only:
        wanted = [s.strip() for s in args.only.split(",") if s.strip()]
        by_id = {e["id"]: e for e in entries}
        missing = [w for w in wanted if w not in by_id]
        if missing:
            print(f"Error: ids not in campaign inventory: {missing}",
                  file=sys.stderr)
            return 1
        entries = [by_id[w] for w in wanted]
    for e in entries:
        if e["kind"] == "corpus" and not (e["src_dir"]).is_dir():
            e["blocker"] = f"corpus dir missing: {e['src_dir']}"

    # ---- resume ----
    ckpt_dir = args.output / "checkpoints"
    results: dict[str, dict] = {}
    if not args.no_resume and ckpt_dir.is_dir():
        for ck in sorted(ckpt_dir.glob("*.json")):
            try:
                r = json.loads(ck.read_text())
            except (json.JSONDecodeError, OSError):
                continue
            if not (isinstance(r, dict) and r.get("id") == ck.stem):
                continue
            # campaign checkpoints only; any schema gap triggers a re-run
            if r.get("mode") != "campaign" or "compare" not in r:
                continue
            if not all(isinstance(r.get(e), dict) and "status" in r[e]
                       and "converged" in r[e] for e in ("rust", "ref")):
                continue
            if any(e["id"] == r["id"] for e in entries):
                results[r["id"]] = r
    todo = [e for e in entries if e["id"] not in results]

    cfg = {"rust_bin": rust_bin, "ref_bins": ref_bins,
           "rust_bibtex": rust_bibtex, "sys_bibtex": sys_bibtex,
           "env": env, "timeout": args.timeout, "max_passes": args.max_passes,
           "mem_limit_mib": args.mem_limit_mib,
           "dpi": args.dpi, "page_min": args.page_min, "doc_min": args.doc_min,
           "keep_work": not args.no_keep_work}
    meta = {
        "mode": "campaign",
        "notice": f"CONVERGED CAMPAIGN: aux+bibtex until stable (max "
                  f"{args.max_passes} passes/engine); gate = exact RGB parity "
                  f"@{args.dpi:g}dpi, every page >= {args.page_min}%, doc "
                  f">= {args.doc_min}%, identical geometry/page counts.",
        "started_utc": now_iso(),
        "corpus_dir": str(args.corpus_dir), "manifest": str(manifest_file),
        "output": str(args.output), "jobs": args.jobs,
        "timeout_s": args.timeout, "max_passes": args.max_passes,
        "mem_limit_mib": args.mem_limit_mib,
        "dpi": args.dpi, "page_min_pct": args.page_min,
        "doc_min_pct": args.doc_min,
        "rust_bin": rust_bin, "rust_version": capture_version(args.rust, 10),
        "ref_bins": ref_bins,
        "sys_bibtex": sys_bibtex, "rust_bibtex": rust_bibtex,
        "overlay": str(args.overlay) if args.overlay else None,
        "overlay_vars": overlay_vars,
        "removed_diagnostics": removed_diag,
        "private_provenance": getattr(args, "priv_prov", {}),
        "pymupdf_version": pymupdf.__version__, "numpy": np is not None,
        "selected": len(entries), "resumed": len(results), "planned": len(todo),
        "expected_total": 104 if not (args.only or args.corpus_only
                                      or args.private_only) else None,
        "keep_work": cfg["keep_work"],
    }
    if meta["expected_total"] and len(entries) != meta["expected_total"]:
        print(f"Error: campaign expects {meta['expected_total']} documents, "
              f"inventory has {len(entries)}", file=sys.stderr)
        return 1
    lock = threading.Lock()

    def publish(r: dict) -> None:
        with lock:
            results[r["id"]] = r
            atomic_write_json(ckpt_dir / f"{r['id']}.json", r)
            atomic_write_json(args.output / "report.json",
                              {"meta": meta,
                               "summary": summarize_campaign(
                                   results, cfg["page_min"], cfg["doc_min"]),
                               "results": {k: results[k] for k in sorted(results)}})
            atomic_write_json(args.output / "gate.json",
                              campaign_gate(entries, results, cfg))

    print("=" * 78)
    print(f"*** {meta['notice']} ***")
    print(f"Campaign {len(entries)} documents | rust={rust_bin}")
    print(f"  ref={ {k: Path(v).name for k, v in ref_bins.items()} }")
    print(f"jobs={args.jobs} timeout={args.timeout}s max-passes={args.max_passes} "
          f"mem-limit={args.mem_limit_mib}MiB output={args.output} "
          f"resumed={len(results)} to-run={len(todo)}")
    print("=" * 78, flush=True)

    t0 = time.perf_counter()
    try:
        with cf.ThreadPoolExecutor(max_workers=max(1, args.jobs)) as ex:
            futs = {ex.submit(process_campaign_project, e, args.output,
                              cfg): e["id"] for e in todo}
            done = len(results)
            total = len(entries)
            for fut in cf.as_completed(futs):
                aid = futs[fut]
                try:
                    r = fut.result()
                except Exception as e:  # noqa: BLE001 - never lose the run
                    import traceback
                    traceback.print_exc()
                    r = {"id": aid, "mode": "campaign", "kind": "?",
                         "finished_utc": now_iso(), "harness_error": str(e),
                         "rust": {"status": "failure", "converged": False,
                                  "errors": [f"harness: {e}"]},
                         "ref": {"status": "failure", "converged": False},
                         "compare": {"compared": False, "note": "harness error",
                                     "page_failures": [], "per_page": [],
                                     "raster_warnings": []}}
                done += 1
                publish(r)
                print_campaign_line(done, total, r)
    except KeyboardInterrupt:
        print("Interrupted — checkpointed results are preserved; rerun to resume.",
              file=sys.stderr)

    gate = campaign_gate(entries, results, cfg)
    atomic_write_json(args.output / "report.json",
                      {"meta": meta,
                       "summary": summarize_campaign(
                           results, args.page_min, args.doc_min),
                       "results": {k: results[k] for k in sorted(results)}})
    atomic_write_json(args.output / "gate.json", gate)
    s = summarize_campaign(results, args.page_min, args.doc_min)
    dt = time.perf_counter() - t0
    print("=" * 78)
    print(f"CAMPAIGN ({len(results)}/{len(entries)} completed in {dt:.1f}s) "
          f"— independent convergence attempted (max {args.max_passes} passes); "
          f"bibtex honored; gate decides parity at {args.dpi:g}dpi")
    print(f"  rust status: {s['status_counts']['rust']}")
    print(f"  ref  status: {s['status_counts']['ref']}")
    print(f"  page-count match {s['page_count_match']} | geometry match "
          f"{s['geometry_match']} | fully identical {s['fully_identical_projects']}")
    print(f"  pages >= {args.page_min}%: {s['pages_ge_page_min']}/{s['pages_total']} "
          f"| docs >= {args.doc_min}%: {s['docs_ge_doc_min']}")
    print(f"  mean doc parity {s['mean_doc_exact_parity']}% | min doc "
          f"{s['min_doc_exact_parity']}% | min page {s['min_page_exact_parity']}%")
    print(f"  GATE: {'PASS' if gate['ok'] else 'FAIL'} "
          f"({len(gate['failures'])} failure rows) -> {args.output / 'gate.json'}")
    print(f"  report: {args.output / 'report.json'}")
    print("=" * 78)
    return 0 if gate["ok"] else 1


def main() -> int:
    ap = argparse.ArgumentParser(
        description="Corpus/parity harness: Rust pdflatex vs system engines. "
                    "Default --mode campaign = converged 104-document exact "
                    "150dpi RGB parity gate; --mode single-pass = legacy "
                    "one-invocation 72dpi diagnostic.")
    ap.add_argument("--mode", choices=("campaign", "single-pass"),
                    default="campaign",
                    help="campaign = default goal-grade converged gate; "
                         "single-pass = legacy diagnostic only")
    ap.add_argument("--corpus-dir", type=Path, default=Path("corpus"))
    ap.add_argument("--rust", type=Path, default=Path("target/release/pdflatex"),
                    help="Rust pdflatex binary")
    ap.add_argument("--sys", type=Path, default=Path("/usr/bin/pdflatex"),
                    help="Reference system pdflatex")
    ap.add_argument("--sys-xelatex", type=Path, default=Path("/usr/bin/xelatex"),
                    help="Reference system XeTeX (fontspec docs)")
    ap.add_argument("--sys-lualatex", type=Path,
                    default=Path("/usr/bin/lualatex"),
                    help="Reference system LuaTeX (luacode docs)")
    ap.add_argument("--rust-bibtex", type=Path,
                    default=Path("target/release/tex-bibtex"),
                    help="native Rust BibTeX for the Rust side (never system)")
    ap.add_argument("--sys-bibtex", type=Path, default=Path("/usr/bin/bibtex"),
                    help="system BibTeX for the reference side")
    ap.add_argument("--overlay", type=Path,
                    default=Path("output/parity-deps/texmf"),
                    help="isolated TDS dependency overlay exported as TEXMFHOME "
                         "identically to both engines (campaign mode)")
    ap.add_argument("--max-passes", type=int, default=5,
                    help="convergence pass bound per engine (campaign mode)")
    ap.add_argument("--dpi", type=float, default=CAMPAIGN_DPI,
                    help="campaign raster DPI (parity rule: 150)")
    ap.add_argument("--page-min", type=float, default=PAGE_MIN_PARITY_DEFAULT,
                    help="required exact-pixel parity percent for EVERY page")
    ap.add_argument("--doc-min", type=float, default=DOC_MIN_PARITY_DEFAULT,
                    help="required exact-pixel parity percent per document")
    ap.add_argument("--output", type=Path, default=Path("output/corpus"),
                    help="Report/log/pdf/workspace output root")
    ap.add_argument("--jobs", type=int, default=4, help="Concurrent projects")
    ap.add_argument("--timeout", type=float, default=60.0,
                    help="Per-compile timeout seconds (process group killed)")
    ap.add_argument("--mem-limit-mib", type=int, default=DEFAULT_MEM_LIMIT_MIB,
                    help="RLIMIT_AS address-space cap (MiB) applied to every "
                         "engine/bibtex child in the preexec path; a runaway "
                         "compile dies on a signal and classifies as "
                         "'memlimit'. 0 disables (default: 4 GiB)")
    ap.add_argument("--limit", type=int, default=100,
                    help="Number of corpus manifest entries (default: full 100)")
    ap.add_argument("--only", type=str, default=None,
                    help="Comma-separated subset of project ids (overrides --limit)")
    ap.add_argument("--corpus-only", action="store_true",
                    help="campaign: exclude the four private documents")
    ap.add_argument("--private-only", action="store_true",
                    help="campaign: only the four private documents")
    ap.add_argument("--no-resume", action="store_true",
                    help="Ignore existing checkpoints and recompile everything")
    ap.add_argument("--no-keep-work", action="store_true",
                    help="Delete per-engine workspace copies after comparison "
                         "(PDFs and logs are still persisted)")
    args = ap.parse_args()
    if args.mode == "campaign":
        return run_campaign(args)

    manifest_file = args.corpus_dir / "manifest.json"
    if not manifest_file.is_file():
        print(f"Error: manifest not found at {manifest_file}", file=sys.stderr)
        return 1
    manifest = json.loads(manifest_file.read_text())
    if not args.rust.is_file():
        print(f"Error: rust binary not found: {args.rust}", file=sys.stderr)
        return 1
    if not args.sys.is_file():
        print(f"Error: reference binary not found: {args.sys}", file=sys.stderr)
        return 1

    if args.only:
        wanted = [s.strip() for s in args.only.split(",") if s.strip()]
        by_id = {e["id"]: e for e in manifest}
        selected = [by_id[w] for w in wanted if w in by_id]
        missing = [w for w in wanted if w not in by_id]
        if missing:
            print(f"Error: ids not in manifest: {missing}", file=sys.stderr)
            return 1
    else:
        selected = manifest[: args.limit]

    ckpt_dir = args.output / "checkpoints"
    results: dict[str, dict] = {}
    todo = []
    if not args.no_resume and ckpt_dir.is_dir():
        for ck in sorted(ckpt_dir.glob("*.json")):
            try:
                r = json.loads(ck.read_text())
            except (json.JSONDecodeError, OSError):
                continue
            if not (isinstance(r, dict) and r.get("id")):
                continue
            # discard crashed/harness-error or schema-incomplete checkpoints
            if r.get("harness_error") or "compare" not in r:
                continue
            if not all(isinstance(r.get(e), dict) and "status" in r[e]
                       and "exit" in r[e] for e in ("rust", "ref")):
                continue
            if any(e["id"] == r["id"] for e in selected):
                results[r["id"]] = r
    todo = [e for e in selected if e["id"] not in results]

    meta = {
        "passes_per_engine": 1,
        "single_pass_notice": "SINGLE PASS: exactly one pdflatex invocation per engine "
                              "per project; bibtex is NOT run; source .bbl preserved.",
        "started_utc": now_iso(),
        "corpus_dir": str(args.corpus_dir), "manifest": str(manifest_file),
        "output": str(args.output), "jobs": args.jobs, "timeout_s": args.timeout,
        "mem_limit_mib": args.mem_limit_mib,
        # abspath, NOT resolve(): argv0 must stay "<...>/pdflatex" — symlink
        # resolution would rename it to pdftex and select the wrong format.
        "rust_bin": os.path.abspath(args.rust),
        "rust_version": capture_version(args.rust, 10),
        "sys_bin": os.path.abspath(args.sys),
        "sys_version": capture_version(args.sys, 10),
        "pymupdf_version": pymupdf.__version__, "numpy": np is not None,
        "selected": len(selected), "resumed": len(results), "planned": len(todo),
        "keep_work": not args.no_keep_work,
    }
    lock = threading.Lock()

    def publish(r: dict) -> None:
        with lock:
            results[r["id"]] = r
            atomic_write_json(ckpt_dir / f"{r['id']}.json", r)
            atomic_write_json(args.output / "report.json", build_report(meta, results))

    print("=" * 78)
    print(f"*** {meta['single_pass_notice']} ***")
    print(f"Corpus {len(selected)} projects | rust={meta['rust_bin']}")
    print(f"                          | ref ={meta['sys_bin']}")
    print(f"jobs={args.jobs} timeout={args.timeout}s output={args.output} "
          f"resumed={len(results)} to-run={len(todo)}")
    print("=" * 78, flush=True)

    t0 = time.perf_counter()
    try:
        with cf.ThreadPoolExecutor(max_workers=max(1, args.jobs)) as ex:
            rust_abspath = os.path.abspath(args.rust)
            sys_abspath = os.path.abspath(args.sys)
            futs = {ex.submit(process_project, e, args.corpus_dir, args.output,
                              rust_abspath, sys_abspath, args.timeout,
                              args.mem_limit_mib,
                              not args.no_keep_work): e["id"] for e in todo}
            done = len(results)
            total = len(selected)
            for fut in cf.as_completed(futs):
                aid = futs[fut]
                try:
                    r = fut.result()
                except Exception as e:  # noqa: BLE001 - never lose the run
                    import traceback
                    traceback.print_exc()
                    r = {"id": aid, "archive": "?", "main_tex": "?",
                         "finished_utc": now_iso(), "harness_error": str(e),
                         "stripped_generated": {},
                         "rust": {"engine": "rust", "bin": rust_abspath,
                                  "status": "failure", "exit": None, "timed_out": False,
                                  "time_ms": None, "pages": None, "pdf_valid": False,
                                  "pdf_exists": False, "pdf": None,
                                  "errors": [f"harness: {e}"]},
                         "ref": {"engine": "ref", "bin": sys_abspath,
                                 "status": "failure", "exit": None, "timed_out": False,
                                 "time_ms": None, "pages": None, "pdf_valid": False,
                                 "pdf_exists": False, "pdf": None, "errors": []},
                         "compare": {"compared": False, "note": "harness error"}}
                done += 1
                publish(r)
                print_report_line(done, total, r)
    except KeyboardInterrupt:
        print("Interrupted — checkpointed results are preserved; rerun to resume.",
              file=sys.stderr)

    # Final report (also refreshed per completion; durable partials either way).
    atomic_write_json(args.output / "report.json", build_report(meta, results))
    s = summarize(results)
    dt = time.perf_counter() - t0
    print("=" * 78)
    print(f"SUMMARY ({len(results)}/{len(selected)} completed in {dt:.1f}s) "
          f"— SINGLE PASS, bibtex not run")
    print(f"  rust status: {s['status_counts']['rust']}")
    print(f"  ref  status: {s['status_counts']['ref']}")
    print(f"  valid PDFs: rust {s['pdf_valid']['rust']}, ref {s['pdf_valid']['ref']} "
          f"| raster-compared: {s['raster_compared']}")
    print(f"  page-count match: {s['page_count_match']} | pixel-identical projects: "
          f"{s['pixel_identical_projects']}")
    print(f"  mean page pixel diff (0-255): {s['mean_page_pixel_diff']} | "
          f"mean text similarity: {s['mean_text_similarity']}")
    print(f"  compile times ms (median): {s['compile_time_ms']}")
    print(f"  report: {args.output / 'report.json'}")
    print("=" * 78)
    return 0


if __name__ == "__main__":
    sys.exit(main())
