#!/usr/bin/env python3
"""Regressions for interpreting reported engines and non-compilation issues."""

import unittest

from issue_bot import prepare_context

TEX = "\\documentclass{article}\n\\begin{document}Example\\end{document}\n"


def report(body, title="[Bug]: compilation failure", comment=None, labels=None):
    event = {"issue": {"title": title, "body": body, "labels": labels or [], "user": {"login": "reporter"}}}
    if comment is not None:
        event["comment"] = {"body": comment}
    return prepare_context(event)


class ReportInterpretationTests(unittest.TestCase):
    def test_issue_19_uses_failing_command_not_working_engine_mentions(self):
        context = report(f"```latex\n{TEX}```\nratex -lualatex test_document_minimal.tex\nError: unexpected symbol near '#'.\n(with -pdf or -xelatex it works just fine!)")
        self.assertEqual(context["engines"], ["-lualatex"])
        self.assertEqual(context["snippet"], TEX)

    def test_multiple_explicit_commands_are_tested_separately(self):
        context = report(f"```tex\n{TEX}```\nWorking: ratex -pdf main.tex\nFailing: ratex -lualatex main.tex")
        self.assertEqual(context["engines"], ["-pdf", "-lualatex"])

    def test_reproduce_comment_reuses_issue_document_and_command(self):
        context = report(f"```latex\n{TEX}```\nratex -lualatex main.tex", comment="/reproduce")
        self.assertEqual((context["snippet"], context["engines"]), (TEX, ["-lualatex"]))

    def test_comment_explicitly_changes_the_test_engine(self):
        context = report(f"```latex\n{TEX}```\nratex -lualatex main.tex", comment="/test -xelatex")
        self.assertEqual(context["engines"], ["-xelatex"])

    def test_updated_comment_document_replaces_issue_document(self):
        updated = "\\documentclass{article}\n\\begin{document}Updated\\end{document}\n"
        context = report(f"```latex\n{TEX}```\nratex -lualatex main.tex", comment=f"/reproduce\n```latex\n{updated}```")
        self.assertEqual((context["snippet"], context["engines"]), (updated, ["-lualatex"]))

    def test_shell_and_log_fences_are_not_selected_as_tex(self):
        context = report(f"```sh\nratex -lualatex main.tex\n```\n```text\nLuaTeX error: unexpected symbol near '#'\n```\n```latex\n{TEX}```")
        self.assertEqual((context["snippet"], context["engines"]), (TEX, ["-lualatex"]))

    def test_naming_suggestion_does_not_request_a_tex_reproduction(self):
        context = report("A different project already uses this name. Consider renaming the project.", title="Suggestion: Consider renaming your project")
        self.assertEqual((context["action"], context["engines"]), ("review_request", []))

    def test_bug_without_a_document_requests_the_missing_snippet(self):
        context = report("ratex -lualatex main.tex fails", title="Compilation fails", labels=[{"name": "bug"}])
        self.assertEqual((context["action"], context["engines"]), ("request_snippet", []))

    def test_untrusted_command_options_cannot_become_execution_arguments(self):
        context = report(f"```latex\n{TEX}```\nratex -lualatex --shell-escape '../main.tex'; touch /tmp/marker")
        self.assertEqual(context["engines"], ["-lualatex"])

    def test_engine_alias_and_quoted_filename_are_understood(self):
        context = report(f"```latex\n{TEX}```\nxelatex 'document with spaces.tex'")
        self.assertEqual(context["engines"], ["-xelatex"])

    def test_engine_mentions_without_a_command_are_not_cherry_picked(self):
        context = report(f"```latex\n{TEX}```\n-pdf works, -lualatex fails")
        self.assertEqual(context["engines"], ["-pdf", "-lualatex"])


if __name__ == "__main__":
    unittest.main()
