#!/usr/bin/env python3
"""Time TeXres against TeX Live on the public benchmark corpus.

Reruns everything and writes scripts/bench/results/bench_tl.json and
bench_tl.md (override with --out-dir). See PERFORMANCE.md, "TeXres vs TeX Live".

    python3 scripts/bench_tl.py --texres target/release/texres
    python3 scripts/bench_tl.py --texres ... --runs 5 --docs article_math,beamer_deck

Corpus: scripts/bench/corpus/<name>/ (regenerate with scripts/bench/gen_corpus.py).
Each document is first built once with both tools ("verification build", also a
warm-up for the system-side caches such as the luaotfload font database that TeX
Live keeps in $HOME). The PDFs must have the same page count and the same
`pdftotext -layout` text after deleting all whitespace. Documents that differ are
reported and excluded from timing.

Scenarios (every timed run starts from an identical restored state; the two
tools alternate, and the order flips every run):
  a  cold build: TeXres with an empty cache root vs latexmk in a clean directory
     (all passes, plus BibTeX or Biber where the document needs it)
  b  no-change rebuild of a converged build (same command again)
  c  one-line-edit rebuild (BENCH-A -> BENCH-B in the middle of main.tex)
  d  one engine pass (TeXres `pdflatex` personality vs /usr/bin/pdflatex, and the
     xelatex / lualatex counterparts) over auxiliary files left by a converged
     TeX Live build; TeXres gets an empty cache root each time

Only /usr/bin TeX Live tools are used (PATH is fixed); TeXres is the binary given
with --texres. Wall time is measured around the whole command; CPU time is
user+system of the process tree (wait4 rusage).
"""

import argparse
import datetime
import hashlib
import json
import os
import platform
import re
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
CORPUS = HERE / "bench" / "corpus"
TL_BIN = Path("/usr/bin")
CLEAN_PATH = "/usr/bin:/usr/bin/vendor_perl:/bin"

# name -> (engine flag for texres/latexmk, engine executable name)
DOCS = {
    "article_math": ("-pdf", "pdflatex"),
    "article_biblatex": ("-pdf", "pdflatex"),
    "article_natbib": ("-pdf", "pdflatex"),
    "beamer_deck": ("-pdf", "pdflatex"),
    "tikz_pgfplots": ("-pdf", "pdflatex"),
    "xelatex_fontspec": ("-xelatex", "xelatex"),
    "lualatex_fontspec": ("-lualatex", "lualatex"),
    "long_thesis": ("-pdf", "pdflatex"),
    "toc_wrap_canary": ("-pdf", "pdflatex"),
}
SCENARIOS = {
    "a": "cold build",
    "b": "no-change rebuild",
    "c": "one-line-edit rebuild",
    "d": "single engine pass",
}
# products removed from the converged TeX Live directory before scenario d
PRODUCT_RE = re.compile(r"\.(pdf|log|fdb_latexmk|fls|synctex\.gz|blg)$")
MAIN = "main.tex"
TIMEOUT = 900


def log(*a):
    print(*a, flush=True)


# ----------------------------------------------------------------- running


class Failed(Exception):
    pass


def make_env():
    env = {
        k: v
        for k, v in os.environ.items()
        if not (k.startswith("TEX") or k in ("PHASE_TIMING", "PERL5LIB", "LUA_PATH"))
    }
    env.update(
        PATH=CLEAN_PATH,
        TZ="UTC",
        LC_ALL="C.UTF-8",
        SOURCE_DATE_EPOCH="1700000000",
        FORCE_SOURCE_DATE="1",
    )
    return env


def timed(cmd, cwd, env, timeout=TIMEOUT):
    """Run cmd; return (wall_s, cpu_s, returncode, output_tail)."""
    with tempfile.TemporaryFile() as out:
        t0 = time.perf_counter()
        p = subprocess.Popen(
            cmd,
            cwd=cwd,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=out,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        timer = threading.Timer(timeout, lambda: os.killpg(p.pid, signal.SIGKILL))
        timer.start()
        _, status, ru = os.wait4(p.pid, 0)
        wall = time.perf_counter() - t0
        timer.cancel()
        p.returncode = os.waitstatus_to_exitcode(status)
        out.seek(0)
        tail = out.read()[-3000:].decode("utf-8", "replace")
    return wall, ru.ru_utime + ru.ru_stime, p.returncode, tail


def sh(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


def restore(src: Path, dst: Path):
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(src, dst, copy_function=shutil.copy2, symlinks=True)


def fresh_copy(src: Path, dst: Path):
    restore(src, dst)


# ---------------------------------------------------------------- measuring


class Tool:
    """One side of the comparison for one document."""

    def __init__(self, kind, doc, root, texres, bindir):
        self.kind = kind  # "texres" or "tl"
        self.doc = doc
        self.flag, self.engine = DOCS[doc]
        self.base = root / doc / kind
        self.work = self.base / "work"
        self.cache = self.base / "cache"
        self.snap_work = self.base / "snap_work"
        self.snap_cache = self.base / "snap_cache"
        self.texres = texres
        self.bindir = bindir
        self.env = make_env()
        if kind == "texres":
            self.env["TEX_RS_CACHE_DIR"] = str(self.cache)

    # full builds (scenarios a, b, c)
    def build_cmd(self):
        if self.kind == "texres":
            return [str(self.texres), self.flag, MAIN]
        return [str(TL_BIN / "latexmk"), self.flag, "-interaction=nonstopmode", MAIN]

    # one engine pass (scenario d)
    def pass_cmd(self):
        exe = (self.bindir if self.kind == "texres" else TL_BIN) / self.engine
        return [str(exe), "-interaction=nonstopmode", "-halt-on-error", MAIN]

    def fresh_cache(self):
        if self.kind == "texres":
            if self.cache.exists():
                shutil.rmtree(self.cache)
            self.cache.mkdir(parents=True)

    def run(self, cmd):
        wall, cpu, rc, tail = timed(cmd, self.work, self.env)
        if rc != 0:
            raise Failed(f"{self.kind} {self.doc}: {' '.join(cmd)} exited {rc}\n{tail}")
        return wall, cpu

    def pdf(self):
        return self.work / "main.pdf"


def pdf_summary(pdf: Path):
    info = sh(["pdfinfo", str(pdf)]).stdout
    pages = int(re.search(r"^Pages:\s+(\d+)", info, re.M).group(1))
    text = sh(["pdftotext", "-layout", str(pdf), "-"]).stdout
    return pages, text


def compare_pdfs(tl_pdf, rs_pdf):
    p1, t1 = pdf_summary(tl_pdf)
    p2, t2 = pdf_summary(rs_pdf)
    res = {"pages_tl": p1, "pages_texres": p2}
    n1, n2 = re.sub(r"\s+", "", t1), re.sub(r"\s+", "", t2)
    res["text_equal"] = n1 == n2
    res["pages_equal"] = p1 == p2
    if n1 != n2:
        # first differing page, and a short excerpt of each side
        diff_pages = []
        for p in range(1, min(p1, p2) + 1):
            a = re.sub(r"\s+", "", sh(["pdftotext", "-layout", "-f", str(p), "-l", str(p), str(tl_pdf), "-"]).stdout)
            b = re.sub(r"\s+", "", sh(["pdftotext", "-layout", "-f", str(p), "-l", str(p), str(rs_pdf), "-"]).stdout)
            if a != b:
                diff_pages.append(p)
        res["differing_pages"] = diff_pages
        i = next((k for k in range(min(len(n1), len(n2))) if n1[k] != n2[k]), min(len(n1), len(n2)))
        res["first_difference"] = {"tl": n1[max(0, i - 30) : i + 40], "texres": n2[max(0, i - 30) : i + 40]}
    return res


def summarize(xs):
    return {
        "median": statistics.median(xs),
        "min": min(xs),
        "max": max(xs),
        "n": len(xs),
    }


def measure(doc, tools, scenario, runs, prep_d):
    """Return {tool kind: {"wall": [...], "cpu": [...]}} with alternating order."""
    out = {k: {"wall": [], "cpu": []} for k in tools}
    kinds = list(tools)
    for i in range(runs):
        order = kinds if i % 2 == 0 else kinds[::-1]
        for kind in order:
            t = tools[kind]
            if scenario == "a":
                fresh_copy(CORPUS / doc, t.work)
                t.fresh_cache()
                cmd = t.build_cmd()
            elif scenario in ("b", "c"):
                restore(t.snap_work, t.work)
                if kind == "texres":
                    restore(t.snap_cache, t.cache)
                if scenario == "c":
                    main = t.work / MAIN
                    src = main.read_text(encoding="utf-8")
                    assert src.count("BENCH-A") == 1, f"{doc}: marker missing"
                    main.write_text(src.replace("BENCH-A", "BENCH-B"), encoding="utf-8")
                cmd = t.build_cmd()
            else:
                restore(prep_d, t.work)
                t.fresh_cache()
                cmd = t.pass_cmd()
            wall, cpu = t.run(cmd)
            if scenario == "c" and i == 0:
                _, text = pdf_summary(t.pdf())
                if "BENCH-B" not in text:
                    raise Failed(f"{kind} {doc}: edit rebuild did not pick up the edit")
            out[kind]["wall"].append(wall)
            out[kind]["cpu"].append(cpu)
            log(f"    [{scenario}] run {i + 1}/{runs} {kind:6s} wall {wall:7.3f}s cpu {cpu:7.3f}s")
    return out


# --------------------------------------------------------------- environment


def first_line(cmd):
    r = sh(cmd, env=make_env())
    return ((r.stdout or r.stderr).strip().splitlines() or ["?"])[0]


def environment(texres: Path):
    cpuinfo = Path("/proc/cpuinfo").read_text()
    model = re.search(r"^model name\s*:\s*(.+)$", cpuinfo, re.M).group(1)
    lscpu = sh(["lscpu"]).stdout
    def lf(k):
        m = re.search(rf"^{k}:\s+(.+)$", lscpu, re.M)
        return m.group(1).strip() if m else "?"
    mem_kb = int(re.search(r"MemTotal:\s+(\d+)", Path("/proc/meminfo").read_text()).group(1))
    osr = dict(
        l.split("=", 1) for l in Path("/etc/os-release").read_text().splitlines() if "=" in l
    )
    gov = Path("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
    return {
        "date": datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
        "cpu_model": model,
        "logical_cpus": os.cpu_count(),
        "cores_per_socket": lf(r"Core\(s\) per socket"),
        "threads_per_core": lf(r"Thread\(s\) per core"),
        "sockets": lf(r"Socket\(s\)"),
        "ram_gib": round(mem_kb / 1048576, 1),
        "os": osr.get("PRETTY_NAME", "").strip('"'),
        "kernel": platform.release(),
        "cpu_governor": gov.read_text().strip() if gov.exists() else "unknown",
        "texres_version": first_line([str(texres), "--version"]),
        "texres_sha256": hashlib.sha256(texres.read_bytes()).hexdigest(),
        "pdflatex": first_line([str(TL_BIN / "pdflatex"), "--version"]),
        "xelatex": first_line([str(TL_BIN / "xelatex"), "--version"]),
        "lualatex": first_line([str(TL_BIN / "lualatex"), "--version"]),
        "latexmk": first_line([str(TL_BIN / "latexmk"), "--version"]),
        "bibtex": first_line([str(TL_BIN / "bibtex"), "--version"]),
        "biber": first_line(["biber", "--version"]),
        "pdftotext": first_line(["pdftotext", "-v"]),
        "python": platform.python_version(),
        "loadavg_at_start": os.getloadavg(),
    }


# ------------------------------------------------------------------ output


def fmt_t(s):
    if s < 1:
        return f"{s:.3f}"
    return f"{s:.2f}" if s < 100 else f"{s:.0f}"


def cell(d):
    return f"{fmt_t(d['median'])} ({fmt_t(d['min'])}–{fmt_t(d['max'])})"


def ratio_cell(a, b):
    """'TeXres s / TeX Live s' (medians) with a plain-words ratio."""
    r = b / a
    return f"{fmt_t(a)} / {fmt_t(b)} ({r:.2f}× faster)" if r >= 1 else f"{fmt_t(a)} / {fmt_t(b)} ({1 / r:.2f}× slower)"


def render_summary(res):
    ok = [d for d, v in res["documents"].items() if v["verify"]["equivalent"]]
    sc = [s for s in SCENARIOS if s in res["scenarios_run"]]
    head = "| Document | Pages | " + " | ".join(f"({s}) {SCENARIOS[s]}" for s in sc) + " |"
    L = [head, "| --- | ---: | " + " | ".join("---:" for _ in sc) + " |"]
    for d in ok:
        v = res["documents"][d]
        cells = []
        for s in sc:
            t = v["timings"][s]
            cells.append(ratio_cell(t["texres"]["wall"]["median"], t["tl"]["wall"]["median"]))
        L.append(f"| {d} | {v['verify']['pages_tl']} | " + " | ".join(cells) + " |")
    return "\n".join(L) + "\n"


def render_md(res):
    env = res["environment"]
    L = []
    L.append("# TeXres vs TeX Live: benchmark results\n")
    L.append(f"Run on {env['date']}; {res['runs']} timed runs per cell, tools alternating.\n")
    L.append("## Environment\n")
    L.append("| | |\n| --- | --- |")
    L.append(f"| CPU | {env['cpu_model']} ({env['sockets']} socket, {env['cores_per_socket']} cores, {env['threads_per_core']} threads/core, {env['logical_cpus']} logical) |")
    L.append(f"| RAM | {env['ram_gib']} GiB |")
    L.append(f"| OS | {env['os']}, kernel {env['kernel']}; CPU governor `{env['cpu_governor']}` |")
    L.append(f"| TeXres | {env['texres_version']} (sha256 `{env['texres_sha256'][:16]}…`) |")
    L.append(f"| TeX Live | {env['pdflatex']}; {env['xelatex']}; {env['lualatex']}; {env['latexmk']}; {env['biber']} |")
    L.append("")
    ok = [d for d, v in res["documents"].items() if v["verify"]["equivalent"]]
    bad = [d for d in res["documents"] if d not in ok]
    L.append("## Equivalence of outputs\n")
    L.append("| Document | Engine | Pages (TeXres / TL) | `pdftotext -layout` text | Status |")
    L.append("| --- | --- | ---: | --- | --- |")
    for d, v in res["documents"].items():
        vf = v["verify"]
        text = "equal" if vf["text_equal"] else f"differs on page(s) {', '.join(map(str, vf.get('differing_pages', [])))}"
        L.append(f"| {d} | {v['engine']} | {vf['pages_texres']} / {vf['pages_tl']} | {text} | {'timed' if vf['equivalent'] else '**excluded**'} |")
    L.append("")
    for d in bad:
        vf = res["documents"][d]["verify"]
        L.append(f"Excluded `{d}`: " + ("page counts differ; " if not vf["pages_equal"] else "")
                 + (f"first text difference, TL `…{vf['first_difference']['tl']}…` vs TeXres `…{vf['first_difference']['texres']}…`" if "first_difference" in vf else ""))
        L.append("")
    L.append("## Summary: median wall time in seconds, TeXres / TeX Live\n")
    L.append(render_summary(res))
    for sc, title in SCENARIOS.items():
        if sc not in res["scenarios_run"]:
            continue
        L.append(f"## ({sc}) {title}: wall time, seconds, median (min–max)\n")
        L.append("| Document | Pages | TeXres | TeX Live | TeX Live ÷ TeXres | CPU s: TeXres | CPU s: TeX Live |")
        L.append("| --- | ---: | ---: | ---: | ---: | ---: | ---: |")
        for d in ok:
            r = res["documents"][d].get("timings", {}).get(sc)
            if not r:
                continue
            a, b = r["texres"]["wall"], r["tl"]["wall"]
            ca, cb = r["texres"]["cpu"], r["tl"]["cpu"]
            L.append(
                f"| {d} | {res['documents'][d]['verify']['pages_tl']} | {cell(a)} | {cell(b)} | {b['median'] / a['median']:.2f}× | {fmt_t(ca['median'])} | {fmt_t(cb['median'])} |"
            )
        L.append("")
    L.append("`TeX Live ÷ TeXres` above 1 means TeXres was faster; below 1 means TeXres was slower.\n")
    return "\n".join(L) + "\n"


# -------------------------------------------------------------------- main


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--texres", help="path to the texres executable")
    ap.add_argument("--from-json", help="only re-render the markdown next to this results JSON")
    ap.add_argument("--runs", type=int, default=7, help="timed runs per cell (default 7, minimum 5)")
    ap.add_argument("--docs", default=",".join(DOCS), help="comma-separated document names")
    ap.add_argument("--scenarios", default="abcd")
    ap.add_argument("--out-dir", default=str(HERE / "bench" / "results"))
    ap.add_argument("--workdir", default=None, help="scratch directory (default: a new temp dir)")
    ap.add_argument("--keep", action="store_true", help="keep the scratch directory")
    args = ap.parse_args()
    if args.from_json:
        jp = Path(args.from_json)
        jp.with_suffix(".md").write_text(render_md(json.loads(jp.read_text())))
        return 0
    if not args.texres:
        ap.error("--texres is required")
    if args.runs < 5:
        ap.error("--runs must be at least 5")
    texres = Path(args.texres).resolve()
    if not texres.is_file():
        ap.error(f"{texres} not found")
    docs = args.docs.split(",")
    for d in docs:
        if d not in DOCS:
            ap.error(f"unknown document {d}")
    for tool in ("pdflatex", "xelatex", "lualatex", "latexmk", "bibtex"):
        if not (TL_BIN / tool).exists():
            ap.error(f"/usr/bin/{tool} missing")
    if shutil.which("biber", path=CLEAN_PATH) is None:
        ap.error("biber not found on " + CLEAN_PATH)

    root = Path(args.workdir or tempfile.mkdtemp(prefix="texres-bench-")).resolve()
    root.mkdir(parents=True, exist_ok=True)
    bindir = root / "bin"
    bindir.mkdir(exist_ok=True)
    for name in ("pdflatex", "xelatex", "lualatex"):
        link = bindir / name
        if link.is_symlink() or link.exists():
            link.unlink()
        link.symlink_to(texres)

    res = {
        "environment": environment(texres),
        "runs": args.runs,
        "scenarios_run": list(args.scenarios),
        "documents": {},
    }
    log(f"scratch: {root}")
    for doc in docs:
        log(f"== {doc}")
        entry = res["documents"][doc] = {"engine": DOCS[doc][1]}
        tools = {k: Tool(k, doc, root, texres, bindir) for k in ("texres", "tl")}
        # verification build (also warms system-side caches); becomes the snapshot
        for k, t in tools.items():
            fresh_copy(CORPUS / doc, t.work)
            t.fresh_cache()
            wall, cpu = t.run(t.build_cmd())
            log(f"  verification build {k}: {wall:.2f}s")
            restore(t.work, t.snap_work)
            if k == "texres":
                restore(t.cache, t.snap_cache)
        vf = compare_pdfs(tools["tl"].pdf(), tools["texres"].pdf())
        vf["equivalent"] = vf["pages_equal"] and vf["text_equal"]
        entry["verify"] = vf
        log(f"  equivalent: {vf['equivalent']} (pages TL {vf['pages_tl']}, TeXres {vf['pages_texres']})")
        if not vf["equivalent"]:
            log(f"  EXCLUDED: {json.dumps(vf.get('first_difference'))}")
            continue
        # prepared auxiliary state for scenario d: converged TeX Live directory
        prep_d = root / doc / "prep_d"
        restore(tools["tl"].snap_work, prep_d)
        for f in list(prep_d.iterdir()):
            if PRODUCT_RE.search(f.name):
                f.unlink()
        entry["timings"] = {}
        for sc in args.scenarios:
            log(f"  scenario {sc}: {SCENARIOS[sc]} (loadavg {os.getloadavg()[0]:.1f})")
            raw = measure(doc, tools, sc, args.runs, prep_d)
            entry["timings"][sc] = {
                k: {"wall": summarize(v["wall"]), "cpu": summarize(v["cpu"]), "wall_runs": v["wall"], "cpu_runs": v["cpu"]}
                for k, v in raw.items()
            }
            entry["timings"][sc]["loadavg_1m"] = os.getloadavg()[0]
    res["environment"]["loadavg_at_end"] = os.getloadavg()

    out = Path(args.out_dir)
    out.mkdir(parents=True, exist_ok=True)
    (out / "bench_tl.json").write_text(json.dumps(res, indent=2) + "\n")
    (out / "bench_tl.md").write_text(render_md(res))
    log(f"wrote {out / 'bench_tl.json'} and {out / 'bench_tl.md'}")
    if not args.keep and not args.workdir:
        shutil.rmtree(root, ignore_errors=True)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Failed as e:
        print(f"FAILED: {e}", file=sys.stderr)
        sys.exit(1)
