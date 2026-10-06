#!/usr/bin/env python3
"""Write the inputs of the tool-perl-script-run-exact Biber fixture.

Each case runs one Perl script-run pattern ((*sr:...)/(*asr:...)) as a tool-mode
sourcemap match on a `note` field and replaces it with the whole match and the
first three captures. The cases target the regex VM paths where script runs
meet subroutine calls, recursion, lookaround, backtracking and capture
restoration. Afterwards record the oracle output with:

  python3 scripts/test_biber.py --regen --case tool-perl-script-run-exact
"""
import html
import json
from pathlib import Path

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "biber" / "tool-perl-script-run-exact"

# (name, pattern, inputs)
CASES = [
    # A script run started in a called group replaces the caller's run start.
    ("call-define", r"^(?(DEFINE)(?<x>(*sr:α)))(*sr:a(?&x))$", ["aα", "aa", "αα"]),
    ("call-define-tail", r"^(?(DEFINE)(?<x>(*sr:α)))(*sr:a(?&x)b)$", ["aαb", "aαα", "aα"]),
    ("call-numbered", r"^(*sr:a(?1))((*sr:α+))?$", ["aα", "aαα", "aααβ"]),
    ("call-atomic", r"^(?(DEFINE)(?<x>(*asr:α+)))(*asr:a(?&x))$", ["aα", "aαα", "a"]),
    ("call-sr-in-atomic", r"^(?(DEFINE)(?<x>(*sr:α)))(*sr:a(?>(?&x)))b$", ["aαb", "aαα"]),
    # Backtracking past or into a script run.
    ("backtrack-call", r"^(?(DEFINE)(?<x>(*sr:α)))(*sr:a(?:(?&x)z|αα))$", ["aαα", "aαz", "aαb"]),
    ("backtrack-call-b", r"^(?(DEFINE)(?<x>(*sr:α)))(*sr:a(?:(?&x)z|αb))$", ["aαb", "aαα"]),
    ("backtrack-into", r"^(*sr:(a+)(.+))$", ["aaα", "aab", "aaα1"]),
    ("backtrack-into-atomic", r"^(*asr:(a+)(.+))$", ["aaα", "aab"]),
    ("backtrack-greedy", r"(*sr:.+)", ["abαβ", "αβab", "a1α"]),
    ("backtrack-lazy", r"(*sr:.+?)$", ["abαβ", "αβab"]),
    ("backtrack-alt", r"^(?:(*sr:a.)z|(*sr:.α))", ["aαz", "aα", "bα"]),
    ("backtrack-quant", r"^(?:(*sr:..))+$", ["aαbβ", "abαβ", "abab"]),
    # Lookaround inside a script run.
    ("lookahead-run", r"^(*sr:a(?=(*sr:α))α)$", ["aα", "aαα"]),
    ("lookahead-call", r"^(?(DEFINE)(?<x>(*sr:α)))(*sr:a(?=(?&x))α)$", ["aα"]),
    ("neg-lookahead-call", r"^(?(DEFINE)(?<x>(*sr:β)))(*sr:a(?!(?&x))α)$", ["aα", "aβ"]),
    # Recursion into a group containing the script run.
    ("recurse", r"^((*sr:a(?1)?α))$", ["aα", "aaαα", "aαaα"]),
    ("recurse-whole", r"^(*sr:a(?R)?α)$", ["aα", "aaαα"]),
    ("recurse-tail", r"^((*sr:[aα]))(?1)+$", ["aα", "aaα", "αα"]),
    # Capture restoration after failed calls and recursion.
    ("capture-failed-call", r"^(?:(?&x)z|(a)α)(?<x>(a))?", ["aα", "az", "aαa"]),
    ("capture-call-then-fail", r"^(?(DEFINE)(?<x>(a)(*sr:.)))(?:(?&x)z|(?&x))$", ["aα", "aαz", "ab"]),
    ("capture-sr-call", r"^(?:(?2)z|(a))((*sr:a.))?$", ["aα", "aαz", "aaα"]),
    ("capture-recursion", r"^(a|b(?1))(c)?$", ["bba", "bac", "a"]),
    ("capture-sr-recursion", r"^((*sr:a)|b(?1))(.)?$", ["bba", "baα", "a"]),
    ("capture-sr-fail-restore", r"^(?:((*sr:a.))z|(a)(.))$", ["aα", "aαz", "ab"]),
    ("capture-call-in-sr", r"^(*sr:(a)(?1)(.))$", ["aaα", "aab", "aa1"]),
]


def main():
    maps, bib = [], []
    for name, pattern, inputs in CASES:
        for index, value in enumerate(inputs):
            key = f"{name}-{index}"
            maps.append(f'<map><map_step map_field_source="entrykey" map_match="^{key}$" map_final="1"/>'
                        f'<map_step map_field_source="note" map_match="{html.escape(pattern, quote=True)}" '
                        f'map_replace="&lt;$&amp;|$1|$2|$3&gt;"/></map>')
            bib.append(f"@misc{{{key}, note={{{value}}}}}")
    FIXTURE.mkdir(parents=True, exist_ok=True)
    (FIXTURE / "tool.conf").write_text(
        '<?xml version="1.0" encoding="UTF-8"?>\n<config><sourcemap><maps datatype="bibtex" map_overwrite="1">\n'
        + "\n".join(maps) + "\n</maps></sourcemap></config>\n", encoding="utf-8")
    (FIXTURE / "main.bib").write_text("\n".join(bib) + "\n", encoding="utf-8")
    (FIXTURE / "options.json").write_text(
        json.dumps({"tool": True, "configfile": "tool.conf", "output_format": "bibtex"}) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
