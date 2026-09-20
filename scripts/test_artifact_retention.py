#!/usr/bin/env python3
"""Focused tests for bounded harness output and artifact retention."""

from __future__ import annotations

import hashlib
import io
import json
import re
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

from bounded_capture import CaptureBuffer, run_bounded  # noqa: E402
import bench_cold  # noqa: E402
import test_corpus  # noqa: E402
from bench_cold import format_provenance, timed_compile  # noqa: E402
from prune_corpus_artifacts import apply_plan, make_plan  # noqa: E402
from test_corpus import (  # noqa: E402
    apply_artifact_retention,
    campaign_gate,
    classify_failure_kind,
    compile_engine,
    prepare_workspace,
    qualification_ledger,
    validate_manifest_entry,
)


def campaign_result(root: Path, aid: str, *, failed: bool = False) -> dict:
    idir = root / "results" / aid
    pdfdir = root / "pdf"
    worst = root / "worst"
    for directory in (idir / "passlogs", pdfdir, worst, root / "work" / aid / "rust"):
        directory.mkdir(parents=True, exist_ok=True)
    artifacts = {}
    for suffix in ("rust", "ref", "diff"):
        path = worst / f"{aid}-p1-{suffix}.png"
        path.write_bytes(suffix.encode())
        artifacts[suffix] = str(path)
    engines = {}
    for engine in ("rust", "ref"):
        pdf = pdfdir / f"{aid}.{engine}.pdf"
        stdout = idir / f"{engine}.stdout.log"
        texlog = idir / f"{engine}.tex.log"
        old = idir / "passlogs" / f"{engine}-pass1.stdout.log"
        pdf.write_bytes(b"pdf")
        stdout.write_bytes(b"stdout")
        texlog.write_bytes(b"tex log")
        old.write_bytes(b"old")
        engines[engine] = {
            "status": "errors" if failed and engine == "rust" else "clean",
            "exit": 1 if failed and engine == "rust" else 0,
            "converged": not (failed and engine == "rust"),
            "pdf_valid": True,
            "pdf": str(pdf),
            "pages": 1,
            "errors": ["! failure"] if failed and engine == "rust" else [],
            "captured_log": str(stdout),
            "tex_log": str(texlog),
            "pass_records": [
                {"cmd": [engine], "stdout_log": str(old), "tex_log": None}
            ],
        }
    return {
        "id": aid,
        "mode": "campaign",
        "main_tex": "main.tex",
        **engines,
        "compare": {
            "compared": True,
            "page_count_match": True,
            "geometry_match": True,
            "raster_warnings": [],
            "producer_rust": "tex-rs",
            "page_failures": ([{"page": 1}] if failed else []),
            "document_exact_parity": 90.0 if failed else 100.0,
            "worst_artifacts": artifacts,
        },
    }


class CaptureTests(unittest.TestCase):
    def test_head_tail_hash_and_error_scan_cover_complete_stream(self) -> None:
        pattern = re.compile(r"^!")
        capture = CaptureBuffer(1024, head_bytes=128, error_pattern=pattern)
        payload = b"! first error\n" + b"x" * 3000 + b"TAIL"
        for start in range(0, len(payload), 73):
            capture.feed(payload[start : start + 73])
        capture.finish()
        data = capture.bytes_for_file()
        meta = capture.metadata()
        self.assertEqual(meta["bytes"], len(payload))
        self.assertEqual(meta["sha256"], hashlib.sha256(payload).hexdigest())
        self.assertTrue(meta["truncated"])
        self.assertEqual(meta["retained_bytes"], 1024)
        self.assertTrue(data.startswith(payload[:128]))
        self.assertLessEqual(len(data), 1024)
        self.assertTrue(data.endswith(payload[-meta["tail_bytes"] :]))
        self.assertGreater(meta["tail_bytes"], 800)
        self.assertEqual(meta["errors"], ["! first error"])

    def test_subprocess_capture_is_streamed_and_bounded(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "child.log"
            child = run_bounded(
                [
                    sys.executable,
                    "-c",
                    "import sys; sys.stdout.buffer.write(b'!' + b'E'*20 + b'\\n' + b'x'*200000)",
                ],
                cwd=Path(tmp),
                output_path=log,
                timeout=10,
                max_bytes=4096,
                error_pattern=re.compile(r"^!"),
            )
            self.assertEqual(child["returncode"], 0)
            self.assertEqual(child["capture"]["bytes"], 200022)
            self.assertTrue(child["capture"]["truncated"])
            self.assertLess(log.stat().st_size, 4300)
            self.assertEqual(child["capture"]["errors"], ["!" + "E" * 20])

    def test_timeout_kills_child_and_preserves_partial_capture(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "timeout.log"
            child = run_bounded(
                [
                    sys.executable,
                    "-c",
                    "import sys,time; print('started', flush=True); time.sleep(5)",
                ],
                cwd=Path(tmp),
                output_path=log,
                timeout=0.05,
                max_bytes=1024,
            )
            self.assertTrue(child["timed_out"])
            self.assertIn(b"started", log.read_bytes())
            self.assertLessEqual(log.stat().st_size, 1024)

    def test_cold_benchmark_accepts_embedded_format_and_bounds_log(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = root / "pdflatex"
            binary.write_text(
                f"#!{sys.executable}\n"
                "import sys\n"
                "sys.stdout.buffer.write(b'x' * 10000)\n"
            )
            binary.chmod(0o755)
            (root / "main.tex").write_text("test")
            self.assertEqual(format_provenance(binary)["kind"], "embedded")
            elapsed, rc, capture = timed_compile(
                binary,
                root,
                "main",
                {},
                root / "run.log",
                1024,
            )
            self.assertGreaterEqual(elapsed, 0)
            self.assertEqual(rc, 0)
            self.assertEqual(capture["bytes"], 10000)
            self.assertTrue(capture["truncated"])
            self.assertLessEqual((root / "run.log").stat().st_size, 1024)

    def test_successful_cold_run_keeps_metrics_only(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            parent = Path(tmp)
            root = parent / "inputs"
            fixture = root / "one-rust"
            fixture.mkdir(parents=True)
            (fixture / "main.tex").write_text("test")
            (root / "inputs.json").write_text(json.dumps({"one": {"pages": 1}}))
            seeds = root / ".seeds" / "one"
            seeds.mkdir(parents=True)
            (seeds / "seed-list.json").write_text("[]")
            bindir = parent / "bin"
            bindir.mkdir()
            binary = bindir / "pdflatex"
            binary.write_text(
                f"#!{sys.executable}\n"
                "import pymupdf\n"
                "d=pymupdf.open(); d.new_page(); "
                "d.set_metadata({'producer':'tex-rs'}); "
                "d.save('main.pdf'); d.close()\n"
                "print('x'*10000)\n"
            )
            binary.chmod(0o755)
            old_docs, old_gated = bench_cold.DOCS, bench_cold.GATED
            try:
                bench_cold.DOCS = {"one": {"job": "main"}}
                bench_cold.GATED = ("one",)
                with redirect_stdout(io.StringIO()):
                    bench_cold.run(
                        root, binary, "clean", 1, None, "failures", False, 1024
                    )
            finally:
                bench_cold.DOCS, bench_cold.GATED = old_docs, old_gated
            results = parent / "results" / "clean"
            timing = json.loads((results / "timings.json").read_text())
            self.assertTrue(timing["one"]["a"]["valid"])
            self.assertEqual(
                timing["one"]["a"]["samples"][0]["capture"]["bytes"], 10001
            )
            self.assertFalse(any(results.glob("*.log")))
            self.assertFalse((root / "one-rust__clean").exists())
            formats = json.loads((results / "env.json").read_text())["formats"]
            self.assertEqual(next(iter(formats.values()))["kind"], "embedded")


class RetentionTests(unittest.TestCase):
    def test_rmtree_rechecks_target_identity_immediately_before_delete(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "output"
            target = root / "work" / "safe"
            target.mkdir(parents=True)
            (target / "original").write_text("keep")
            moved = root / "work" / "moved"
            original_validate = test_corpus._validate_cleanup_target
            calls = 0

            def replace_after_first_check(*args, **kwargs):
                nonlocal calls
                checked = original_validate(*args, **kwargs)
                calls += 1
                if calls == 1:
                    target.rename(moved)
                    target.mkdir()
                    (target / "replacement").write_text("keep")
                return checked

            with mock.patch.object(
                test_corpus,
                "_validate_cleanup_target",
                side_effect=replace_after_first_check,
            ):
                with self.assertRaisesRegex(ValueError, "changed during validation"):
                    test_corpus._safe_rmtree(target, root, "work")

            self.assertEqual((moved / "original").read_text(), "keep")
            self.assertEqual((target / "replacement").read_text(), "keep")

    def test_retention_rejects_symlinked_managed_root_before_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            parent = Path(tmp)
            root = parent / "output"
            evidence = root / "results" / "safe" / "keep.log"
            evidence.parent.mkdir(parents=True)
            evidence.write_text("evidence")
            outside = parent / "outside"
            outside.mkdir()
            sentinel = outside / "keep"
            sentinel.write_text("untouched")
            (root / "work").symlink_to(outside, target_is_directory=True)
            result = {
                "id": "safe",
                "mode": "campaign",
                "rust": {},
                "ref": {},
                "compare": {},
            }

            with self.assertRaisesRegex(ValueError, "managed artifact root"):
                apply_artifact_retention(
                    result,
                    root,
                    {"retain": "none", "keep_work": False, "doc_min": 99.0},
                )

            self.assertEqual(evidence.read_text(), "evidence")
            self.assertEqual(sentinel.read_text(), "untouched")

    def test_retention_rejects_symlinked_recursive_target(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            parent = Path(tmp)
            root = parent / "output"
            (root / "work").mkdir(parents=True)
            outside = parent / "outside"
            outside.mkdir()
            sentinel = outside / "keep"
            sentinel.write_text("untouched")
            (root / "work" / "safe").symlink_to(outside, target_is_directory=True)
            result = {
                "id": "safe",
                "mode": "campaign",
                "rust": {},
                "ref": {},
                "compare": {},
            }

            with self.assertRaisesRegex(ValueError, "cleanup target"):
                apply_artifact_retention(
                    result,
                    root,
                    {"retain": "all", "keep_work": False, "doc_min": 99.0},
                )

            self.assertTrue((root / "work" / "safe").is_symlink())
            self.assertEqual(sentinel.read_text(), "untouched")

    def test_workspace_refresh_rejects_symlinked_work_root(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            parent = Path(tmp)
            root = parent / "output"
            root.mkdir()
            source = parent / "source"
            source.mkdir()
            (source / "main.tex").write_text("test")
            outside = parent / "outside"
            outside.mkdir()
            sentinel = outside / "keep"
            sentinel.write_text("untouched")
            (root / "work").symlink_to(outside, target_is_directory=True)

            with self.assertRaisesRegex(ValueError, "managed artifact root"):
                prepare_workspace(
                    source,
                    root / "work" / "safe" / "rust",
                    Path("main.tex"),
                    out_root=root,
                )

            self.assertEqual(sentinel.read_text(), "untouched")

    def test_retention_rejects_unsafe_checkpoint_id_before_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "output"
            root.mkdir()
            sentinel = Path(tmp) / "sentinel"
            sentinel.mkdir()
            (sentinel / "keep").write_text("untouched")
            result = {
                "id": "../../sentinel",
                "mode": "campaign",
                "rust": {},
                "ref": {},
                "compare": {},
            }

            with self.assertRaisesRegex(ValueError, "unsafe manifest id"):
                apply_artifact_retention(
                    result,
                    root,
                    {
                        "retain": "none",
                        "keep_work": False,
                        "doc_min": 99.0,
                    },
                )
            self.assertEqual((sentinel / "keep").read_text(), "untouched")

    def test_manifest_paths_cannot_escape_cleanup_root(self) -> None:
        validate_manifest_entry({"id": "2609.01234", "main_tex": "src/main.tex"})
        for entry in (
            {"id": "..", "main_tex": "main.tex"},
            {"id": "project/../../outside", "main_tex": "main.tex"},
            {"id": "project", "main_tex": "../../outside.tex"},
            {"id": "project", "main_tex": "/outside.tex"},
        ):
            with self.assertRaisesRegex(ValueError, "unsafe"):
                validate_manifest_entry(entry)

    def test_success_keeps_metrics_but_removes_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = campaign_result(root, "pass")
            apply_artifact_retention(
                result,
                root,
                {
                    "retain": "failures",
                    "keep_work": False,
                    "doc_min": 99.0,
                },
            )
            self.assertFalse((root / "results" / "pass").exists())
            self.assertFalse((root / "pdf" / "pass.rust.pdf").exists())
            self.assertFalse((root / "work" / "pass").exists())
            self.assertEqual(result["rust"]["pages"], 1)
            self.assertTrue(result["rust"]["pdf_valid"])
            self.assertIsNone(result["rust"]["pdf"])
            gate = campaign_gate(
                [{"id": "pass"}],
                {"pass": result},
                {"dpi": 150, "page_min": 99.0, "doc_min": 99.0},
            )
            self.assertTrue(gate["ok"], gate)

    def test_invalid_engine_pdf_is_measured_but_never_copied(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            workspace = root / "work" / "invalid" / "rust"
            workspace.mkdir(parents=True)
            (workspace / "main.tex").write_text("test")
            binary = root / "fake-pdflatex"
            payload = b"not a pdf\n" + b"x" * (2 << 20)
            binary.write_text(
                f"#!{sys.executable}\n"
                "from pathlib import Path\n"
                f"Path('main.pdf').write_bytes({payload!r})\n"
            )
            binary.chmod(0o755)
            destination = root / "pdf" / "invalid.rust.pdf"
            destination.parent.mkdir()
            destination.write_bytes(b"stale output from an earlier run")

            result = compile_engine(
                "rust",
                str(binary),
                workspace,
                Path("main.tex"),
                root / "results" / "invalid",
                10,
                mem_limit_mib=0,
            )

            self.assertEqual(result["status"], "failure")
            self.assertTrue(result["pdf_produced"])
            self.assertFalse(result["pdf_valid"])
            self.assertFalse(result["pdf_exists"])
            self.assertIsNone(result["pdf"])
            self.assertEqual(result["pdf_bytes"], len(payload))
            self.assertEqual(result["pdf_sha1"], hashlib.sha1(payload).hexdigest())
            self.assertTrue(result["pdf_error"])
            self.assertFalse(destination.exists())

    def test_retention_drops_invalid_historical_pdf_but_keeps_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = campaign_result(root, "invalid", failed=True)
            invalid = root / "pdf" / "invalid.rust.pdf"
            payload = b"truncated" + b"x" * (2 << 20)
            invalid.write_bytes(payload)
            result["rust"].update(
                {
                    "pdf_valid": False,
                    "pdf_bytes": len(payload),
                    "pdf_sha1": hashlib.sha1(payload).hexdigest(),
                    "pdf_error": "FileDataError: cannot open broken document",
                    "pdf_produced": True,
                }
            )

            apply_artifact_retention(
                result,
                root,
                {
                    "retain": "failures",
                    "keep_work": False,
                    "doc_min": 99.0,
                },
            )

            self.assertFalse(invalid.exists())
            self.assertIsNone(result["rust"]["pdf"])
            self.assertFalse(result["rust"]["pdf_retained"])
            self.assertFalse(result["rust"]["pdf_exists"])
            self.assertTrue(result["rust"]["pdf_produced"])
            self.assertEqual(result["rust"]["pdf_bytes"], len(payload))
            self.assertEqual(
                result["rust"]["pdf_sha1"], hashlib.sha1(payload).hexdigest()
            )
            self.assertIn("FileDataError", result["rust"]["pdf_error"])
            failure = json.loads(
                (root / "results" / "invalid" / "failure.json").read_text()
            )["engines"]["rust"]
            self.assertFalse(failure["pdf_valid"])
            self.assertEqual(failure["pdf_bytes"], len(payload))
            self.assertEqual(failure["pdf_sha1"], hashlib.sha1(payload).hexdigest())
            self.assertIn("FileDataError", failure["pdf_error"])

    def test_failure_bundle_keeps_only_final_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = campaign_result(root, "fail", failed=True)
            apply_artifact_retention(
                result,
                root,
                {
                    "retain": "failures",
                    "keep_work": False,
                    "doc_min": 99.0,
                },
            )
            idir = root / "results" / "fail"
            self.assertTrue((idir / "rust.stdout.log").is_file())
            self.assertTrue((idir / "failure.json").is_file())
            self.assertFalse((idir / "passlogs").exists())
            self.assertTrue((root / "pdf" / "fail.rust.pdf").is_file())
            self.assertFalse((root / "work" / "fail").exists())

    def test_retain_all_and_keep_work_preserve_success_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = campaign_result(root, "all")
            apply_artifact_retention(
                result,
                root,
                {
                    "retain": "all",
                    "keep_work": True,
                    "doc_min": 99.0,
                },
            )
            self.assertTrue((root / "results" / "all" / "rust.stdout.log").is_file())
            self.assertTrue((root / "pdf" / "all.rust.pdf").is_file())
            self.assertTrue((root / "worst" / "all-p1-diff.png").is_file())
            self.assertTrue((root / "work" / "all").is_dir())

    def test_retain_none_removes_failure_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = campaign_result(root, "none", failed=True)
            apply_artifact_retention(
                result,
                root,
                {
                    "retain": "none",
                    "keep_work": False,
                    "doc_min": 99.0,
                },
            )
            self.assertFalse((root / "results" / "none").exists())
            self.assertFalse((root / "pdf" / "none.rust.pdf").exists())
            self.assertFalse((root / "worst" / "none-p1-diff.png").exists())
            self.assertFalse((root / "work" / "none").exists())

    def test_reference_only_embedding_failure_blocks_qualification_and_retains_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            aid = "ref_embed_fail"
            result = campaign_result(root, aid, failed=False)
            ref_font_err = "validator error: unhandled PDF format error"
            result["ref"]["font_embedding_errors"] = [ref_font_err]
            result["ref"]["font_embedding_valid"] = False
            result["ref"]["failure_kind"] = "embedding"
            result["compare"]["font_embedding_failure"] = True
            result["compare"]["font_embedding_errors"] = [ref_font_err]
            result["compare"]["font_embedding_errors_by_engine"] = {
                "rust": [],
                "ref": [ref_font_err],
            }

            # 1. campaign_gate must fail with kind "embedding"
            gate = campaign_gate(
                [{"id": aid}],
                {aid: result},
                {"dpi": 150, "page_min": 99.0, "doc_min": 99.0},
            )
            self.assertFalse(gate["ok"])
            embed_failures = [f for f in gate["failures"] if f.get("kind") == "embedding"]
            self.assertEqual(len(embed_failures), 1)
            self.assertEqual(embed_failures[0]["id"], aid)

            # 2. qualification_ledger must disqualify the project
            ledger = qualification_ledger([{"id": aid}], {aid: result}, 99.0)
            self.assertEqual(ledger["qualified_count"], 0)
            self.assertNotIn(aid, ledger["qualified_ids"])
            self.assertFalse(ledger["projects"][0]["qualified"])
            self.assertIn("ref-font-embedding", ledger["projects"][0]["reasons"])

            # 3. apply_artifact_retention must keep evidence under retain="failures"
            apply_artifact_retention(
                result,
                root,
                {
                    "retain": "failures",
                    "keep_work": False,
                    "doc_min": 99.0,
                },
            )
            self.assertTrue(result["retention"]["failure"])
            self.assertTrue(result["retention"]["evidence_retained"])
            self.assertTrue((root / "results" / aid / "failure.json").is_file())
            self.assertTrue((root / "pdf" / f"{aid}.ref.pdf").is_file())
            self.assertTrue((root / "pdf" / f"{aid}.rust.pdf").is_file())

    def test_missing_outline_program_classifies_as_missing_asset(self) -> None:
        run = {"exit": 1, "timed_out": False, "mem_killed": False}
        pdf_stat = {"pdf_valid": False}
        err = ["Font `\\test` (TFM `test`) has no associated outline program"]

        # Missing outline must classify as missing-asset, not compilation or unsupported-engine
        self.assertEqual(classify_failure_kind(run, pdf_stat, err), "missing-asset")

        # Preamble mentions of font packages must not disguise the missing asset as unsupported-engine
        log_with_fontspec = "\\usepackage{fontspec}\nFont `cmr10` (TFM `cmr10`) has no associated outline program"
        self.assertEqual(
            classify_failure_kind(run, pdf_stat, err, log_text=log_with_fontspec),
            "missing-asset",
        )

        # Genuine engine requirement error remains unsupported-engine
        unsupported = ["Package fontspec Error: The fontspec package requires either XeTeX or LuaTeX"]
        self.assertEqual(
            classify_failure_kind(run, pdf_stat, unsupported),
            "unsupported-engine",
        )

        # Generic syntax error remains compilation
        syntax_err = ["! Undefined control sequence: \\xyz"]
        self.assertEqual(
            classify_failure_kind(run, pdf_stat, syntax_err),
            "compilation",
        )


class PruneTests(unittest.TestCase):
    def _fixture(self, parent: Path) -> Path:
        root = parent / "old-run"
        passed = campaign_result(root, "pass")
        failed = campaign_result(root, "fail", failed=True)
        report = {
            "meta": {"mode": "campaign", "doc_min_pct": 99.0, "output": str(root)},
            "results": {"pass": passed, "fail": failed},
        }
        (root / "report.json").write_text(json.dumps(report))
        (root / "checkpoints").mkdir()
        (root / "checkpoints" / "pass.json").write_text("{}")
        return root

    def test_plan_and_apply_preserve_reports_and_failure_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            parent = Path(tmp)
            root = self._fixture(parent)
            plan_path = parent / "plan.json"
            plan = make_plan(root, plan_path, 1 << 20)
            action_paths = {item["path"] for item in plan["actions"]}
            self.assertIn("pdf/pass.rust.pdf", action_paths)
            self.assertNotIn("pdf/fail.rust.pdf", action_paths)
            summary = apply_plan(plan_path)
            self.assertGreater(summary["removed_files"], 0)
            self.assertTrue((root / "report.json").is_file())
            self.assertTrue((root / "checkpoints" / "pass.json").is_file())
            self.assertTrue((root / "pdf" / "fail.rust.pdf").is_file())
            self.assertFalse((root / "pdf" / "pass.rust.pdf").exists())

    def test_plan_deletes_reported_invalid_failure_pdf(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            parent = Path(tmp)
            root = self._fixture(parent)
            report_path = root / "report.json"
            report = json.loads(report_path.read_text())
            state = report["results"]["fail"]["rust"]
            invalid = root / "pdf" / "fail.rust.pdf"
            payload = b"broken pdf" + b"x" * (2 << 20)
            invalid.write_bytes(payload)
            state.update(
                {
                    "pdf_valid": False,
                    "pdf_bytes": len(payload),
                    "pdf_sha1": hashlib.sha1(payload).hexdigest(),
                    "pdf_error": "invalid PDF",
                }
            )
            report_path.write_text(json.dumps(report))

            plan_path = parent / "invalid-plan.json"
            plan = make_plan(root, plan_path, 1 << 20)
            action = next(
                item
                for item in plan["actions"]
                if item["path"] == "pdf/fail.rust.pdf"
            )
            self.assertEqual(action["action"], "delete")
            self.assertEqual(action["reason"], "duplicate-failure-evidence")
            self.assertEqual(action["bytes"], len(payload))

            apply_plan(plan_path)
            self.assertFalse(invalid.exists())
            self.assertEqual(state["pdf_bytes"], len(payload))
            self.assertEqual(state["pdf_sha1"], hashlib.sha1(payload).hexdigest())

    def test_apply_revalidates_all_files_before_deleting(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            parent = Path(tmp)
            root = self._fixture(parent)
            plan_path = parent / "plan.json"
            plan = make_plan(root, plan_path, 1 << 20)
            targets = [root / item["path"] for item in plan["actions"]]
            targets[-1].write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "changed since review"):
                apply_plan(plan_path)
            self.assertTrue(targets[0].exists())

    def test_pruner_refuses_trust_own_tree(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            parent = Path(tmp) / "trust_own"
            parent.mkdir()
            root = self._fixture(parent)
            with self.assertRaisesRegex(ValueError, "trust_own"):
                make_plan(root, Path(tmp) / "plan.json", 1024)

    def test_historical_campaign_report_keeps_only_last_failure_logs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            parent = Path(tmp)
            root = self._fixture(parent)
            report_path = root / "report.json"
            report = json.loads(report_path.read_text())
            old = report["results"]["fail"]
            old.pop("mode", None)
            for engine in ("rust", "ref"):
                state = old[engine]
                plog = root / "results" / "fail" / "passlogs"
                last = plog / f"{engine}-pass2.stdout.log"
                last.write_bytes(b"last")
                state["captured_log"] = str(last)
                state["pass_records"].append(
                    {
                        "stdout_log": str(last),
                        "tex_log": None,
                    }
                )
            report_path.write_text(json.dumps(report))
            plan = make_plan(root, parent / "historical-plan.json", 1 << 20)
            actions = {item["path"] for item in plan["actions"]}
            self.assertIn("results/fail/passlogs/rust-pass1.stdout.log", actions)
            self.assertNotIn("results/fail/passlogs/rust-pass2.stdout.log", actions)

    def test_oversized_historical_failure_log_is_hashed_and_compacted(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            parent = Path(tmp)
            root = self._fixture(parent)
            log = root / "results" / "fail" / "rust.stdout.log"
            original = b"head\n" + b"x" * 300000 + b"\ntail\n"
            log.write_bytes(original)
            plan_path = parent / "compact-plan.json"
            plan = make_plan(root, plan_path, 200000)
            action = next(
                item
                for item in plan["actions"]
                if item["path"] == "results/fail/rust.stdout.log"
            )
            self.assertEqual(action["action"], "compact")
            self.assertEqual(action["sha256"], hashlib.sha256(original).hexdigest())
            apply_plan(plan_path)
            self.assertLessEqual(log.stat().st_size, 200000)
            self.assertIn(b"output bytes omitted", log.read_bytes())

    def test_planner_never_compacts_a_log_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            parent = Path(tmp)
            root = self._fixture(parent)
            link = root / "results" / "fail" / "rust.stdout.log"
            target = parent / "outside.log"
            target.write_bytes(b"x" * 300000)
            link.unlink()
            link.symlink_to(target)

            plan = make_plan(root, parent / "symlink-plan.json", 200000)
            action = next(
                item
                for item in plan["actions"]
                if item["path"] == "results/fail/rust.stdout.log"
            )
            self.assertEqual(action["action"], "delete")
            self.assertNotIn("sha256", action)


if __name__ == "__main__":
    unittest.main()
