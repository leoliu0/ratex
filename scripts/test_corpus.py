#!/usr/bin/env python3
"""Compare unchanged projects using the public Ratex and system latexmk drivers.

Campaign mode (default) gives each driver a fresh source copy, home and cache.
The drivers own pass convergence and bibliography processing; their actual
commands, exit status, logs and generated PDFs are retained in the report.
Source documents are never edited. Reference-only and Rust-only runs retain
every selected project but do not claim cross-engine output parity.

Full comparisons use exact 150-DPI RGB parity, page counts and page geometry.
The gate retains compilation, dependency, convergence and rendering failures.
The explicit legacy single-pass mode invokes engines once, without bibliography
processing, and is diagnostic only.

Examples:
  scripts/test_corpus.py --rust target/release/ratex --corpus-only
  scripts/test_corpus.py --engine ref --sys-latexmk /usr/bin/latexmk --only id1,id2
  scripts/test_corpus.py --retain all --keep-work --output output/corpus
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
import stat
import statistics
import subprocess
import sys
import threading
import time
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath

try:
    import resource  # RLIMIT_AS child guard (POSIX)
except ImportError:  # graceful degrade: no address-space cap
    resource = None

try:
    import numpy as np
except ImportError:  # graceful degrade: equality-only raster comparison
    np = None

try:
    import pymupdf  # installed PyMuPDF (fitz replacement)
except ImportError:  # graceful degrade
    pymupdf = None
try:
    from bounded_capture import (
        DEFAULT_MAX_CAPTURE_BYTES,
        capture_file,
        run_bounded,
    )
except ModuleNotFoundError:  # support `python -m scripts.test_corpus`
    from scripts.bounded_capture import (
        DEFAULT_MAX_CAPTURE_BYTES,
        capture_file,
        run_bounded,
    )

try:
    from test_fonts import validate_pdf_font_embedding
except ModuleNotFoundError:
    from scripts.test_fonts import validate_pdf_font_embedding

REFERENCE_SYSTEM_PATH = os.pathsep.join(
    path
    for path in (
        "/usr/local/sbin",
        "/usr/local/bin",
        "/usr/sbin",
        "/usr/bin",
        "/sbin",
        "/bin",
        "/usr/bin/site_perl",
        "/usr/bin/vendor_perl",
        "/usr/bin/core_perl",
    )
    if Path(path).is_dir()
)



def is_ratex_cli(bin_path: str | Path | None) -> bool:
    """Return True if bin_path is the Ratex driver executable (ratex or texmk)."""
    if not bin_path:
        return False
    stem = Path(bin_path).stem.lower()
    return stem in ("ratex", "texmk", "latexmk") or stem.startswith("ratex")

def validate_artifact_id(aid: object) -> str:
    """Return a path-safe project identifier or raise ``ValueError``."""
    if (
        not isinstance(aid, str)
        or not aid
        or aid in (".", "..")
        or "/" in aid
        or "\\" in aid
        or "\0" in aid
    ):
        raise ValueError(f"unsafe manifest id: {aid!r}")
    return aid


def validate_manifest_entry(entry: object) -> None:
    """Reject path-shaped identifiers before retention code constructs paths.

    Corpus manifests are local inputs, but their identifiers feed default
    workspace deletion. Treat them as untrusted so a malformed downloaded
    manifest cannot make cleanup escape the requested output tree.
    """
    if not isinstance(entry, dict):
        raise ValueError("manifest entry is not an object")
    aid = validate_artifact_id(entry.get("id"))
    main = entry.get("main_tex")
    if main is None and entry.get("blocker"):
        return
    if not isinstance(main, str) or not main or "\0" in main:
        raise ValueError(f"unsafe main_tex for {aid!r}: {main!r}")
    normalized = PurePosixPath(main.replace("\\", "/"))
    if normalized.is_absolute() or ".." in normalized.parts:
        raise ValueError(f"unsafe main_tex for {aid!r}: {main!r}")

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


def capture_version(bin_path: Path | str, timeout: float,
                    env: dict[str, str] | None = None) -> str:
    try:
        r = subprocess.run([str(bin_path), "--version"], stdin=subprocess.DEVNULL,
                           capture_output=True, text=True, timeout=timeout,
                           env={**os.environ, **env} if env is not None else None)
        return first_line((r.stdout or "") + (r.stderr or ""))
    except Exception as e:  # noqa: BLE001 - version string is best-effort
        return f"<error: {e}>"


# --------------------------------------------------------------------------
# Workspace preparation

def gen_artifact_names(stem: str) -> set[str]:
    names = {f"{stem}.{ext}" for ext in STRIP_EXTS}
    names |= {f"{stem}.{suffix}" for suffix in STRIP_NAMES_EXTRA}
    return names


def prepare_workspace(
    src: Path, dst: Path, tex_rel: Path, *, out_root: Path
) -> list[str]:
    """Fresh isolated copy of src into dst, stripping generated main-job
    artifacts by file name. Returns the stripped file names (audit trail)."""
    _safe_rmtree(dst, out_root, "work")
    _validate_cleanup_target(dst, out_root, "work")
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


def run_command(cmd: list[str], work_dir: Path, cap_path: Path,
                timeout: float, env: dict | None = None,
                mem_limit_mib: int = DEFAULT_MEM_LIMIT_MIB,
                max_capture_bytes: int = DEFAULT_MAX_CAPTURE_BYTES) -> dict:
    """Execute one child command with bounded head/tail capture and RLIMIT_AS guard."""
    limit_bytes = max(0, int(mem_limit_mib)) * (1 << 20)
    preexec = _mem_limit_preexec(limit_bytes) if limit_bytes and resource else None
    child = run_bounded(
        cmd,
        cwd=work_dir,
        output_path=cap_path,
        timeout=timeout,
        env=env,
        max_bytes=max_capture_bytes,
        error_pattern=ERROR_LINE_RE,
        preexec_fn=preexec,
    )
    timed_out = child["timed_out"]
    spawn_error = child["spawn_error"]
    rc = child["returncode"]
    dt_ms = child["elapsed_seconds"] * 1000.0
    kill_signal = None
    mem_killed = False
    if not timed_out and rc is not None and rc < 0:
        try:
            kill_signal = signal.Signals(-rc).name
        except ValueError:
            kill_signal = f"SIG{-rc}"
        mem_killed = limit_bytes > 0
    return {"exit": rc, "timed_out": timed_out, "time_ms": dt_ms,
            "spawn_error": spawn_error, "cmd": cmd,
            "mem_killed": mem_killed, "kill_signal": kill_signal,
            "capture": child["capture"]}


def run_compile(bin_path: str, work_dir: Path, tex_name: str, cap_path: Path,
                timeout: float, env: dict | None = None,
                flags: tuple[str, ...] | None = None,
                mem_limit_mib: int = DEFAULT_MEM_LIMIT_MIB,
                max_capture_bytes: int = DEFAULT_MAX_CAPTURE_BYTES) -> dict:
    """One engine pass. argv list (no shell), stdin DEVNULL, new process
    group, with merged output drained through a bounded head/tail capture."""
    if flags is None:
        flags = ("-interaction=nonstopmode", "-no-shell-escape")
    cmd = [bin_path, *flags, tex_name]
    return run_command(cmd, work_dir, cap_path, timeout, env=env,
                       mem_limit_mib=mem_limit_mib,
                       max_capture_bytes=max_capture_bytes)

def pdf_info(pdf: Path) -> dict:
    """Validate a PDF and report pages/bytes/sha. Never trust existence alone."""
    info = {"pdf": str(pdf), "pdf_exists": False, "pdf_valid": False,
            "pages": None, "pdf_bytes": 0, "pdf_sha1": None,
            "pdf_error": None}
    if not pdf.is_file():
        return info
    info["pdf_exists"] = True
    try:
        info["pdf_bytes"] = pdf.stat().st_size
    except OSError as exc:
        info["pdf_error"] = f"stat failed: {type(exc).__name__}: {exc}"
        return info

    # Hash every produced file, including empty or malformed remnants.  This
    # keeps failure reports useful without retaining the potentially huge file.
    try:
        h = hashlib.sha1()
        with open(pdf, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 16), b""):
                h.update(chunk)
        info["pdf_sha1"] = h.hexdigest()
    except OSError as exc:
        info["pdf_error"] = f"hash failed: {type(exc).__name__}: {exc}"
        return info

    if info["pdf_bytes"] == 0:
        info["pdf_error"] = "empty PDF"
        return info
    try:
        if pymupdf is not None:
            doc = pymupdf.open(str(pdf))
            try:
                info["pages"] = doc.page_count
                info["pdf_valid"] = doc.page_count >= 1
                if not info["pdf_valid"]:
                    info["pdf_error"] = "PDF contains no pages"
            finally:
                doc.close()
        elif shutil.which("pdfinfo"):
            proc = subprocess.run(
                ["pdfinfo", str(pdf)],
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
            )
            if proc.returncode == 0:
                m = re.search(r"^Pages:\s+(\d+)", proc.stdout, re.MULTILINE)
                pages = int(m.group(1)) if m else 0
                info["pages"] = pages
                info["pdf_valid"] = pages >= 1
            else:
                info["pdf_valid"] = False
                info["pdf_error"] = proc.stderr.strip() or "pdfinfo failed"
        else:
            with open(pdf, "rb") as f:
                header = f.read(1024)
            info["pdf_valid"] = b"%PDF-" in header and info["pdf_bytes"] > 500
            info["pages"] = 1
    except Exception as exc:  # corrupt / truncated output still counts as invalid
        info["pdf_valid"] = False
        detail = str(exc).strip().replace("\n", " ")
        if len(detail) > 500:
            detail = detail[:497] + "..."
        info["pdf_error"] = (
            f"{type(exc).__name__}: {detail}" if detail else type(exc).__name__
        )
    return info


def persist_valid_pdf(source: Path, destination: Path) -> dict:
    """Inspect an engine PDF and persist it only when it can be opened.

    A previous run may have left a destination behind, so remove that path
    before deciding whether this run produced evidence worth keeping.
    """
    info = pdf_info(source)
    destination.unlink(missing_ok=True)
    if info["pdf_valid"]:
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)
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
def classify_failure_kind(run: dict, pdf_stat: dict, errors: list[str],
                          compare: dict | None = None,
                          log_text: str = "",
                          font_errors: list[str] | None = None) -> str:
    """Classify failure kind distinctly:
    - unsupported-engine: required engine feature/engine unavailable (luacode, luatexja, ptex, fontspec on pdflatex, opentype math)
    - missing-asset: missing file/font/package (.sty, .cls, .ttf, .pfb, .tfm, etc.)
    - compilation: general syntax/macro error
    - extraction: PDF valid but text extraction/layer failed
    - embedding: PDF valid but font embedding/glyph inspection validation failed
    - render: geometry/page count mismatch or raster parity failure
    - timeout: process group timed out
    - memlimit: killed by RLIMIT_AS address cap
    - clean: successful compilation
    """
    if run.get("timed_out"):
        return "timeout"
    if run.get("mem_killed"):
        return "memlimit"
    # 0. Missing or invalid source prerequisite
    if any(
        "source directory missing" in e
        or "main_tex source file missing" in e
        or "main_tex missing" in e
        or "corpus dir missing" in e
        for e in errors
    ):
        return "invalid-source"


    combined_err = " ".join(errors) + " " + log_text

    # Missing asset check for outline program gaps:
    # "has no associated outline program" indicates a missing font outline asset (.pfb/.ttf/.otf),
    # never an unsupported engine capability or generic compilation failure.
    if (
        "has no associated outline program" in combined_err
        or "no associated outline program" in combined_err
    ):
        return "missing-asset"

    # 1. Unsupported engine
    if any(p in combined_err for p in (
        "requires LuaTeX", "requires either XeTeX or LuaTeX", "requires XeLaTeX",
        "cannot run in pdfTeX", "luacode", "luatexja.sty not found",
        "Package fontspec Error: The fontspec package requires",
        "Ratex does not implement OpenType MATH",
        "Use classic LaTeX math fonts, or compile with a full XeTeX or LuaTeX engine",
        "This package requires LuaTeX", "XeTeX is required",
        "LaTeX Error: This package requires LuaTeX",
    )):
        return "unsupported-engine"

    # 2. Missing asset
    if any(p in combined_err for p in (
        "not found", "File `", "File '", "cannot find font",
        "I can't find file", "Font \\", "checksum mismatch",
        "kpathsea: Running mktexmf",
        "Metric (TFM) file not found",
        "has no associated outline program",
        "no associated outline program",
    )):
        return "missing-asset"

    # 3. Invalid or missing PDF
    if not pdf_stat.get("pdf_valid"):
        return "compilation"

    # 4. Font embedding / glyph errors on valid PDF
    if font_errors or (compare and compare.get("font_embedding_failure")):
        return "embedding"

    # 5. Text extraction failure on valid PDF
    if compare and compare.get("text_extraction_failure"):
        return "extraction"

    # 6. If PDF exists, check compare for render/geometry/parity
    if compare:
        if compare.get("unsupported_engine"):
            return "unsupported-engine"
        if not compare.get("page_count_match") or not compare.get("geometry_match") or compare.get("page_failures"):
            return "render"

    if run.get("exit", 0) != 0 or errors:
        return "compilation"

    return "clean"


def compile_engine(engine: str, bin_path: str, ws: Path, tex_rel: Path,
                   idir: Path, timeout: float,
                   mem_limit_mib: int = DEFAULT_MEM_LIMIT_MIB,
                   max_capture_bytes: int = DEFAULT_MAX_CAPTURE_BYTES,
                   env: dict | None = None,
                   ref_engine: str = "pdflatex") -> dict:
    """Single pass + classification + artifact persistence for one engine."""
    work_dir = ws / tex_rel.parent
    cap_path = idir / f"{engine}.stdout.log"

    if engine == "rust" and is_ratex_cli(bin_path):
        flags = ("-1", "-interaction=nonstopmode", "-halt-on-error", "-no-shell-escape")
    elif engine == "rust":
        flags = ("-interaction=nonstopmode", "-halt-on-error", "-no-shell-escape")
    else:
        flags = REF_ENGINE_FLAGS.get(ref_engine, ("-interaction=nonstopmode", "-no-shell-escape"))

    run_env = dict(env if env is not None else os.environ)
    home_dir = ws / f".home_{engine}"
    cache_dir = ws / f".cache_{engine}"
    home_dir.mkdir(parents=True, exist_ok=True)
    cache_dir.mkdir(parents=True, exist_ok=True)
    run_env.setdefault("HOME", str(home_dir))
    run_env.setdefault("XDG_CACHE_HOME", str(cache_dir))
    if engine == "rust":
        run_env["TEX_RS_HERMETIC"] = "1"
        run_env.setdefault("TEX_RS_CACHE_DIR", str(cache_dir / "tex-rs"))
        for k in (
            "TEXMFHOME", "TEXMFVAR", "TEXMFCACHE", "TEXMFCONFIG",
            "TEXINPUTS", "BIBINPUTS", "BSTINPUTS", "TEXFORMATS",
            "LUAINPUTS", "TEXMFLOCAL", "TEXMFSYSVAR", "TEXMFSYSCONFIG",
            "TEXMF", "TEXMFCNF", "TEXMFDIST", "TEXMFMAIN",
            "TEXFONTMAPS", "ENCFONTS", "TFMFONTS", "T1FONTS", "VFFONTS",
            "TTFONTS", "OPENTYPEFONTS", "OSFONTDIR",
            "TEX_RS_TEXMF", "TEX_RS_FONT_DIR", "LUAOTFLOAD_TOOL_FORCE_CACHE",
        ):
            run_env.pop(k, None)

    run = run_compile(bin_path, work_dir, tex_rel.name, cap_path, timeout,
                      env=run_env, flags=flags,
                      mem_limit_mib=mem_limit_mib,
                      max_capture_bytes=max_capture_bytes)
    stem_pdf = work_dir / f"{tex_rel.stem}.pdf"
    stem_log = work_dir / f"{tex_rel.stem}.log"

    # Persist engine-written .log and the output PDF outside the workspace.
    kept_log = None
    tex_capture = None
    if stem_log.is_file():
        kept_log = idir / f"{engine}.tex.log"
        tex_capture = capture_file(
            stem_log, kept_log, max_bytes=max_capture_bytes,
            error_pattern=ERROR_LINE_RE,
        )
    kept_pdf = out_root_of(idir) / "pdf" / f"{pdf_basename(idir.name, engine)}"
    pdf_stat = persist_valid_pdf(stem_pdf, kept_pdf)

    # Error source: engine-written transcript preferred, captured output second.
    errors = ((tex_capture or {}).get("errors")
              or run["capture"].get("errors") or [])
    if not errors and run["spawn_error"]:
        errors = [f"spawn-error: {run['spawn_error']}"]

    font_embedding_errors: list[str] = []
    if pdf_stat["pdf_valid"] and kept_pdf.is_file():
        try:
            font_embedding_errors = validate_pdf_font_embedding(kept_pdf, {})
        except Exception as e:
            font_embedding_errors = [f"validator error: {e}"]

    status = classify(run, pdf_stat["pdf_valid"], errors)
    log_text = (tex_capture or {}).get("excerpt", "") if isinstance(tex_capture, dict) else ""
    failure_kind = classify_failure_kind(run, pdf_stat, errors, log_text=log_text, font_errors=font_embedding_errors)

    return {
        "engine": engine, "bin": bin_path, "status": status,
        "failure_kind": failure_kind,
        "exit": run["exit"], "timed_out": run["timed_out"],
        "mem_killed": run["mem_killed"], "kill_signal": run["kill_signal"],
        "time_ms": round(run["time_ms"], 1),
        "errors": errors,
        "font_embedding_errors": font_embedding_errors,
        "font_embedding_valid": len(font_embedding_errors) == 0,
        "capture": run["capture"], "tex_log_capture": tex_capture,
        **pdf_stat,
        "pdf": str(kept_pdf) if kept_pdf.is_file() else None,
        "pdf_produced": pdf_stat["pdf_exists"],
        "pdf_exists": kept_pdf.is_file(),
        "captured_log": str(cap_path), "tex_log": str(kept_log) if kept_log else None,
    }


def pdf_basename(aid: str, engine: str) -> str:
    return f"{aid}.{engine}.pdf"


def out_root_of(idir: Path) -> Path:
    """results/<id>/ -> output root (pdfs live at <output>/pdf/)."""
    return idir.parent.parent


def _project_failure_reasons(res: dict, cfg: dict) -> list[str]:
    reasons: list[str] = []
    if res.get("harness_error"):
        reasons.append("harness-error")
    campaign = res.get("mode") == "campaign"
    engine_filter = cfg.get("engine_filter", "both")
    for engine in ("rust", "ref"):
        if engine_filter != "both" and engine != engine_filter:
            continue
        state = res.get(engine) or {}
        if state.get("status") != "clean":
            reasons.append(f"{engine}-status:{state.get('status')}")
        if not state.get("pdf_valid"):
            reasons.append(f"{engine}-invalid-pdf")
        if campaign and not state.get("converged"):
            reasons.append(f"{engine}-not-converged")
        if state.get("font_embedding_errors"):
            reasons.append(f"{engine}-font-embedding")
    compare = res.get("compare") or {}
    if not compare.get("compared"):
        if engine_filter == "both":
            reasons.append("not-compared")
    elif campaign:
        if not compare.get("page_count_match"):
            reasons.append("page-count-mismatch")
        if not compare.get("geometry_match"):
            reasons.append("geometry-mismatch")
        if compare.get("raster_warnings"):
            reasons.append("raster-warning")
        prod_rust = compare.get("producer_rust")
        prod_ref = compare.get("producer_ref")
        if prod_rust != "tex-rs" and (not prod_ref or prod_rust != prod_ref):
            reasons.append("wrong-rust-producer")
        if compare.get("page_failures"):
            reasons.append("page-parity")
        parity = compare.get("document_exact_parity")
        if parity is None or parity < cfg["doc_min"]:
            reasons.append("document-parity")
        if (
            compare.get("font_embedding_failure")
            or (res.get("rust") or {}).get("font_embedding_errors")
            or (res.get("ref") or {}).get("font_embedding_errors")
        ):
            if "font-embedding" not in reasons:
                reasons.append("font-embedding")
        if compare.get("text_extraction_failure"):
            reasons.append("text-extraction")
    else:
        if not compare.get("page_count_match"):
            reasons.append("page-count-mismatch")
        if (compare.get("pages_identical") != compare.get("pages_compared")
                or compare.get("unmatched_pages")):
            reasons.append("pixel-mismatch")
        if (
            compare.get("font_embedding_failure")
            or (res.get("rust") or {}).get("font_embedding_errors")
            or (res.get("ref") or {}).get("font_embedding_errors")
        ):
            if "font-embedding" not in reasons:
                reasons.append("font-embedding")
        if compare.get("text_extraction_failure"):
            reasons.append("text-extraction")
    return reasons


def _path_within(path: Path, root: Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
        return True
    except (OSError, ValueError):
        return False


_MANAGED_ARTIFACT_ROOTS = ("results", "pdf", "worst", "work")


def _lexical_absolute(path: Path) -> Path:
    """Make a path absolute without following any filesystem links."""
    return Path(os.path.abspath(os.fspath(path)))


def _entry_identity(info: os.stat_result) -> tuple[int, int, int]:
    return (info.st_dev, info.st_ino, stat.S_IFMT(info.st_mode))


def _validated_managed_root(
    out_root: Path, name: str
) -> tuple[Path, Path, Path, os.stat_result | None]:
    """Return lexical/resolved output roots and a checked managed child root."""
    if name not in _MANAGED_ARTIFACT_ROOTS:
        raise ValueError(f"unknown managed artifact root: {name!r}")
    output = _lexical_absolute(out_root)
    try:
        output_info = output.lstat()
    except FileNotFoundError as exc:
        raise ValueError(f"artifact output root does not exist: {output}") from exc
    if stat.S_ISLNK(output_info.st_mode) or not stat.S_ISDIR(output_info.st_mode):
        raise ValueError(f"artifact output root is not a plain directory: {output}")
    resolved_output = output.resolve(strict=True)

    managed = output / name
    try:
        managed_info = managed.lstat()
    except FileNotFoundError:
        return output, resolved_output, managed, None
    if stat.S_ISLNK(managed_info.st_mode) or not stat.S_ISDIR(managed_info.st_mode):
        raise ValueError(f"managed artifact root is not a plain directory: {managed}")
    resolved_managed = managed.resolve(strict=True)
    try:
        relative = resolved_managed.relative_to(resolved_output)
    except ValueError as exc:
        raise ValueError(f"managed artifact root escapes output root: {managed}") from exc
    if not relative.parts:
        raise ValueError(f"managed artifact root aliases output root: {managed}")
    return output, resolved_output, managed, managed_info


def _validate_managed_roots(out_root: Path) -> None:
    """Reject unsafe managed roots before retention changes any artifact."""
    for name in _MANAGED_ARTIFACT_ROOTS:
        _validated_managed_root(out_root, name)


def _validate_cleanup_target(
    target: Path, out_root: Path, managed_name: str
) -> tuple[Path, tuple[int, int, int], tuple[int, int, int]] | None:
    """Validate one recursive-cleanup target and every existing parent."""
    _output, resolved_output, managed, managed_info = _validated_managed_root(
        out_root, managed_name
    )
    target = _lexical_absolute(target)
    try:
        relative = target.relative_to(managed)
    except ValueError as exc:
        raise ValueError(f"cleanup target is outside managed root: {target}") from exc
    if not relative.parts:
        raise ValueError(f"refusing to recursively remove managed root: {managed}")
    if managed_info is None:
        try:
            target.lstat()
        except FileNotFoundError:
            return None
        raise ValueError(f"cleanup target exists without its managed root: {target}")

    current = managed
    for part in relative.parts[:-1]:
        current /= part
        try:
            info = current.lstat()
        except FileNotFoundError:
            return None
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            raise ValueError(f"cleanup parent is not a plain directory: {current}")
        try:
            current.resolve(strict=True).relative_to(resolved_output)
        except ValueError as exc:
            raise ValueError(f"cleanup parent escapes output root: {current}") from exc

    try:
        target_info = target.lstat()
    except FileNotFoundError:
        return None
    if stat.S_ISLNK(target_info.st_mode) or not stat.S_ISDIR(target_info.st_mode):
        raise ValueError(f"cleanup target is not a plain directory: {target}")
    try:
        target.resolve(strict=True).relative_to(managed.resolve(strict=True))
    except ValueError as exc:
        raise ValueError(f"cleanup target escapes managed root: {target}") from exc
    parent_info = target.parent.lstat()
    if stat.S_ISLNK(parent_info.st_mode) or not stat.S_ISDIR(parent_info.st_mode):
        raise ValueError(f"cleanup parent is not a plain directory: {target.parent}")
    return target, _entry_identity(parent_info), _entry_identity(target_info)


def _safe_rmtree(target: Path, out_root: Path, managed_name: str) -> None:
    """Recursively remove only a stable, contained, plain directory target."""
    checked = _validate_cleanup_target(target, out_root, managed_name)
    if checked is None:
        return
    target, parent_identity, target_identity = checked
    # Re-check both entries immediately before handing the path to rmtree.
    # shutil.rmtree uses descriptor-based traversal where the platform supports
    # it, while these identity checks protect its parent lookup boundary.
    checked_again = _validate_cleanup_target(target, out_root, managed_name)
    if checked_again is None or checked_again[1:] != (parent_identity, target_identity):
        raise ValueError(f"cleanup target changed during validation: {target}")
    shutil.rmtree(target, ignore_errors=True)


def _resolve_evidence_path(value: str | None, out_root: Path) -> Path | None:
    if not value:
        return None
    raw = Path(value)
    candidates = [raw]
    for index, part in enumerate(raw.parts):
        if part in ("results", "pdf", "worst", "work"):
            candidates.append(out_root / Path(*raw.parts[index:]))
            break
    for path in candidates:
        if _path_within(path, out_root) and (path.exists() or path.is_symlink()):
            return path
    return None


def _unlink_evidence(path_value: str | None, out_root: Path) -> None:
    path = _resolve_evidence_path(path_value, out_root)
    if path is not None and (path.is_file() or path.is_symlink()):
        path.unlink(missing_ok=True)


def _compact_failure_logs(res: dict, out_root: Path) -> None:
    """Keep stable final logs and remove per-pass duplicates."""
    idir = out_root / "results" / res["id"]
    idir.mkdir(parents=True, exist_ok=True)
    for engine in ("rust", "ref"):
        state = res.get(engine) or {}
        bib_runs = state.get("bibtex_runs") or []
        if bib_runs:
            source_value = bib_runs[-1].get("log")
            source = _resolve_evidence_path(source_value, out_root)
            dest = idir / f"{engine}.bibtex.log"
            if (source and source.is_file()
                    and source.resolve() != dest.resolve()):
                shutil.copyfile(source, dest)
            for bib in bib_runs:
                bib["log"] = None
            bib_runs[-1]["log"] = str(dest) if dest.is_file() else None
        for record in state.get("pass_records") or []:
            record["stdout_log"] = None
            record["tex_log"] = None
        state["pass_log_dir"] = None
    _safe_rmtree(idir / "passlogs", out_root, "results")


def _failure_bundle(res: dict, reasons: list[str]) -> dict:
    engines = {}
    for engine in ("rust", "ref"):
        state = res.get(engine) or {}
        engines[engine] = {
            key: state.get(key) for key in (
                "bin", "ref_engine", "status", "failure_kind", "exit", "timed_out",
                "mem_killed", "kill_signal", "time_ms", "passes",
                "converged", "errors", "aux_hashes", "capture",
                "tex_log_capture", "captured_log", "tex_log", "pdf",
                "pdf_produced", "pdf_exists", "pdf_valid", "pdf_bytes",
                "pdf_sha1", "pdf_error", "pages",
                "font_embedding_errors", "font_embedding_valid",
            ) if key in state
        }
        records = state.get("pass_records") or []
        if records:
            engines[engine]["last_command"] = records[-1].get("cmd")
    compare = res.get("compare") or {}
    return {
        "schema": "tex-corpus-failure-v1",
        "id": res.get("id"),
        "mode": res.get("mode", "single-pass"),
        "main_tex": res.get("main_tex"),
        "manifest_index": res.get("manifest_index"),
        "finished_utc": res.get("finished_utc"),
        "reasons": reasons,
        "harness_error": res.get("harness_error"),
        "engines": engines,
        "comparison": {
            key: compare.get(key) for key in (
                "compared", "note", "page_count_match", "geometry_match",
                "document_exact_parity", "worst_page", "worst_page_parity",
                "page_failures", "raster_warnings", "worst_artifacts",
                "font_embedding_failure", "font_embedding_errors",
                "text_extraction_failure", "text_extraction_error",
                "mean_text_similarity",
            ) if key in compare
        },
    }


def apply_artifact_retention(res: dict, out_root: Path, cfg: dict) -> None:
    """Apply the post-metrics policy without deleting checkpoint data."""
    # This function also reconciles persisted checkpoints.  Validate their ID
    # again at the mutation boundary so a hand-edited or legacy checkpoint can
    # never turn its results/work cleanup paths into path traversal.
    validate_artifact_id(res.get("id"))
    _validate_managed_roots(out_root)
    retain = cfg["retain"]
    reasons = _project_failure_reasons(res, cfg)
    keep_evidence = retain == "all" or (retain == "failures" and bool(reasons))
    idir = out_root / "results" / res["id"]
    # Validate all possible recursive targets together, before an evidence
    # unlink, compaction, or metadata rewrite can partially apply the policy.
    _validate_cleanup_target(idir, out_root, "results")
    _validate_cleanup_target(idir / "passlogs", out_root, "results")
    _validate_cleanup_target(out_root / "work" / res["id"], out_root, "work")

    # Old checkpoints could reference corrupt engine remnants because earlier
    # harness versions copied every nonempty .pdf.  Metadata remains in the
    # report, but invalid files are never evidence under any retention policy.
    for engine in ("rust", "ref"):
        state = res.get(engine) or {}
        if not state.get("pdf_valid"):
            _unlink_evidence(state.get("pdf"), out_root)
            state["pdf"] = None
            state["pdf_retained"] = False
            state["pdf_exists"] = False

    if keep_evidence and reasons and retain == "failures":
        _compact_failure_logs(res, out_root)
        for engine in ("rust", "ref"):
            state = res.get(engine) or {}
            state["pdf_retained"] = bool(
                _resolve_evidence_path(state.get("pdf"), out_root)
            )
        atomic_write_json(idir / "failure.json", _failure_bundle(res, reasons))
    elif not keep_evidence:
        for engine in ("rust", "ref"):
            state = res.get(engine) or {}
            _unlink_evidence(state.get("pdf"), out_root)
            state["pdf"] = None
            state["pdf_retained"] = False
            state["pdf_exists"] = False
            state["captured_log"] = None
            state["tex_log"] = None
            state["pass_log_dir"] = None
            for record in state.get("pass_records") or []:
                record["stdout_log"] = None
                record["tex_log"] = None
            for bib in state.get("bibtex_runs") or []:
                bib["log"] = None
        artifacts = (res.get("compare") or {}).get("worst_artifacts")
        if isinstance(artifacts, dict):
            for value in artifacts.values():
                _unlink_evidence(value, out_root)
        if "compare" in res:
            res["compare"]["worst_artifacts"] = None
            res["compare"]["artifacts_retained"] = False
        _safe_rmtree(idir, out_root, "results")
    else:
        for engine in ("rust", "ref"):
            state = res.get(engine) or {}
            state["pdf_retained"] = bool(
                _resolve_evidence_path(state.get("pdf"), out_root)
            )

    if not cfg["keep_work"]:
        _safe_rmtree(out_root / "work" / res["id"], out_root, "work")
    for directory in (out_root / "results", out_root / "pdf",
                      out_root / "worst", out_root / "work"):
        try:
            directory.rmdir()
        except OSError:
            pass
    res["retention"] = {
        "policy": retain,
        "failure": bool(reasons),
        "failure_reasons": reasons,
        "evidence_retained": keep_evidence,
        "workspace_retained": bool(cfg["keep_work"]),
    }


# --------------------------------------------------------------------------
# Campaign mode: 104-document converged exact-parity gate

CAMPAIGN_DPI = 150.0
PAGE_MIN_PARITY_DEFAULT = 99.0   # every page must clear this exact-pixel floor
DOC_MIN_PARITY_DEFAULT = 99.0    # document aggregate exact-pixel floor
AUX_EXTS = ("aux", "out", "toc", "nav", "snm", "bbl", "lof", "lot", "brf")
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
    "xelatex": ("-interaction=nonstopmode", "-no-shell-escape"),
    "lualatex": ("-interaction=nonstopmode", "-no-shell-escape"),
}
FONTSPEC_RE = re.compile(
    r"\\(?:usepackage|RequirePackage)(?:\[[^\]]*\])?\{[^}]*\b(?:fontspec|xeCJK|xunicode|unicode-math|bxjsarticle|bxjsbook|zxjatype|xecyr|xltxtra|polyglossia|bidi)\b[^}]*\}",
    re.DOTALL,
)
LUALATEX_RE = re.compile(
    r"\\(?:usepackage|RequirePackage)(?:\[[^\]]*\])?\{[^}]*\b(?:luacode|luatextra|luatexja|luamplib|lualibs|luaotfload|luatexbase)\b[^}]*\}",
    re.DOTALL,
)


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def campaign_environments(
    overlay: Path | None,
    rust_home: Path | None = None,
    rust_cache: Path | None = None,
    ref_home: Path | None = None,
    ref_cache: Path | None = None,
) -> tuple[dict, dict, list[str], dict]:
    """Produce separate, isolated child environments for Rust and Reference subprocesses.

    Rust runs in a strictly hermetic environment (TEX_RS_HERMETIC=1) with no host
    TEXMF variables or external font paths, and dedicated fresh HOME and cache.
    Reference receives host TeX Live environment plus the isolated TDS dependency overlay
    exported via TEXMFHOME and TEXMFVAR, and its own dedicated fresh HOME and cache.
    """
    base_env = dict(os.environ)
    removed = [k for k in DIAGNOSTIC_VARS if base_env.pop(k, None) is not None]

    # Rust environment: strictly hermetic, no host TEXMF or search paths
    rust_env = dict(base_env)
    rust_env["TEX_RS_HERMETIC"] = "1"
    for k in (
        "TEXMFHOME", "TEXMFVAR", "TEXMFCACHE", "TEXMFCONFIG",
        "TEXINPUTS", "BIBINPUTS", "BSTINPUTS", "TEXFORMATS",
        "LUAINPUTS", "TEXMFLOCAL", "TEXMFSYSVAR", "TEXMFSYSCONFIG",
        "TEXMF", "TEXMFCNF", "TEXMFDIST", "TEXMFMAIN",
        "TEXFONTMAPS", "ENCFONTS", "TFMFONTS", "T1FONTS", "VFFONTS",
        "TTFONTS", "OPENTYPEFONTS", "OSFONTDIR",
        "TEX_RS_TEXMF", "TEX_RS_FONT_DIR", "LUAOTFLOAD_TOOL_FORCE_CACHE",
    ):
        rust_env.pop(k, None)
    rust_env["LANG"] = "C.UTF-8"
    rust_env["LC_ALL"] = "C.UTF-8"
    if rust_home is not None:
        rust_env["HOME"] = str(rust_home)
    if rust_cache is not None:
        rust_env["XDG_CACHE_HOME"] = str(rust_cache)
        rust_env["TEX_RS_CACHE_DIR"] = str(rust_cache / "tex-rs")

    # Reference environment: uses host TeX Live + overlay with fresh HOME
    ref_env = dict(base_env)
    ref_env.pop("TEX_RS_HERMETIC", None)
    ref_env.pop("TEX_RS_CACHE_DIR", None)
    ref_env.pop("TEX_RS_TEXMF", None)
    ref_env.pop("TEX_RS_FONT_DIR", None)
    for k in ("TEXMFCACHE", "TEXMFCONFIG"):
        ref_env.pop(k, None)
    if ref_home is not None:
        ref_env["HOME"] = str(ref_home)
    if ref_cache is not None:
        ref_env["XDG_CACHE_HOME"] = str(ref_cache)

    overlay_vars: dict = {}
    if overlay is not None:
        ov = Path(overlay).resolve()
        var = ov.with_name("texmf-var")
        if not var.is_dir():
            print(f"CAMPAIGN: WARNING: overlay TEXMFVAR tree missing: {var}; "
                  "generated font maps (nanumfonts.map/umj.map) will not "
                  "resolve — failures retained", file=sys.stderr)
        existing = ref_env.get("TEXMFHOME", "").strip(":")
        ref_env["TEXMFHOME"] = f"{existing}:{ov}" if existing else str(ov)
        ref_env["TEXMFVAR"] = str(var)
        overlay_vars = {"TEXMFHOME": ref_env["TEXMFHOME"],
                        "TEXMFVAR": ref_env["TEXMFVAR"],
                        "overlay": str(ov),
                        "texmf_var_exists": var.is_dir()}

    return rust_env, ref_env, removed, overlay_vars

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


def freeze_workspace(
    src: Path,
    dst: Path,
    tex_rel: Path,
    extra_ignore: tuple[str, ...] = (),
    *,
    out_root: Path,
) -> list[str]:
    """Fresh isolated copy of the ACTIVE source with generated main-job
    artifacts stripped (source .bbl preserved). Returns stripped names."""
    _safe_rmtree(dst, out_root, "work")
    _validate_cleanup_target(dst, out_root, "work")
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


def detect_ref_engine(src_dir: Path, tex_rel: Path, entry: dict | None = None) -> str:
    """Choose the genuine reference engine (pdflatex, xelatex, or lualatex)
    based on document source requirements, magic comments, project includes,
    and declared metadata. No fontspec.sty shim assumption is made.
    """
    if entry:
        eng = (entry.get("ref_engine") or entry.get("engine") or "").lower()
        if "lualatex" in eng or "luatex" in eng:
            return "lualatex"
        if "xelatex" in eng or "xetex" in eng:
            return "xelatex"
        if "pdflatex" in eng:
            return "pdflatex"

    # Check project latexmkrc / .latexmkrc
    for rc_name in ("latexmkrc", ".latexmkrc"):
        rc_path = src_dir / rc_name
        if rc_path.is_file():
            try:
                rc_text = rc_path.read_text(errors="replace")
                if re.search(r"\$pdf_mode\s*=\s*4", rc_text) or re.search(r"\$pdflatex\s*=.*(?:xelatex|xetex)", rc_text, re.IGNORECASE):
                    return "xelatex"
                if re.search(r"\$pdf_mode\s*=\s*5", rc_text) or re.search(r"\$pdflatex\s*=.*(?:lualatex|luatex)", rc_text, re.IGNORECASE):
                    return "lualatex"
                if re.search(r"\$pdf_mode\s*=\s*1", rc_text):
                    return "pdflatex"
            except OSError:
                pass

    main_tex = src_dir / tex_rel
    raw_main = _read_text(main_tex)
    if not raw_main:
        return "pdflatex"

    # Magic comments in main_tex (first 30 lines)
    for line in raw_main.splitlines()[:30]:
        m = re.search(r"%\s*!(?:TEX|TeX|tex)\s+(?:TS-)?program\s*=\s*([a-zA-Z0-9_\-]+)", line, re.IGNORECASE)
        if m:
            prog = m.group(1).lower()
            if "lualatex" in prog or "luatex" in prog:
                return "lualatex"
            if "xelatex" in prog or "xetex" in prog:
                return "xelatex"
            if "pdflatex" in prog:
                return "pdflatex"

    def _strip_comments(text: str) -> str:
        lines = []
        for line in text.splitlines():
            lines.append(re.sub(r"(?<!\\)%.*$", "", line))
        return "\n".join(lines)

    # Transitively collect main_tex and included files / packages in src_dir
    visited: set[Path] = set()
    to_visit: list[Path] = [main_tex]
    collected: list[str] = []
    while to_visit:
        curr = to_visit.pop()
        if curr in visited or not curr.is_file():
            continue
        visited.add(curr)
        text = _read_text(curr)
        stripped = _strip_comments(text)
        collected.append(stripped)

        # \input{...}, \include{...}, \subfile{...}
        for inc in re.findall(r"\\(?:input|include|subfile)\{([^}]+)\}", stripped):
            inc = inc.strip()
            for candidate in (src_dir / inc, src_dir / f"{inc}.tex"):
                if candidate.is_file() and candidate not in visited:
                    to_visit.append(candidate)

        # \input filename (unbraced)
        for inc in re.findall(r"\\input\s+([^\s%{}]+)", stripped):
            inc = inc.strip()
            for candidate in (src_dir / inc, src_dir / f"{inc}.tex"):
                if candidate.is_file() and candidate not in visited:
                    to_visit.append(candidate)

        # \documentclass{...}, \LoadClass{...}, \LoadClassWithOptions{...} -> local .cls
        for cls_match in re.findall(r"\\(?:documentclass|LoadClass|LoadClassWithOptions)(?:\[[^\]]*\])?\{([^}]+)\}", stripped):
            cls_name = cls_match.strip()
            cls_path = src_dir / f"{cls_name}.cls"
            if cls_path.is_file() and cls_path not in visited:
                to_visit.append(cls_path)

        # \usepackage{...} or \RequirePackage{...} -> local .sty
        for pkg_match in re.findall(r"\\(?:usepackage|RequirePackage)(?:\[[^\]]*\])?\{([^}]+)\}", stripped, re.DOTALL):
            for pkg in pkg_match.split(","):
                pkg_name = pkg.strip()
                sty_path = src_dir / f"{pkg_name}.sty"
                if sty_path.is_file() and sty_path not in visited:
                    to_visit.append(sty_path)

    full_text = "\n".join(collected)

    # 1. LuaTeX specific packages
    if LUALATEX_RE.search(full_text):
        return "lualatex"

    # 2. XeTeX / fontspec / Unicode packages
    if FONTSPEC_RE.search(full_text):
        return "xelatex"
    if re.search(r"\\(?:documentclass|LoadClass|LoadClassWithOptions)(?:\[[^\]]*\])?\{[^}]*\b(?:bxjsarticle|bxjsbook)\b[^}]*\}", full_text):
        return "xelatex"

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


def compile_campaign_ref(
    ws_ref: Path,
    tex_rel: Path,
    idir: Path,
    env: dict,
    timeout: float,
    cfg: dict,
    ref_engine: str,
    mem_limit_mib: int = DEFAULT_MEM_LIMIT_MIB,
    max_capture_bytes: int = DEFAULT_MAX_CAPTURE_BYTES,
) -> dict:
    """Run real system latexmk reference inside a safe isolated environment."""
    work_dir = ws_ref / tex_rel.parent
    job = tex_rel.stem
    bbl_is_source = (work_dir / f"{job}.bbl").is_file()
    cap_path = idir / "ref.stdout.log"
    latexmk_bin = cfg.get("sys_latexmk") or shutil.which("latexmk") or "/usr/bin/latexmk"
    ref_bin = cfg.get("ref_bins", {}).get(ref_engine) or f"/usr/bin/{ref_engine}"

    if ref_engine == "xelatex":
        engine_flags = ["-xelatex", f"-pdfxelatex={ref_bin} -interaction=nonstopmode %O %S"]
    elif ref_engine == "lualatex":
        engine_flags = ["-lualatex", f"-pdflualatex={ref_bin} -interaction=nonstopmode %O %S"]
    else:
        engine_flags = ["-pdf", f"-pdflatex={ref_bin} -interaction=nonstopmode %O %S"]

    latexmk_cmd = [
        str(latexmk_bin),
        *engine_flags,
        "-interaction=nonstopmode",
        tex_rel.name,
    ]

    ref_home = ws_ref / ".home"
    ref_cache = ws_ref / ".cache"
    ref_home.mkdir(parents=True, exist_ok=True)
    ref_cache.mkdir(parents=True, exist_ok=True)

    bwrap_path = shutil.which("bwrap") if not cfg.get("no_sandbox") else None
    use_bwrap = bwrap_path is not None and Path(bwrap_path).is_file()

    overlay_path = cfg.get("overlay_path")

    if use_bwrap:
        bwrap_cmd = [
            bwrap_path,
            "--ro-bind", "/usr", "/usr",
            "--ro-bind", "/lib", "/lib",
        ]
        if Path("/lib64").is_dir():
            bwrap_cmd += ["--ro-bind", "/lib64", "/lib64"]
        bwrap_cmd += [
            "--ro-bind", "/etc", "/etc",
            "--ro-bind", "/var", "/var",
            "--ro-bind", "/bin", "/bin",
            "--dev", "/dev",
            "--proc", "/proc",
            "--tmpfs", "/tmp",
            "--unshare-net",
            "--bind", str(ws_ref), str(ws_ref),
            "--bind", str(ref_home), str(ref_home),
            "--bind", str(ref_cache), str(ref_cache),
        ]
        if overlay_path and Path(overlay_path).is_dir():
            bwrap_cmd += ["--ro-bind", str(overlay_path), str(overlay_path)]
        bwrap_cmd += [
            "--setenv", "PATH", REFERENCE_SYSTEM_PATH,
            "--setenv", "HOME", str(ref_home),
            "--setenv", "XDG_CACHE_HOME", str(ref_cache),
        ]
        if overlay_path and Path(overlay_path).is_dir():
            bwrap_cmd += ["--setenv", "TEXMFHOME", str(overlay_path)]
        full_cmd = bwrap_cmd + latexmk_cmd
        run_env = dict(env if env is not None else os.environ)
    else:
        latexmk_cmd.insert(1, "-norc")
        full_cmd = latexmk_cmd
        run_env = dict(env if env is not None else os.environ)
        run_env["PATH"] = "/usr/bin:/bin:" + run_env.get("PATH", "")
        run_env["HOME"] = str(ref_home)
        run_env["XDG_CACHE_HOME"] = str(ref_cache)
        if overlay_path and Path(overlay_path).is_dir():
            run_env["TEXMFHOME"] = str(overlay_path)

    run = run_command(
        full_cmd,
        work_dir=work_dir,
        cap_path=cap_path,
        timeout=timeout,
        env=run_env,
        mem_limit_mib=mem_limit_mib,
        max_capture_bytes=max_capture_bytes,
    )

    stem_pdf = work_dir / f"{job}.pdf"
    stem_log = work_dir / f"{job}.log"
    kept_pdf = out_root_of(idir) / "pdf" / f"{pdf_basename(idir.name, 'ref')}"
    pdf_stat = persist_valid_pdf(stem_pdf, kept_pdf)

    kept_log = None
    tex_capture = None
    if stem_log.is_file():
        kept_log = idir / "ref.tex.log"
        tex_capture = capture_file(
            stem_log, kept_log, max_bytes=max_capture_bytes,
            error_pattern=ERROR_LINE_RE,
        )

    stdout_text = _read_text(cap_path)
    log_text = _read_text(kept_log) if kept_log else ""

    errors = ((tex_capture or {}).get("errors")
              or run["capture"].get("errors") or [])
    if not errors and run["spawn_error"]:
        errors = [f"spawn-error: {run['spawn_error']}"]

    font_embedding_errors: list[str] = []
    if pdf_stat["pdf_valid"] and kept_pdf.is_file():
        try:
            font_embedding_errors = validate_pdf_font_embedding(kept_pdf, {})
        except Exception as e:
            font_embedding_errors = [f"validator error: {e}"]

    rule_runs = re.findall(r"Run number (\d+) of rule '([^']+)'", stdout_text)
    passes = max((int(number) for number, rule in rule_runs
                  if rule == ref_engine), default=0)
    bib_runs = [{"run": int(number), "rule": rule}
                for number, rule in rule_runs
                if rule.split()[0] in ("bibtex", "biber")]

    converged = bool(run["exit"] == 0 and pdf_stat["pdf_valid"]
                     and not run["timed_out"] and not run["mem_killed"])
    status = classify(run, pdf_stat["pdf_valid"], errors)
    if status == "clean" and not converged:
        status = "unconverged"

    failure_kind = classify_failure_kind(
        run,
        pdf_stat,
        errors,
        log_text=log_text,
        font_errors=font_embedding_errors,
    )

    rec = {
        "engine": "ref",
        "driver": "latexmk",
        "bin": str(latexmk_bin),
        "ref_engine": ref_engine,
        "ref_bin": ref_bin,
        "status": status,
        "failure_kind": failure_kind,
        "exit": run["exit"],
        "timed_out": run["timed_out"],
        "mem_killed": run["mem_killed"],
        "kill_signal": run["kill_signal"],
        "time_ms": round(run["time_ms"], 1),
        "passes": passes,
        "converged": converged,
        "pass_records": [{"pass": 1, "cmd": run["cmd"], "time_ms": round(run["time_ms"], 1), "exit": run["exit"]}],
        "bibtex_runs": bib_runs,
        "shipped_bbl": bbl_is_source,
        "errors": errors,
        "aux_hashes": aux_state(work_dir, job),
        "font_embedding_errors": font_embedding_errors,
        "font_embedding_valid": len(font_embedding_errors) == 0,
        "capture": run["capture"],
        "tex_log_capture": tex_capture,
        "captured_log": str(cap_path) if cap_path.is_file() else None,
        "tex_log": str(kept_log) if kept_log and kept_log.is_file() else None,
        "pass_log_dir": None,
        "sandboxed": use_bwrap,
        "cmd": run["cmd"],
    }
    rec.update(pdf_stat)
    rec["pdf"] = str(kept_pdf) if kept_pdf.is_file() else None
    rec["pdf_produced"] = pdf_stat["pdf_exists"]
    rec["pdf_exists"] = kept_pdf.is_file()
    return rec


def compile_campaign_rust(
    ws_rust: Path,
    tex_rel: Path,
    idir: Path,
    env: dict,
    timeout: float,
    cfg: dict,
    ref_engine: str,
    mem_limit_mib: int = DEFAULT_MEM_LIMIT_MIB,
    max_capture_bytes: int = DEFAULT_MAX_CAPTURE_BYTES,
) -> dict:
    """Run public Ratex driver (not Ratex -1) with full convergence and EPS preprocessing."""
    work_dir = ws_rust / tex_rel.parent
    job = tex_rel.stem
    bbl_is_source = (work_dir / f"{job}.bbl").is_file()
    cap_path = idir / "rust.stdout.log"
    rust_bin = cfg["rust_bin"]

    rust_home = ws_rust / ".home"
    rust_cache = ws_rust / ".cache"
    rust_home.mkdir(parents=True, exist_ok=True)
    rust_cache.mkdir(parents=True, exist_ok=True)

    run_env = dict(env if env is not None else os.environ)
    run_env["TEX_RS_HERMETIC"] = "1"
    run_env["HOME"] = str(rust_home)
    run_env["XDG_CACHE_HOME"] = str(rust_cache)
    run_env["TEX_RS_CACHE_DIR"] = str(rust_cache / "tex-rs")
    for k in (
        "TEXMFHOME", "TEXMFVAR", "TEXMFCACHE", "TEXMFCONFIG",
        "TEXINPUTS", "BIBINPUTS", "BSTINPUTS", "TEXFORMATS",
        "LUAINPUTS", "TEXMFLOCAL", "TEXMFSYSVAR", "TEXMFSYSCONFIG",
        "TEXMF", "TEXMFCNF", "TEXMFDIST", "TEXMFMAIN",
        "TEXFONTMAPS", "ENCFONTS", "TFMFONTS", "T1FONTS", "VFFONTS",
        "TTFONTS", "OPENTYPEFONTS", "OSFONTDIR",
        "TEX_RS_TEXMF", "TEX_RS_FONT_DIR", "LUAOTFLOAD_TOOL_FORCE_CACHE",
        "TEXMK_LIB", "TEXMK_INTERNAL_MODE",
    ):
        run_env.pop(k, None)

    if ref_engine == "xelatex":
        engine_flag = "-xelatex"
    elif ref_engine == "lualatex":
        engine_flag = "-lualatex"
    else:
        engine_flag = "-pdf"
    cmd = [
        str(rust_bin),
        engine_flag,
        "-interaction=nonstopmode",
        "--keep-logs",
        "--cache-directory", str(rust_cache / "tex-rs"),
        tex_rel.name,
    ]

    run = run_command(
        cmd,
        work_dir=work_dir,
        cap_path=cap_path,
        timeout=timeout,
        env=run_env,
        mem_limit_mib=mem_limit_mib,
        max_capture_bytes=max_capture_bytes,
    )

    stem_pdf = work_dir / f"{job}.pdf"
    stem_log = work_dir / f"{job}.log"
    kept_pdf = out_root_of(idir) / "pdf" / f"{pdf_basename(idir.name, 'rust')}"
    pdf_stat = persist_valid_pdf(stem_pdf, kept_pdf)

    kept_log = None
    tex_capture = None
    if stem_log.is_file():
        kept_log = idir / "rust.tex.log"
        tex_capture = capture_file(
            stem_log, kept_log, max_bytes=max_capture_bytes,
            error_pattern=ERROR_LINE_RE,
        )

    stdout_text = _read_text(cap_path)
    log_text = _read_text(kept_log) if kept_log else ""

    errors = ((tex_capture or {}).get("errors")
              or run["capture"].get("errors") or [])
    if not errors and run["spawn_error"]:
        errors = [f"spawn-error: {run['spawn_error']}"]

    font_embedding_errors: list[str] = []
    if pdf_stat["pdf_valid"] and kept_pdf.is_file():
        try:
            font_embedding_errors = validate_pdf_font_embedding(kept_pdf, {})
        except Exception as e:
            font_embedding_errors = [f"validator error: {e}"]

    pass_matches = re.findall(r"(\d+) \S+ pass\(es\)|failed on pass (\d+)", stdout_text)
    passes = int(next(value for value in pass_matches[-1] if value)) if pass_matches else 0
    bib_matches = re.findall(r"(\d+) bibtex run\(s\)", stdout_text)
    bib_runs = [{"tool": "tex-bibtex"}
                for _ in range(int(bib_matches[-1]) if bib_matches else 0)]

    converged = bool(run["exit"] == 0 and pdf_stat["pdf_valid"]
                     and not run["timed_out"] and not run["mem_killed"])
    status = classify(run, pdf_stat["pdf_valid"], errors)
    if status == "clean" and not converged:
        status = "unconverged"

    failure_kind = classify_failure_kind(
        run,
        pdf_stat,
        errors,
        log_text=log_text,
        font_errors=font_embedding_errors,
    )

    rec = {
        "engine": "rust",
        "driver": "ratex",
        "bin": str(rust_bin),
        "ref_engine": ref_engine,
        "status": status,
        "failure_kind": failure_kind,
        "exit": run["exit"],
        "timed_out": run["timed_out"],
        "mem_killed": run["mem_killed"],
        "kill_signal": run["kill_signal"],
        "time_ms": round(run["time_ms"], 1),
        "passes": passes,
        "converged": converged,
        "pass_records": [{"pass": 1, "cmd": run["cmd"], "time_ms": round(run["time_ms"], 1), "exit": run["exit"]}],
        "bibtex_runs": bib_runs,
        "shipped_bbl": bbl_is_source,
        "errors": errors,
        "aux_hashes": aux_state(work_dir, job),
        "font_embedding_errors": font_embedding_errors,
        "font_embedding_valid": len(font_embedding_errors) == 0,
        "capture": run["capture"],
        "tex_log_capture": tex_capture,
        "captured_log": str(cap_path) if cap_path.is_file() else None,
        "tex_log": str(kept_log) if kept_log and kept_log.is_file() else None,
        "pass_log_dir": None,
        "cmd": run["cmd"],
    }
    rec.update(pdf_stat)
    rec["pdf"] = str(kept_pdf) if kept_pdf.is_file() else None
    rec["pdf_produced"] = pdf_stat["pdf_exists"]
    rec["pdf_exists"] = kept_pdf.is_file()
    return rec


def exact_parity_compare(aid: str, a_path: Path | None, b_path: Path | None,
                         a_pages: int | None, b_pages: int | None,
                         out_root: Path, dpi: float, page_min: float,
                         doc_min: float,
                         rust_font_errors: list[str] | None = None,
                         ref_font_errors: list[str] | None = None) -> dict:
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
                 "repaired": None,
                 "font_embedding_failure": bool(rust_font_errors or ref_font_errors),
                 "font_embedding_errors": list(rust_font_errors or []) + list(ref_font_errors or []),
                 "font_embedding_errors_by_engine": {
                     "rust": list(rust_font_errors or []),
                     "ref": list(ref_font_errors or []),
                 },
                 "text_extraction_failure": False,
                 "text_extraction_error": None,
                 "mean_text_similarity": None}
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
        out["producer_ref"] = db.metadata.get("producer") or ""
        out["repaired"] = {"rust": bool(da.is_repaired),
                           "ref": bool(db.is_repaired)}
        out["page_count_match"] = a_pages == b_pages
        n = min(a_pages, b_pages)
        differing = total = 0
        geometry_ok = True
        worst_pct: float | None = None
        worst_page = 0
        text_sims: list[float] = []
        for i in range(n):
            pa, pb = da[i], db[i]
            pymupdf.TOOLS.mupdf_warnings(reset=True)
            a = pa.get_pixmap(matrix=pymupdf.Matrix(dpi / 72.0, dpi / 72.0),
                              colorspace=pymupdf.csRGB, alpha=False)
            warn = pymupdf.TOOLS.mupdf_warnings(reset=True)
            if warn:
                out["raster_warnings"].append({"page": i + 1, "text": warn[:500]})
            b = pb.get_pixmap(matrix=pymupdf.Matrix(dpi / 72.0, dpi / 72.0),
                              colorspace=pymupdf.csRGB, alpha=False)
            pymupdf.TOOLS.mupdf_warnings(reset=True)
            pixels = a.width * a.height
            ta = _norm_text(pa)
            tb = _norm_text(pb)
            sim = round(difflib.SequenceMatcher(None, ta, tb).ratio(), 4)
            text_sims.append(sim)
            entry = {"page": i + 1, "pixels": pixels,
                     "rust_dims_px": [a.width, a.height],
                     "ref_dims_px": [b.width, b.height],
                     "rust_size_pt": [round(pa.rect.width, 3), round(pa.rect.height, 3)],
                     "ref_size_pt": [round(pb.rect.width, 3), round(pb.rect.height, 3)],
                     "dims_match": (a.width, a.height) == (b.width, b.height),
                     "text_similarity": sim}
            if tb.strip() and not ta.strip():
                entry["extraction_failure"] = True
                out["text_extraction_failure"] = True
                if not out["text_extraction_error"]:
                    out["text_extraction_error"] = f"page {i + 1} text layer missing in rust PDF"
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
        if text_sims:
            out["mean_text_similarity"] = round(statistics.fmean(text_sims), 4)
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
                 "manifest_index": entry.get("manifest_index"),
                 "prepared": entry.get("prepared", {})}
    try:
        if entry.get("blocker"):
            raise RuntimeError(entry["blocker"])
        src: Path = entry["src_dir"]
        if not src.is_dir():
            raise FileNotFoundError(f"corpus source directory missing: {src}")
        tex_rel = Path(entry["main_tex"])
        main_tex_file = src / tex_rel
        if not main_tex_file.is_file():
            raise FileNotFoundError(f"main_tex source file missing: {main_tex_file}")
        ws_root = out_root / "work" / aid
        ws_rust = ws_root / "rust"
        ws_ref = ws_root / "ref"
        extra_ignore = PRIVATE_IGNORE if entry["kind"] == "private" else ()
        stripped_r = freeze_workspace(
            src, ws_rust, tex_rel, extra_ignore, out_root=out_root
        )
        stripped_f = freeze_workspace(
            src, ws_ref, tex_rel, extra_ignore, out_root=out_root
        )
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
        ref_engine = detect_ref_engine(src, tex_rel, entry)
        res["ref_engine"] = ref_engine
        res["ref_bin"] = cfg["ref_bins"][ref_engine]
        res["rust_bin"] = cfg["rust_bin"]
        res["rust_bin_sha256"] = cfg.get("rust_bin_sha256")
        res["ref_bin_sha256"] = cfg.get("ref_bins_info", {}).get(ref_engine, {}).get("sha256")
        res["main_tex_sha256"] = sha256_file(main_tex_file) if main_tex_file.is_file() else None

        rust_home = ws_rust / ".home"
        rust_cache = ws_rust / ".cache"
        rust_home.mkdir(parents=True, exist_ok=True)
        rust_cache.mkdir(parents=True, exist_ok=True)

        ref_home = ws_ref / ".home"
        ref_cache = ws_ref / ".cache"
        ref_home.mkdir(parents=True, exist_ok=True)
        ref_cache.mkdir(parents=True, exist_ok=True)

        p_rust_env, p_ref_env, _, _ = campaign_environments(
            cfg.get("overlay_path"),
            rust_home=rust_home, rust_cache=rust_cache,
            ref_home=ref_home, ref_cache=ref_cache
        )

        engine_filter = cfg.get("engine_filter", "both")
        if engine_filter in ("both", "ref"):
            ref = compile_campaign_ref(
                ws_ref, tex_rel, idir, p_ref_env, cfg["timeout"],
                cfg, ref_engine, cfg["mem_limit_mib"], cfg["max_capture_bytes"]
            )
        else:
            ref = {
                "engine": "ref", "driver": "latexmk", "status": "skipped",
                "failure_kind": "skipped", "converged": True, "pdf_valid": False,
                "pages": None, "pdf": None, "exit": 0, "errors": [],
                "time_ms": 0.0, "passes": 0, "font_embedding_errors": [],
                "font_embedding_valid": True,
            }

        if engine_filter in ("both", "rust"):
            rust = compile_campaign_rust(
                ws_rust, tex_rel, idir, p_rust_env, cfg["timeout"],
                cfg, ref_engine, cfg["mem_limit_mib"], cfg["max_capture_bytes"]
            )
        else:
            rust = {
                "engine": "rust", "driver": "ratex", "status": "skipped",
                "failure_kind": "skipped", "converged": True, "pdf_valid": False,
                "pages": None, "pdf": None, "exit": 0, "errors": [],
                "time_ms": 0.0, "passes": 0, "font_embedding_errors": [],
                "font_embedding_valid": True,
            }
        res["ref"] = ref
        res["rust"] = rust

        if engine_filter == "both":
            cmp = exact_parity_compare(
                aid,
                Path(rust["pdf"]) if rust["pdf"] else None,
                Path(ref["pdf"]) if ref["pdf"] else None,
                rust["pages"], ref["pages"], out_root, cfg["dpi"],
                cfg["page_min"], cfg["doc_min"],
                rust_font_errors=rust.get("font_embedding_errors"),
                ref_font_errors=ref.get("font_embedding_errors"))
        else:
            cmp = {
                "compared": False,
                "note": f"{engine_filter}-only run",
                "page_failures": [],
                "per_page": [],
                "raster_warnings": [],
            }
        res["compare"] = cmp
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
        is_source_err = isinstance(e, FileNotFoundError) or "source" in str(e).lower() or "main_tex" in str(e).lower() or "missing" in str(e).lower()
        f_kind = "invalid-source" if is_source_err else "compilation"
        res.setdefault("rust", {"engine": "rust", "status": "failure",
                                "failure_kind": f_kind,
                                "exit": None, "pdf_valid": False, "pages": None,
                                "errors": [f"harness: {e}"], "pdf": None,
                                "converged": False})
        res.setdefault("ref", {"engine": "ref", "status": "failure",
                               "failure_kind": f_kind,
                               "exit": None, "pdf_valid": False, "pages": None,
                               "errors": [f"harness: {e}"], "pdf": None,
                               "converged": False})
        res.setdefault("compare", {"compared": False, "note": f"harness error: {e}",
                                   "page_failures": [], "per_page": [],
                                   "raster_warnings": []})
    res["finished_utc"] = now_iso()
    apply_artifact_retention(res, out_root, cfg)
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
        engine_filter = cfg.get("engine_filter", "both")
        for eng in ("rust", "ref"):
            if engine_filter != "both" and eng != engine_filter:
                continue
            x = r.get(eng) or {}
            if x.get("status") != "clean" or not x.get("converged"):
                f_kind = x.get("failure_kind") or "compilation"
                failures.append({"kind": f_kind, "id": aid,
                                 "engine": eng, "status": x.get("status"),
                                 "converged": x.get("converged"),
                                 "passes": x.get("passes"),
                                 "errors": (x.get("errors") or [])[:3]})
            elif not x.get("pdf_valid"):
                f_kind = x.get("failure_kind") or "compilation"
                failures.append({"kind": f_kind, "id": aid,
                                 "engine": eng})
        if engine_filter != "both":
            continue
        # oracle_pages_note is provenance only (active source drift); a
        # genuinely missing source/dependency already raised harness_error.
        c = r.get("compare") or {}
        rust_state = r.get("rust") or {}
        ref_state = r.get("ref") or {}
        font_errs = (
            c.get("font_embedding_errors")
            or (
                (rust_state.get("font_embedding_errors") or [])
                + (ref_state.get("font_embedding_errors") or [])
            )
        )
        if (
            font_errs
            or c.get("font_embedding_failure")
            or rust_state.get("font_embedding_errors")
            or ref_state.get("font_embedding_errors")
        ):
            if not any(f.get("id") == aid and f.get("kind") == "embedding" for f in failures):
                failures.append({"kind": "embedding", "id": aid,
                                 "errors": (font_errs or [])[:5]})
        if c.get("text_extraction_failure"):
            failures.append({"kind": "extraction", "id": aid,
                             "detail": c.get("text_extraction_error") or "text extraction failed"})
        if not c.get("compared"):
            failures.append({"kind": "not-compared", "id": aid,
                             "note": c.get("note")})
            continue
        if not c.get("page_count_match"):
            failures.append({"kind": "render", "subkind": "page-count", "id": aid})
        if not c.get("geometry_match"):
            failures.append({"kind": "render", "subkind": "geometry", "id": aid})
        if c.get("raster_warnings"):
            failures.append({"kind": "raster-warnings", "id": aid,
                             "count": len(c["raster_warnings"])})
        if c.get("producer_rust") != "tex-rs":
            failures.append({"kind": "producer", "id": aid,
                             "producer": c.get("producer_rust")})
        for pf in c.get("page_failures") or []:
            failures.append({"kind": "render", "subkind": "page-parity", "id": aid, **pf,
                             "min_pct": cfg["page_min"]})
        dp = c.get("document_exact_parity")
        if dp is None or dp < cfg["doc_min"]:
            failures.append({"kind": "render", "subkind": "doc-parity", "id": aid,
                             "parity": dp, "min_pct": cfg["doc_min"]})
    return {"ok": not failures, "gate_utc": now_iso(), "mode": "campaign",
            "expected": len(ids), "completed": len(results),
            "dpi": cfg["dpi"], "page_min_pct": cfg["page_min"],
            "doc_min_pct": cfg["doc_min"], "failures": failures}


def qualification_ledger(selected: list[dict], results: dict[str, dict],
                         doc_min: float) -> dict:
    """Deterministic per-project ledger for the scalable parity objective.

    A project qualifies only when both engines compile cleanly to valid PDFs,
    the complete documents have matching page geometry/counts, rasterization
    is trustworthy, and document exact-pixel parity is strictly above
    ``doc_min``. Per-page parity is diagnostic and is deliberately not a
    qualification criterion.
    """
    projects = []
    for entry in selected:
        aid = entry["id"]
        result = results.get(aid)
        reasons: list[str] = []
        parity = None
        if result is None:
            reasons.append("missing-result")
        else:
            if result.get("harness_error"):
                reasons.append("harness-error")
            for engine in ("rust", "ref"):
                state = result.get(engine) or {}
                if state.get("status") != "clean":
                    reasons.append(f"{engine}-status:{state.get('status')}")
                if not state.get("converged"):
                    reasons.append(f"{engine}-not-converged")
                if not state.get("pdf_valid"):
                    reasons.append(f"{engine}-invalid-pdf")
                if state.get("font_embedding_errors"):
                    reasons.append(f"{engine}-font-embedding")
            compare = result.get("compare") or {}
            if not compare.get("compared"):
                reasons.append("not-compared")
            else:
                parity = compare.get("document_exact_parity")
                if not compare.get("page_count_match"):
                    reasons.append("page-count-mismatch")
                if not compare.get("geometry_match"):
                    reasons.append("geometry-mismatch")
                if compare.get("raster_warnings"):
                    reasons.append("raster-warning")
                prod_rust = compare.get("producer_rust")
                prod_ref = compare.get("producer_ref")
                if prod_rust != "tex-rs" and (not prod_ref or prod_rust != prod_ref):
                    reasons.append("wrong-rust-producer")
                if (
                    compare.get("font_embedding_failure")
                    and not any(r.endswith("-font-embedding") for r in reasons)
                ):
                    reasons.append("font-embedding")
                if compare.get("text_extraction_failure"):
                    reasons.append("text-extraction")
                if parity is None or parity <= doc_min:
                    reasons.append("document-parity-not-above-threshold")
        projects.append({
            "id": aid,
            "kind": entry.get("kind"),
            "archive": entry.get("archive"),
            "manifest_index": entry.get("manifest_index"),
            "qualified": not reasons,
            "document_exact_parity": parity,
            "reasons": reasons,
        })
    qualified = [project["id"] for project in projects if project["qualified"]]
    return {
        "criteria": {
            "document_exact_parity_strictly_above_pct": doc_min,
            "both_engines_clean_and_converged": True,
            "both_pdfs_valid": True,
            "both_pdfs_font_embedding_valid": True,
            "page_count_and_geometry_match": True,
            "no_raster_warnings": True,
            "rust_pdf_producer": "tex-rs",
        },
        "selected": len(selected),
        "completed": len(results),
        "qualified_count": len(qualified),
        "qualified_ids": qualified,
        "projects": projects,
    }


def summarize_campaign(results: dict[str, dict], page_min: float,
                       doc_min: float) -> dict:
    eng = {e: {s: 0 for s in
              ("clean", "errors", "failure", "timeout", "unconverged",
               "memlimit", "skipped")}
           for e in ("rust", "ref")}
    failure_kinds = {e: {} for e in ("rust", "ref")}
    for r in results.values():
        for e in ("rust", "ref"):
            st = (r.get(e) or {}).get("status")
            if st in eng[e]:
                eng[e][st] += 1
            fk = (r.get(e) or {}).get("failure_kind") or "clean"
            failure_kinds[e][fk] = failure_kinds[e].get(fk, 0) + 1
    rastered = [r for r in results.values()
                if (r.get("compare") or {}).get("compared")]
    doc_p = [r["compare"]["document_exact_parity"] for r in rastered
             if r["compare"].get("document_exact_parity") is not None]
    pages_all = [p["exact_parity_pct"] for r in rastered
                 for p in r["compare"]["per_page"]
                 if p.get("exact_parity_pct") is not None]
    font_embedding_failures_by_engine = {
        "rust": sum(
            1 for r in results.values()
            if bool((r.get("rust") or {}).get("font_embedding_errors"))
        ),
        "ref": sum(
            1 for r in results.values()
            if bool((r.get("ref") or {}).get("font_embedding_errors"))
        ),
    }
    font_embedding_failures = sum(
        1 for r in results.values()
        if bool((r.get("rust") or {}).get("font_embedding_errors"))
        or bool((r.get("ref") or {}).get("font_embedding_errors"))
        or bool((r.get("compare") or {}).get("font_embedding_failure"))
    )
    text_extraction_failures = sum(
        1 for r in results.values()
        if bool((r.get("compare") or {}).get("text_extraction_failure"))
    )
    return {
        "projects_completed": len(results),
        "status_counts": eng,
        "failure_kind_counts": failure_kinds,
        "font_embedding_failures": font_embedding_failures,
        "font_embedding_failures_by_engine": font_embedding_failures_by_engine,
        "text_extraction_failures": text_extraction_failures,
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
                 a_pages: int | None, b_pages: int | None,
                 rust_font_errors: list[str] | None = None,
                 ref_font_errors: list[str] | None = None) -> dict:
    """a = rust output, b = reference output."""
    out = {
        "raster_dpi": DPI, "compared": False, "page_count_match": None,
        "pages_compared": 0, "pages_identical": 0, "dims_all_match": None,
        "mean_pixel_diff": None, "max_pixel_diff": None,
        "pct_pixels_diff": None, "worst_page": None,
        "text_similarity": None, "per_page": [], "note": None,
        "font_embedding_failure": bool(rust_font_errors or ref_font_errors),
        "font_embedding_errors": list(rust_font_errors or []) + list(ref_font_errors or []),
        "font_embedding_errors_by_engine": {
            "rust": list(rust_font_errors or []),
            "ref": list(ref_font_errors or []),
        },
        "text_extraction_failure": False,
        "text_extraction_error": None,
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
                    h = min(sa[1], sb[1])
                    w = min(sa[0], sb[0])
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
            text_a.append(ta)
            text_b.append(tb)
            if tb.strip() and not ta.strip():
                entry["extraction_failure"] = True
                out["text_extraction_failure"] = True
                if not out["text_extraction_error"]:
                    out["text_extraction_error"] = f"page {i + 1} text layer missing in rust PDF"
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
        da.close()
        db.close()
    return out


# --------------------------------------------------------------------------
# Per-project driver

def process_project(entry: dict, corpus_dir: Path, out_root: Path,
                    rust_bin: str, sys_bin: str, timeout: float,
                    mem_limit_mib: int, cfg: dict) -> dict:
    aid = entry["id"]
    src = corpus_dir / aid
    tex_rel = Path(entry["main_tex"])
    idir = out_root / "results" / aid
    idir.mkdir(parents=True, exist_ok=True)
    ws_rust = out_root / "work" / aid / "rust"
    ws_ref = out_root / "work" / aid / "ref"

    stripped_r = prepare_workspace(src, ws_rust, tex_rel, out_root=out_root)
    stripped_f = prepare_workspace(src, ws_ref, tex_rel, out_root=out_root)

    rust_home = ws_rust / ".home"
    rust_cache = ws_rust / ".cache"
    rust_home.mkdir(parents=True, exist_ok=True)
    rust_cache.mkdir(parents=True, exist_ok=True)

    ref_home = ws_ref / ".home"
    ref_cache = ws_ref / ".cache"
    ref_home.mkdir(parents=True, exist_ok=True)
    ref_cache.mkdir(parents=True, exist_ok=True)

    p_rust_env, p_ref_env, _, _ = campaign_environments(
        None, rust_home=rust_home, rust_cache=rust_cache,
        ref_home=ref_home, ref_cache=ref_cache
    )

    ref_engine = detect_ref_engine(src, tex_rel, entry)
    ref_bin = cfg.get("ref_bins", {}).get(ref_engine, sys_bin)
    res_rust = compile_engine("rust", rust_bin, ws_rust, tex_rel, idir,
                              timeout, mem_limit_mib,
                              cfg["max_capture_bytes"], env=p_rust_env,
                              ref_engine=ref_engine)
    res_ref = compile_engine("ref", ref_bin, ws_ref, tex_rel, idir,
                             timeout, mem_limit_mib,
                             cfg["max_capture_bytes"], env=p_ref_env,
                             ref_engine=ref_engine)

    cmp = compare_pdfs(
        Path(res_rust["pdf"]) if res_rust["pdf"] else None,
        Path(res_ref["pdf"]) if res_ref["pdf"] else None,
        res_rust["pages"], res_ref["pages"],
        rust_font_errors=res_rust.get("font_embedding_errors"),
        ref_font_errors=res_ref.get("font_embedding_errors"),
    )

    main_tex_file = src / tex_rel
    result = {
        "id": aid, "archive": entry.get("archive"), "main_tex": entry["main_tex"],
        "mode": "single-pass", "manifest_index": entry.get("manifest_index"),
        "ref_engine": ref_engine,
        "rust_bin": rust_bin,
        "rust_bin_sha256": cfg.get("rust_bin_sha256"),
        "sys_bin": ref_bin,
        "sys_bin_sha256": sha256_file(Path(ref_bin)) if Path(ref_bin).is_file() else cfg.get("sys_bin_sha256"),
        "main_tex_sha256": sha256_file(main_tex_file) if main_tex_file.is_file() else None,
        "finished_utc": now_iso(),
        "stripped_generated": {"rust": stripped_r, "ref": stripped_f},
        "rust": res_rust, "ref": res_ref, "compare": cmp,
    }
    apply_artifact_retention(result, out_root, cfg)
    return result


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
    """Converged exact-parity campaign: inventory, freeze,
    converge, compare, gate. Every failure/blocker is retained."""
    rust_bin = os.path.abspath(args.rust)
    if not Path(rust_bin).is_file() and args.engine != "ref":
        print(f"Error: rust binary not found: {rust_bin}", file=sys.stderr)
        return 1
    if args.engine != "ref" and not is_ratex_cli(rust_bin):
        print("Error: campaign mode requires the ratex or texmk driver; "
              "use --mode single-pass for raw engines", file=sys.stderr)
        return 1
    sys_latexmk = os.path.abspath(str(args.sys_latexmk)) if args.sys_latexmk else (shutil.which("latexmk") or "/usr/bin/latexmk")
    if not Path(sys_latexmk).is_file() and args.engine != "rust":
        print(f"Error: reference latexmk not found: {sys_latexmk}", file=sys.stderr)
        return 1
    sys_bibtex = "/usr/bin/bibtex"
    args.output = args.output.resolve()
    ref_bins = {}
    for engine, path in (("pdflatex", args.sys), ("xelatex", args.sys_xelatex),
                         ("lualatex", args.sys_lualatex)):
        ap_ = os.path.abspath(str(path))
        ref_bins[engine] = ap_
        if not Path(ap_).is_file() and args.engine != "rust":
            # missing reference engine: retained blocker for affected docs
            print(f"CAMPAIGN: reference engine missing: {engine} -> {ap_}",
                  file=sys.stderr)
    rust_bibtex = rust_bin if is_ratex_cli(rust_bin) else None
    rust_env, ref_env, removed_diag, overlay_vars = campaign_environments(args.overlay)
    if args.overlay and not Path(args.overlay).is_dir():
        print(f"CAMPAIGN: WARNING: overlay dir missing: {args.overlay}; "
              "running without it — dependency failures will be retained "
              "gate blockers", file=sys.stderr)
        rust_env, ref_env, removed_diag, overlay_vars = campaign_environments(None)
        args.overlay = None
    if overlay_vars:
        print(f"CAMPAIGN: Reference TEXMFHOME={overlay_vars['TEXMFHOME']} "
              f"TEXMFVAR={overlay_vars['TEXMFVAR']} (Rust engine isolated with TEX_RS_HERMETIC=1)")

    # ---- inventory: a deterministic manifest range plus optional privates ----
    manifest_file = args.corpus_dir / "manifest.json"
    if not manifest_file.is_file():
        print(f"Error: manifest not found at {manifest_file}", file=sys.stderr)
        return 1
    manifest = json.loads(manifest_file.read_text())
    if not isinstance(manifest, list):
        print("Error: corpus manifest must be a JSON list", file=sys.stderr)
        return 1
    try:
        for item in manifest:
            validate_manifest_entry(item)
    except ValueError as error:
        print(f"Error: {error}", file=sys.stderr)
        return 1
    if args.offset < 0 or args.limit < 0:
        print("Error: --offset and --limit must be non-negative", file=sys.stderr)
        return 1
    indexed_manifest = list(enumerate(manifest))
    selected_manifest = (
        indexed_manifest
        if args.only
        else indexed_manifest[args.offset:args.offset + args.limit]
    )
    entries: list[dict] = []
    if not args.private_only:
        entries += [{
            "id": item["id"],
            "kind": "corpus",
            "archive": item.get("archive"),
            "main_tex": item["main_tex"],
            "src_dir": args.corpus_dir / item["id"],
            "prepared": {},
            "manifest_index": index,
        } for index, item in selected_manifest]
    if not args.corpus_only:
        priv, priv_prov = build_private_entries()
        entries += priv
        args.priv_prov = priv_prov
    try:
        for entry in entries:
            validate_manifest_entry(entry)
    except ValueError as error:
        print(f"Error: {error}", file=sys.stderr)
        return 1
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
            if not all(isinstance(r.get(e), dict) and "status" in r[e] for e in ("rust", "ref")):
                continue
            if any(e["id"] == r["id"] for e in entries):
                results[r["id"]] = r
    todo = [e for e in entries if e["id"] not in results]

    cfg = {"rust_bin": rust_bin, "ref_bins": ref_bins,
           "rust_bin_sha256": sha256_file(rust_bin) if Path(rust_bin).is_file() else None,
           "ref_bins_info": {k: {"path": v, "sha256": sha256_file(v) if Path(v).is_file() else None} for k, v in ref_bins.items()},
           "sys_latexmk": sys_latexmk,
           "engine_filter": args.engine,
           "no_sandbox": args.no_sandbox,
           "rust_env": rust_env, "ref_env": ref_env,
           "env": ref_env, "timeout": args.timeout,
           "overlay_path": args.overlay,
           "mem_limit_mib": args.mem_limit_mib,
           "max_capture_bytes": args.max_capture_bytes,
           "dpi": args.dpi, "page_min": args.page_min, "doc_min": args.doc_min,
           "retain": args.retain, "keep_work": args.keep_work}
    # Reconcile resumed checkpoints with the requested policy.  All gate and
    # summary inputs live in JSON, so no retained artifact is needed to resume.
    for resumed in results.values():
        apply_artifact_retention(resumed, args.output, cfg)
        atomic_write_json(ckpt_dir / f"{resumed['id']}.json", resumed)
    meta = {
        "notice": f"CONVERGED CAMPAIGN: real latexmk reference vs public Ratex driver; "
                  f"gate = exact RGB parity @{args.dpi:g}dpi, every page >= {args.page_min}%, doc "
                  f">= {args.doc_min}%, identical geometry/page counts.",
        "sys_latexmk": sys_latexmk,
        "sys_latexmk_version": capture_version(sys_latexmk, 10),
        "sys_latexmk_sha256": sha256_file(Path(sys_latexmk)) if Path(sys_latexmk).is_file() else None,
        "engine_filter": args.engine,
        "sandbox": "disabled" if args.no_sandbox else ("bwrap" if shutil.which("bwrap") else "direct"),
        "started_utc": now_iso(),
        "corpus_dir": str(args.corpus_dir), "manifest": str(manifest_file),
        "manifest_sha256": sha256_file(manifest_file),
        "manifest_offset": args.offset,
        "manifest_limit": args.limit,
        "output": str(args.output), "jobs": args.jobs,
        "timeout_s": args.timeout,
        "mem_limit_mib": args.mem_limit_mib,
        "max_capture_bytes": args.max_capture_bytes,
        "dpi": args.dpi, "page_min_pct": args.page_min,
        "doc_min_pct": args.doc_min,
        "qualification_min_pct": args.qualification_min,
        "rust_bin": rust_bin, "rust_version": capture_version(args.rust, 10),
        "rust_bin_sha256": sha256_file(Path(rust_bin)) if Path(rust_bin).is_file() else None,
        "rust_bibtex": rust_bibtex,
        "rust_bibtex_version": (
            capture_version(rust_bibtex, 10, env={"TEXMK_INTERNAL_MODE": "bibtex"})
            if rust_bibtex else None
        ),
        "rust_bibtex_sha256": sha256_file(Path(rust_bibtex)) if rust_bibtex and Path(rust_bibtex).is_file() else None,
        "ref_bins": ref_bins,
        "ref_bins_info": {
            k: {
                "path": v,
                "sha256": sha256_file(v) if Path(v).is_file() else None,
                "version": capture_version(v, 10),
            } for k, v in ref_bins.items()
        },
        "sys_bibtex": sys_bibtex,
        "sys_bibtex_version": capture_version(sys_bibtex, 10),
        "sys_bibtex_sha256": sha256_file(Path(sys_bibtex)) if Path(sys_bibtex).is_file() else None,
        "overlay": str(args.overlay) if args.overlay else None,
        "overlay_vars": overlay_vars,
        "removed_diagnostics": removed_diag,
        "private_provenance": getattr(args, "priv_prov", {}),
        "pymupdf_version": pymupdf.__version__ if pymupdf else None, "numpy": np is not None,
        "selected": len(entries), "resumed": len(results), "planned": len(todo),
        "expected_total": (
            1000
            if args.corpus_only and not args.only and args.offset == 0
            and args.limit >= 1000 and args.corpus_dir == Path("corpus/standalone-1000")
            else (
                104
                if not (args.only or args.corpus_only or args.private_only)
                and args.offset == 0 and args.limit == 100
                and args.corpus_dir == Path("corpus")
                else None
            )
        ),
        "retain": args.retain, "keep_work": cfg["keep_work"],
        "artifact_policy": "bounded-retention-v1",
    }
    if meta["expected_total"] and len(entries) != meta["expected_total"]:
        print(f"Error: campaign expects {meta['expected_total']} documents, "
              f"inventory has {len(entries)}", file=sys.stderr)
        return 1
    lock = threading.Lock()
    last_aggregate_write = 0.0

    def publish(r: dict) -> None:
        nonlocal last_aggregate_write
        with lock:
            results[r["id"]] = r
            atomic_write_json(ckpt_dir / f"{r['id']}.json", r)
            now = time.monotonic()
            if len(results) % 100 != 0 and now - last_aggregate_write < 60.0:
                return
            last_aggregate_write = now
            atomic_write_json(args.output / "report.json",
                              {"meta": meta,
                               "summary": summarize_campaign(
                                   results, cfg["page_min"], cfg["doc_min"]),
                               "results": {k: results[k] for k in sorted(results)}})
            atomic_write_json(args.output / "gate.json",
                              campaign_gate(entries, results, cfg))
            atomic_write_json(
                args.output / "qualification.json",
                qualification_ledger(entries, results, args.qualification_min),
            )

    print("=" * 78)
    print(f"*** {meta['notice']} ***")
    print(f"Campaign {len(entries)} documents | rust={rust_bin} | latexmk={sys_latexmk}")
    print(f"  ref={ {k: Path(v).name for k, v in ref_bins.items()} }")
    print(f"jobs={args.jobs} timeout={args.timeout}s "
          f"mem-limit={args.mem_limit_mib}MiB output={args.output} "
          f"engine={args.engine} sandbox={'disabled' if args.no_sandbox else ('bwrap' if shutil.which('bwrap') else 'direct')} "
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
                if "retention" not in r:
                    apply_artifact_retention(r, args.output, cfg)
                done += 1
                publish(r)
                print_campaign_line(done, total, r)
    except KeyboardInterrupt:
        print("Interrupted — checkpointed results are preserved; rerun to resume.",
              file=sys.stderr)

    if not cfg["keep_work"]:
        # Private-document support files are copied under work/ at a shared
        # relative depth.  They are metrics inputs, not durable artifacts.
        for entry in entries:
            for rel in (entry.get("external_files") or {}).values():
                path = args.output / "work" / rel
                if _path_within(path, args.output / "work"):
                    path.unlink(missing_ok=True)
        try:
            (args.output / "work").rmdir()
        except OSError:
            pass

    gate = campaign_gate(entries, results, cfg)
    qualification = qualification_ledger(
        entries, results, args.qualification_min
    )
    atomic_write_json(args.output / "report.json",
                      {"meta": meta,
                       "summary": summarize_campaign(
                           results, args.page_min, args.doc_min),
                       "results": {k: results[k] for k in sorted(results)}})
    atomic_write_json(args.output / "gate.json", gate)
    atomic_write_json(args.output / "qualification.json", qualification)
    s = summarize_campaign(results, args.page_min, args.doc_min)
    dt = time.perf_counter() - t0
    print("=" * 78)
    print(f"CAMPAIGN ({len(results)}/{len(entries)} completed in {dt:.1f}s) "
          f"— public drivers own convergence and bibliography; "
          f"gate decides parity at {args.dpi:g}dpi")
    print(f"  rust status: {s['status_counts']['rust']}")
    print(f"  ref  status: {s['status_counts']['ref']}")
    print(f"  page-count match {s['page_count_match']} | geometry match "
          f"{s['geometry_match']} | fully identical {s['fully_identical_projects']}")
    print(f"  pages >= {args.page_min}%: {s['pages_ge_page_min']}/{s['pages_total']} "
          f"| docs >= {args.doc_min}%: {s['docs_ge_doc_min']}")
    print(f"  mean doc parity {s['mean_doc_exact_parity']}% | min doc "
          f"{s['min_doc_exact_parity']}% | min page {s['min_page_exact_parity']}%")
    print(f"  qualified > {args.qualification_min}%: "
          f"{qualification['qualified_count']}/{qualification['selected']} "
          f"-> {args.output / 'qualification.json'}")
    print(f"  GATE: {'PASS' if gate['ok'] else 'FAIL'} "
          f"({len(gate['failures'])} failure rows) -> {args.output / 'gate.json'}")
    print(f"  report: {args.output / 'report.json'}")
    print("=" * 78)
    return 0 if gate["ok"] else 1


def main() -> int:
    ap = argparse.ArgumentParser(
        description="Corpus/parity harness: Rust ratex/pdflatex vs system engines. "
                    "Default --mode campaign = converged exact 150dpi RGB parity gate; "
                    "--mode single-pass = legacy one-invocation 72dpi diagnostic.")
    ap.add_argument("--mode", choices=("campaign", "single-pass"),
                    default="campaign",
                    help="campaign = default goal-grade converged gate; "
                         "single-pass = legacy diagnostic only")
    ap.add_argument("--corpus-dir", type=Path,
                    default=Path("corpus/standalone-1000") if Path("corpus/standalone-1000/manifest.json").is_file() else Path("corpus"),
                    help="Corpus directory root (default: corpus/standalone-1000 or corpus)")
    ap.add_argument("--rust", type=Path,
                    default=Path("target/release/ratex"),
                    help="Rust ratex driver (default: target/release/ratex); raw engines require --mode single-pass")
    ap.add_argument("--sys", type=Path, default=Path("/usr/bin/pdflatex"),
                    help="Reference system pdflatex")
    ap.add_argument("--sys-xelatex", type=Path, default=Path("/usr/bin/xelatex"),
                    help="Reference system XeTeX (fontspec docs)")
    ap.add_argument("--sys-lualatex", type=Path,
                    default=Path("/usr/bin/lualatex"),
                    help="Reference system LuaTeX (luacode docs)")
    ap.add_argument("--sys-latexmk", type=Path,
                    default=Path("/usr/bin/latexmk") if Path("/usr/bin/latexmk").is_file() else Path(shutil.which("latexmk") or "latexmk"),
                    help="Reference system latexmk driver (default: /usr/bin/latexmk)")
    ap.add_argument("--engine", choices=("both", "ref", "rust"), default="both",
                    help="Which engine(s) to compile: both (default, full parity comparison), ref (reference latexmk only), rust (Ratex driver only)")
    ap.add_argument("--no-sandbox", action="store_true",
                    help="Disable bwrap sandbox isolation for reference latexmk")
    ap.add_argument("--overlay", type=Path,
                    default=Path("output/parity-deps/texmf"),
                    help="isolated TDS dependency overlay exported as TEXMFHOME "
                         "to reference engine (campaign mode)")
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
    ap.add_argument("--offset", type=int, default=0,
                    help="Zero-based first corpus manifest entry")
    ap.add_argument("--limit", type=int, default=1000,
                    help="Number of corpus manifest entries after --offset (default: 1000)")
    ap.add_argument("--only", type=str, default=None,
                    help="Comma-separated project ids (overrides range)")
    ap.add_argument("--qualification-min", type=float, default=95.0,
                    help="Strict document-parity threshold for qualification")
    ap.add_argument("--corpus-only", action="store_true",
                    help="campaign: exclude the four private documents")
    ap.add_argument("--private-only", action="store_true",
                    help="campaign: only the four private documents")
    ap.add_argument("--no-resume", action="store_true",
                    help="Ignore existing checkpoints and recompile everything")
    ap.add_argument("--retain", choices=("failures", "all", "none"),
                    default="failures",
                    help="Evidence retention: compact failures (default), all, "
                         "or none; reports/checkpoints are always kept")
    ap.add_argument("--max-capture-bytes", type=int,
                    default=DEFAULT_MAX_CAPTURE_BYTES,
                    help="Maximum retained bytes per child stream (default: "
                         "1 MiB as 128 KiB head + 896 KiB tail; 0 keeps all)")
    work_group = ap.add_mutually_exclusive_group()
    work_group.add_argument("--keep-work", dest="keep_work",
                            action="store_true",
                            help="Keep copied per-engine workspaces")
    work_group.add_argument("--no-keep-work", dest="keep_work",
                            action="store_false",
                            help="Deprecated compatibility alias; deleting "
                                 "copied workspaces is now the default")
    ap.set_defaults(keep_work=False)
    args = ap.parse_args()
    if args.max_capture_bytes < 0:
        ap.error("--max-capture-bytes must be non-negative")
    if args.mode == "campaign":
        return run_campaign(args)

    manifest_file = args.corpus_dir / "manifest.json"
    if not manifest_file.is_file():
        print(f"Error: manifest not found at {manifest_file}", file=sys.stderr)
        return 1
    manifest = json.loads(manifest_file.read_text())
    if not isinstance(manifest, list):
        print("Error: corpus manifest must be a JSON list", file=sys.stderr)
        return 1
    try:
        for item in manifest:
            validate_manifest_entry(item)
    except ValueError as error:
        print(f"Error: {error}", file=sys.stderr)
        return 1
    if not args.rust.is_file():
        print(f"Error: rust binary not found: {args.rust}", file=sys.stderr)
        return 1
    ref_bins = {}
    for engine, path in (("pdflatex", args.sys), ("xelatex", args.sys_xelatex),
                         ("lualatex", args.sys_lualatex)):
        ap_ = os.path.abspath(str(path))
        ref_bins[engine] = ap_
        if not Path(ap_).is_file():
            print(f"SINGLE-PASS: reference engine missing: {engine} -> {ap_}", file=sys.stderr)
    if not args.sys.is_file():
        print(f"Error: reference binary not found: {args.sys}", file=sys.stderr)
        return 1
    if args.offset < 0 or args.limit < 0:
        print("Error: --offset and --limit must be non-negative", file=sys.stderr)
        return 1
    indexed_manifest = list(enumerate(manifest))
    if args.only:
        wanted = [s.strip() for s in args.only.split(",") if s.strip()]
        by_id = {e["id"]: (i, e) for i, e in indexed_manifest}
        selected = [dict(by_id[w][1], manifest_index=by_id[w][0])
                    for w in wanted if w in by_id]
        missing = [w for w in wanted if w not in by_id]
        if missing:
            print(f"Error: ids not in manifest: {missing}", file=sys.stderr)
            return 1
    else:
        selected = [dict(entry, manifest_index=index)
                    for index, entry in indexed_manifest[
                        args.offset:args.offset + args.limit]]

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

    rust_env, ref_env, _, _ = campaign_environments(None)
    single_cfg = {
        "retain": args.retain,
        "keep_work": args.keep_work,
        "max_capture_bytes": args.max_capture_bytes,
        "doc_min": args.doc_min,
        "rust_env": rust_env,
        "ref_env": ref_env,
        "ref_bins": ref_bins,
        "rust_bin_sha256": sha256_file(Path(args.rust)) if Path(args.rust).is_file() else None,
        "sys_bin_sha256": sha256_file(Path(args.sys)) if Path(args.sys).is_file() else None,
    }
    for resumed in results.values():
        apply_artifact_retention(resumed, args.output, single_cfg)
        atomic_write_json(ckpt_dir / f"{resumed['id']}.json", resumed)

    meta = {
        "passes_per_engine": 1,
        "single_pass_notice": "SINGLE PASS: exactly one engine invocation per project; "
                              "bibtex is NOT run; source .bbl preserved.",
        "started_utc": now_iso(),
        "corpus_dir": str(args.corpus_dir), "manifest": str(manifest_file),
        "output": str(args.output), "jobs": args.jobs, "timeout_s": args.timeout,
        "mem_limit_mib": args.mem_limit_mib,
        "max_capture_bytes": args.max_capture_bytes,
        # abspath, NOT resolve(): argv0 must stay "<...>/pdflatex" — symlink
        # resolution would rename it to pdftex and select the wrong format.
        "rust_bin": os.path.abspath(args.rust),
        "rust_version": capture_version(args.rust, 10),
        "sys_bin": os.path.abspath(args.sys),
        "sys_version": capture_version(args.sys, 10),
        "ref_bins": ref_bins,
        "pymupdf_version": pymupdf.__version__ if pymupdf else None, "numpy": np is not None,
        "selected": len(selected), "resumed": len(results), "planned": len(todo),
        "retain": args.retain, "keep_work": args.keep_work,
        "artifact_policy": "bounded-retention-v1",
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
    print(f"                          | ref engines={ref_bins}")
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
                              args.mem_limit_mib, single_cfg): e["id"]
                    for e in todo}
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
                if "retention" not in r:
                    apply_artifact_retention(r, args.output, single_cfg)
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
