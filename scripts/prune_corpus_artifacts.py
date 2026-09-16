#!/usr/bin/env python3
"""Plan or apply safe cleanup of historical corpus-harness artifacts.

Planning is read-only and writes a JSON manifest.  Applying requires that
exact manifest, revalidates the harness report and every file's metadata, and
then performs only the listed deletions/compactions below the recorded root.

Examples:
  scripts/prune_corpus_artifacts.py --root output/campaign-old \
      --plan /tmp/campaign-old-prune.json
  scripts/prune_corpus_artifacts.py --apply /tmp/campaign-old-prune.json
"""

from __future__ import annotations

import argparse
import json
import stat
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import NoReturn

try:
    from bounded_capture import DEFAULT_MAX_CAPTURE_BYTES, capture_file
except ModuleNotFoundError:  # support `python -m scripts.prune_corpus_artifacts`
    from scripts.bounded_capture import DEFAULT_MAX_CAPTURE_BYTES, capture_file


SCHEMA = "tex-corpus-prune-v1"
MANAGED_DIRS = ("work", "pdf", "results", "worst")


def sha256_file(path: Path) -> str:
    import hashlib

    digest = hashlib.sha256()
    with open(path, "rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def fail(message: str) -> NoReturn:
    raise ValueError(message)


def validated_root(root: Path) -> tuple[Path, Path, dict]:
    resolved = root.resolve(strict=True)
    if any(part == "trust_own" for part in resolved.parts):
        fail("refusing to inspect or prune a trust_own tree")
    if not resolved.is_dir():
        fail(f"not a directory: {resolved}")
    report_path = resolved / "report.json"
    if not report_path.is_file() or report_path.is_symlink():
        fail(f"no regular harness report at {report_path}")
    try:
        report = json.loads(report_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        fail(f"invalid harness report: {exc}")
    if not isinstance(report, dict) or not isinstance(report.get("results"), dict):
        fail("report.json is not a corpus harness report (missing results map)")
    meta = report.get("meta")
    if not isinstance(meta, dict) or not (
        meta.get("mode") in ("campaign", "single-pass")
        or meta.get("passes_per_engine") == 1
    ):
        fail("report.json has no recognized corpus harness mode")
    return resolved, report_path, report


def project_failed(result: dict, report: dict) -> bool:
    if result.get("harness_error"):
        return True
    campaign = (
        result.get("mode") == "campaign"
        or (report.get("meta") or {}).get("mode") == "campaign"
    )
    for engine in ("rust", "ref"):
        state = result.get(engine) or {}
        if state.get("status") != "clean" or not state.get("pdf_valid"):
            return True
        if campaign and not state.get("converged"):
            return True
    compare = result.get("compare") or {}
    if not compare.get("compared") or not compare.get("page_count_match"):
        return True
    if campaign:
        meta = report.get("meta") or {}
        parity = compare.get("document_exact_parity")
        return bool(
            not compare.get("geometry_match")
            or compare.get("raster_warnings")
            or compare.get("producer_rust") != "tex-rs"
            or compare.get("page_failures")
            or parity is None
            or parity < float(meta.get("doc_min_pct", 99.0))
        )
    return bool(
        compare.get("pages_identical") != compare.get("pages_compared")
        or compare.get("unmatched_pages")
    )


def relative_if_managed(value, root: Path) -> str | None:
    if not isinstance(value, str) or not value:
        return None
    raw = Path(value)
    candidates = [raw if raw.is_absolute() else Path.cwd() / raw]
    # Reports commonly store output-root-relative strings such as
    # output/run/results/id/ref.log.  Recover the managed suffix without
    # depending on the directory from which pruning is invoked.
    for index, part in enumerate(raw.parts):
        if part in MANAGED_DIRS:
            candidates.append(root / Path(*raw.parts[index:]))
            break
    for path in candidates:
        try:
            rel = path.resolve(strict=False).relative_to(root)
        except (OSError, ValueError):
            continue
        if rel.parts and rel.parts[0] in MANAGED_DIRS:
            return rel.as_posix()
    return None


def failure_evidence(result: dict, root: Path) -> set[str]:
    keep: set[str] = set()
    for engine in ("rust", "ref"):
        state = result.get(engine) or {}
        # Historical harnesses recorded and copied malformed engine remnants.
        # Their size/hash stay in report.json, but the file is not evidence.
        if state.get("pdf_valid") is True:
            rel = relative_if_managed(state.get("pdf"), root)
            if rel:
                keep.add(rel)
        bibtex = state.get("bibtex_runs") or []
        if bibtex:
            rel = relative_if_managed(bibtex[-1].get("log"), root)
            if rel:
                keep.add(rel)
        passes = state.get("pass_records") or []
        last = passes[-1] if passes else {}
        for stable_key, pass_key in (
            ("captured_log", "stdout_log"),
            ("tex_log", "tex_log"),
        ):
            rel = relative_if_managed(state.get(stable_key), root)
            if rel is None:
                rel = relative_if_managed(last.get(pass_key), root)
            if rel:
                keep.add(rel)
    artifacts = (result.get("compare") or {}).get("worst_artifacts")
    if isinstance(artifacts, dict):
        for value in artifacts.values():
            rel = relative_if_managed(value, root)
            if rel:
                keep.add(rel)
    aid = result.get("id")
    if isinstance(aid, str):
        keep.add(f"results/{aid}/failure.json")
    return keep


def lstat_record(path: Path, rel: str, action: str, reason: str) -> dict:
    info = path.lstat()
    return {
        "action": action,
        "path": rel,
        "bytes": info.st_size,
        "mtime_ns": info.st_mtime_ns,
        "mode": stat.S_IFMT(info.st_mode),
        "reason": reason,
    }


def make_plan(root_arg: Path, plan_path: Path, max_capture_bytes: int) -> dict:
    if max_capture_bytes < 0:
        fail("--max-capture-bytes must be non-negative")
    root, report_path, report = validated_root(root_arg)
    results: dict = report["results"]
    failures = {
        aid
        for aid, result in results.items()
        if isinstance(result, dict) and project_failed(result, report)
    }
    keep: set[str] = set()
    for aid in failures:
        keep.update(failure_evidence(results[aid], root))

    actions: list[dict] = []
    for dirname in MANAGED_DIRS:
        base = root / dirname
        if not base.exists():
            continue
        if base.is_symlink():
            fail(f"managed artifact directory is a symlink: {base}")
        for path in sorted(base.rglob("*")):
            if path.is_dir() and not path.is_symlink():
                continue
            rel = path.relative_to(root).as_posix()
            if any(part == "trust_own" for part in Path(rel).parts):
                fail(f"refusing to plan an action beneath trust_own: {rel}")
            parts = Path(rel).parts
            aid = None
            if dirname == "results" and len(parts) >= 2:
                aid = parts[1]
            elif dirname == "pdf" and len(parts) >= 2:
                name = parts[1]
                for suffix in (".rust.pdf", ".ref.pdf"):
                    if name.endswith(suffix):
                        aid = name[: -len(suffix)]
                        break
            elif dirname == "worst" and len(parts) >= 2:
                aid = parts[1].rsplit("-p", 1)[0]
            if dirname == "work":
                actions.append(lstat_record(path, rel, "delete", "copied-workspace"))
            elif aid is None or aid not in results:
                # Unknown files are preserved rather than guessed to be owned.
                continue
            elif aid not in failures:
                actions.append(
                    lstat_record(path, rel, "delete", "passing-project-evidence")
                )
            elif rel not in keep:
                actions.append(
                    lstat_record(path, rel, "delete", "duplicate-failure-evidence")
                )
            elif (
                not path.is_symlink()
                and path.suffix == ".log"
                and path.stat().st_size > max_capture_bytes
                and max_capture_bytes > 0
            ):
                action = lstat_record(path, rel, "compact", "oversized-failure-log")
                action["sha256"] = sha256_file(path)
                actions.append(action)

    plan = {
        "schema": SCHEMA,
        "created_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "root": str(root),
        "report": "report.json",
        "report_sha256": sha256_file(report_path),
        "max_capture_bytes": max_capture_bytes,
        "project_counts": {
            "reported": len(results),
            "failures": len(failures),
            "passing": len(results) - len(failures),
        },
        "actions": actions,
        "estimated_bytes_removed": sum(
            item["bytes"] for item in actions if item["action"] == "delete"
        ),
        "estimated_bytes_compacted": sum(
            max(0, item["bytes"] - max_capture_bytes)
            for item in actions
            if item["action"] == "compact"
        ),
        "preserved": [
            "report.json",
            "gate.json",
            "qualification.json",
            "checkpoints/",
            "bounded final failure logs",
            "valid failure PDFs",
            "failure render triplets",
            "failure reproduction metadata",
        ],
    }
    plan_path.parent.mkdir(parents=True, exist_ok=True)
    with open(plan_path, "x", encoding="utf-8") as stream:
        json.dump(plan, stream, indent=2)
        stream.write("\n")
    return plan


def validate_action(root: Path, item: dict) -> Path:
    rel = item.get("path")
    if not isinstance(rel, str):
        fail("plan action has no path")
    rel_path = Path(rel)
    if rel_path.is_absolute() or ".." in rel_path.parts or not rel_path.parts:
        fail(f"unsafe relative path in plan: {rel!r}")
    if rel_path.parts[0] not in MANAGED_DIRS:
        fail(f"path is outside managed artifact directories: {rel}")
    if any(part == "trust_own" for part in rel_path.parts):
        fail(f"refusing to apply an action beneath trust_own: {rel}")
    path = root / rel_path
    try:
        path.resolve(strict=False).relative_to(root)
    except (OSError, ValueError):
        fail(f"path escapes the recorded root: {rel}")
    try:
        info = path.lstat()
    except FileNotFoundError:
        fail(f"planned file disappeared: {rel}")
    expected = (item.get("bytes"), item.get("mtime_ns"), item.get("mode"))
    actual = (info.st_size, info.st_mtime_ns, stat.S_IFMT(info.st_mode))
    if actual != expected:
        fail(f"planned file changed since review: {rel}")
    if item.get("action") not in ("delete", "compact"):
        fail(f"unknown plan action for {rel}: {item.get('action')!r}")
    if item.get("action") == "compact" and not stat.S_ISREG(info.st_mode):
        fail(f"refusing to compact non-regular file: {rel}")
    if item.get("action") == "compact" and path.suffix != ".log":
        fail(f"refusing to compact a non-log artifact: {rel}")
    if item.get("action") == "compact" and sha256_file(path) != item.get("sha256"):
        fail(f"planned log content changed since review: {rel}")
    return path


def apply_plan(plan_path: Path) -> dict:
    try:
        plan = json.loads(plan_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        fail(f"invalid prune plan: {exc}")
    if not isinstance(plan, dict) or plan.get("schema") != SCHEMA:
        fail(f"unsupported prune plan schema: {plan.get('schema')!r}")
    root, report_path, _report = validated_root(Path(plan.get("root", "")))
    if sha256_file(report_path) != plan.get("report_sha256"):
        fail("report.json changed since the plan was reviewed")
    actions = plan.get("actions")
    if not isinstance(actions, list):
        fail("prune plan has no actions list")
    paths = [item.get("path") for item in actions if isinstance(item, dict)]
    if len(paths) != len(actions) or len(set(paths)) != len(paths):
        fail("prune plan contains invalid or duplicate action paths")
    max_bytes = int(plan.get("max_capture_bytes", DEFAULT_MAX_CAPTURE_BYTES))
    if max_bytes < 0:
        fail("prune plan has a negative capture limit")

    # Validate every action before changing any file.
    validated = [(item, validate_action(root, item)) for item in actions]
    removed = compacted = reclaimed = 0
    for item, path in validated:
        before = item["bytes"]
        if item["action"] == "delete":
            path.unlink()
            removed += 1
            reclaimed += before
        else:
            capture_file(path, path, max_bytes=max_bytes)
            after = path.stat().st_size
            compacted += 1
            reclaimed += max(0, before - after)

    for dirname in MANAGED_DIRS:
        base = root / dirname
        if not base.is_dir() or base.is_symlink():
            continue
        for directory in sorted(
            (p for p in base.rglob("*") if p.is_dir() and not p.is_symlink()),
            key=lambda p: len(p.parts),
            reverse=True,
        ):
            try:
                directory.rmdir()
            except OSError:
                pass
        try:
            base.rmdir()
        except OSError:
            pass
    return {
        "removed_files": removed,
        "compacted_files": compacted,
        "bytes_reclaimed": reclaimed,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument(
        "--root", type=Path, help="Historical harness root to inspect (dry run)"
    )
    mode.add_argument(
        "--apply", type=Path, help="Reviewed JSON plan to revalidate and apply"
    )
    parser.add_argument(
        "--plan", type=Path, help="New JSON plan path required with --root"
    )
    parser.add_argument(
        "--max-capture-bytes",
        type=int,
        default=DEFAULT_MAX_CAPTURE_BYTES,
        help="Failure-log size after compaction (default: 1 MiB)",
    )
    args = parser.parse_args()
    try:
        if args.root:
            if args.plan is None:
                parser.error("--plan is required with --root")
            plan = make_plan(args.root, args.plan, args.max_capture_bytes)
            print(
                json.dumps(
                    {
                        "plan": str(args.plan),
                        "actions": len(plan["actions"]),
                        "estimated_bytes_reclaimed": (
                            plan["estimated_bytes_removed"]
                            + plan["estimated_bytes_compacted"]
                        ),
                        "apply": f"{Path(__file__).name} --apply {args.plan}",
                    },
                    indent=2,
                )
            )
        else:
            if args.plan is not None:
                parser.error("--plan is only valid with --root")
            print(json.dumps(apply_plan(args.apply), indent=2))
    except (OSError, ValueError) as exc:
        print(f"PRUNE: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
