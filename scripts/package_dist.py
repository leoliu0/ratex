#!/usr/bin/env python3
"""Build a self-contained tex-suite distribution package for one target OS.

Assembles release binaries, the pdflatex.fmt format file, a minimal TeX
assets tree (hyphenation patterns etc.), the platform installer scripts,
a README and a manifest with SHA256 checksums, then produces:

  Linux/macOS: dist/tex-suite-v0.1.0-linux-x86_64.tar.gz
  Windows:     dist/tex-suite-v0.1.0-windows-x86_64.zip

Bundle layout (inside the archive root tex-suite-<platform>-<arch>/):
  bin/            pdflatex, xelatex, lualatex, tex-bibtex (+bibtex alias),
                  texmk (+latexmk alias)
  share/tex-suite/pdflatex.fmt
  share/tex-suite/texmf/   TDS tree (tex/generic/hyphen/hyphen.tex, ...)
  <installers>    install-*.sh / install*.ps1 / install*.bat at archive root
  README.txt
  manifest.json

Engines discover the data tree through TEXMFLOCAL/TEXMFDIST (kpathsea
style); the installers are responsible for pointing TEXMFLOCAL at
share/tex-suite/texmf after installation.
"""

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

BINARIES = ["pdflatex", "xelatex", "lualatex", "tex-bibtex", "texmk", "tex-index"]
ALIASES = {"tex-bibtex": "bibtex", "texmk": "latexmk"}

# Minimal TeX Live files bundled under texmf/, keyed by TDS-relative path.
# Each entry is a list of candidate sources tried in order. Placeholders
# are generated (and warned about) when no candidate exists on the build
# machine, so packaging never silently ships an incomplete tree.
ESSENTIAL_ASSETS = {
    "tex/generic/hyphen/hyphen.tex": [
        "tex/generic/hyphen/hyphen.tex",
    ],
    "tex/generic/hyphen/zerohyph.tex": [
        "tex/generic/hyphen/zerohyph.tex",
    ],
    "tex/generic/hyphen/dumyhyph.tex": [
        "tex/generic/hyphen/dumyhyph.tex",
    ],
    "tex/generic/language/language.dat": [
        "tex/generic/language/language.dat",
        "tex/generic/config/language.dat",
        "tex/lambda/config/language.dat",
        "tex/generic/babel/language.dat",
    ],
}
# Optional assets: bundled when present, silently skipped otherwise
# (they exist only in some TeX Live vintages).
OPTIONAL_ASSETS = {
    "tex/generic/babel/locale/en/hyphen-en.tex": [
        "tex/generic/babel/locale/en/hyphen-en.tex",
    ],
    "tex/generic/babel/language.dat": [
        "tex/generic/babel/language.dat",
    ],
}

SYSTEM_TEXMF_PREFIXES = [
    "/usr/share/texmf-dist",
    "/usr/local/share/texmf-dist",
    "/opt/texlive/2025/texmf-dist",
    "/usr/share/texmf",
    "/usr/local/texlive",
]

INSTALLER_GLOBS = [
    "packaging/install-*.sh",
    "packaging/install-*.ps1",
    "packaging/install-*.bat",
    "packaging/install.sh",
    "packaging/install.bat",
    "scripts/install-*.sh",
    "scripts/install-*.ps1",
    "scripts/install-*.bat",
    "install.sh",
]


def die(msg: str) -> None:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def warn(msg: str) -> None:
    print(f"warning: {msg}", file=sys.stderr)


def detect_platform() -> str:
    s = platform.system()
    return {"Linux": "linux", "Darwin": "macos", "Windows": "windows"}.get(s, s.lower())


def detect_arch() -> str:
    m = platform.machine().lower()
    return {"x86_64": "x86_64", "amd64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64"}.get(m, m)


def workspace_version() -> str:
    """Read [workspace.package] version from Cargo.toml."""
    text = (REPO / "Cargo.toml").read_text()
    m = re.search(r"\[workspace\.package\][^[]*?version\s*=\s*\"([^\"]+)\"", text)
    if not m:
        die("cannot find [workspace.package] version in Cargo.toml")
    return m.group(1)


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def run_cargo_build() -> None:
    print("==> cargo build --release --workspace")
    subprocess.run(
        ["cargo", "build", "--release", "--workspace"],
        cwd=REPO,
        check=True,
    )


def exe(name: str, is_windows: bool) -> str:
    return name + (".exe" if is_windows else "")


def collect_binaries(release_dir: Path, stage_bin: Path, is_windows: bool) -> list:
    stage_bin.mkdir(parents=True, exist_ok=True)
    copied = []
    for b in BINARIES:
        src = release_dir / exe(b, is_windows)
        if not src.is_file():
            die(f"binary {src} not found; run with --build or build the workspace first")
        dst = stage_bin / exe(b, is_windows)
        shutil.copy2(src, dst)
        if not is_windows:
            dst.chmod(dst.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        copied.append(dst)
        alias = ALIASES.get(b)
        if alias:
            adst = stage_bin / exe(alias, is_windows)
            shutil.copy2(dst, adst)  # real copy: works in zips and on Windows
            copied.append(adst)
    return copied


def find_format_file(explicit: str) -> Path:
    format_source = (REPO / "crates/tex-core/src/format.rs").read_text()
    expected = tuple(int(re.search(rf"(?:pub )?const {key}: u16 = (\d+);", format_source).group(1))
                     for key in ("VERSION", "SEMANTICS"))

    def compatible(path: Path) -> bool:
        if not path.is_file():
            return False
        with path.open("rb") as source:
            header = source.read(12)
        return (len(header) == 12 and header[:8] == b"RUSTEXFM"
                and struct.unpack("<HH", header[8:]) == expected)

    candidates = []
    if explicit:
        path = Path(explicit)
        if not compatible(path):
            die(f"format {path} is missing or incompatible; expected version/semantics {expected}")
        return path
    # Repo-root working fmt, then the tracked precompiled asset (CI runners
    # have no system TeX, so building via -ini would fail there).
    candidates += [
        REPO / "pdflatex.fmt",
        REPO / "crates" / "tex-cli" / "assets" / "default.fmt",
        REPO / "tmp" / "pdflatex.fmt",
    ]
    for c in candidates:
        if compatible(c):
            return c
    # Last resort: build it with the freshly packaged engine.
    pdflatex = REPO / "target" / "release" / "pdflatex"
    if pdflatex.is_file():
        print("==> pdflatex.fmt missing; building with `pdflatex -ini`")
        with tempfile.TemporaryDirectory() as td:
            out = Path(td) / "pdflatex.fmt"
            r = subprocess.run(
                [str(pdflatex), "-ini", "-interaction=nonstopmode", "pdflatex"],
                cwd=td,
                capture_output=True,
            )
            if r.returncode == 0 and compatible(out):
                target = REPO / "pdflatex.fmt"
                shutil.move(str(out), target)
                return target
            warn(f"format build failed (exit {r.returncode}); see engine output below")
            sys.stdout.write((r.stdout or b"").decode(errors="replace")[-2000:])
            sys.stdout.write((r.stderr or b"").decode(errors="replace")[-2000:])
    die("pdflatex.fmt not found and could not be built; pass --fmt <path>")


def source_texmf_roots() -> list:
    roots = []
    for env in ("TEXMFLOCAL", "TEXMFDIST"):
        for p in os.environ.get(env, "").split(os.pathsep):
            if p and Path(p).is_dir():
                roots.append(Path(p))
    for p in SYSTEM_TEXMF_PREFIXES:
        rp = Path(p)
        if rp.is_dir() and rp not in roots:
            roots.append(rp)
    return roots


def find_asset(rel: str, candidates: list, roots: list):
    local = REPO / "texmf" / rel  # repo-local copy wins (any build machine)
    if local.is_file():
        return local
    for cand in candidates:
        for root in roots:
            probe = root / cand
            if probe.is_file():
                return probe
    return None


def stage_assets(texmf_stage: Path) -> dict:
    """Copy essential (+ optional, when present) assets into the staged
    TDS tree. Returns path->source map for manifest.json."""
    texmf_stage.mkdir(parents=True, exist_ok=True)
    roots = source_texmf_roots()
    placed = {}
    for rel, candidates in ESSENTIAL_ASSETS.items():
        dst = texmf_stage / rel
        src = find_asset(rel, candidates, roots)
        if src is None:
            # Placeholder so the bundle tree is complete and inspectable.
            dst.parent.mkdir(parents=True, exist_ok=True)
            dst.write_text(
                f"% PLACEHOLDER: {rel} not found on this build machine.\n"
                f"% Install it or drop a copy under {REPO / 'texmf' / rel}.\n"
            )
            warn(f"asset {rel}: not found on build machine; wrote placeholder")
            placed[rel] = "placeholder"
        else:
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst)
            placed[rel] = str(src)
    for rel, candidates in OPTIONAL_ASSETS.items():
        src = find_asset(rel, candidates, roots)
        if src is None:
            continue
        dst = texmf_stage / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)
        placed[rel] = str(src)
    return placed


def collect_installers(platform_name: str) -> list:
    """Repo files to stage at the archive root for this platform.

    A script is *tagged* if its name mentions a platform; untagged scripts
    (install.sh) are generic dispatchers and ship with unix bundles.
    Windows bundles only get .ps1/.bat (or -windows-tagged) installers.
    """

    def tag(name: str) -> str:
        for t, plat in (("windows", "windows"), ("win", "windows"),
                        ("macos", "macos"), ("darwin", "macos"), ("osx", "macos"),
                        ("linux", "linux")):
            if t in name:
                return plat
        return ""

    out, seen = [], set()
    for pattern in INSTALLER_GLOBS:
        for p in sorted(REPO.glob(pattern)):
            if not p.is_file() or p in seen:
                continue
            t = tag(p.name.lower())
            if platform_name == "windows":
                ok = t == "windows" or p.suffix.lower() in (".ps1", ".bat")
            else:
                ok = p.suffix == ".sh" and t in ("", platform_name)
            if ok:
                seen.add(p)
                out.append(p)
    if not out:
        warn(f"no installer scripts found for {platform_name}; bundle will lack an installer")
    return out


def readme_text(version: str, platform_name: str, arch: str) -> str:
    suffix = ".exe" if platform_name == "windows" else ""
    installer = "install-windows.ps1 (or install.bat)" if platform_name == "windows" else "./install.sh (or the install-<os>.sh scripts)"
    return f"""tex-suite v{version} — self-contained TeX engines ({platform_name}-{arch})
=======================================================================

Contents
  bin/                pdflatex{suffix}, xelatex{suffix}, lualatex{suffix},
                      tex-bibtex{suffix} (alias bibtex{suffix}),
                      texmk{suffix} (alias latexmk{suffix})
  share/tex-suite/    pdflatex.fmt format file + texmf/ minimal asset tree
  manifest.json       file list with SHA256 checksums
  installer script    {installer}

Quickstart
  1. Extract this archive.
  2. Run the installer (see README output of `--help`): it copies bin/ to
     your PATH directory and share/tex-suite/ to the data directory.
  3. Verify:  pdflatex --version   (or run `pdflatex file.tex`)

Without the installer
  Add bin/ to PATH and point kpathsea at the bundled tree:
    Linux/macOS:  export TEXMFLOCAL="$PWD/share/tex-suite/texmf"
    Windows:      set TEXMFLOCAL=%CD%\\share\\tex-suite\\texmf
  pdflatex{suffix} also finds pdflatex.fmt next to the executable: copy
  share/tex-suite/pdflatex.fmt into bin/ if you skip the installer.

Notes
  * bibtex{suffix} and latexmk{suffix} are copies of tex-bibtex{suffix}
    and texmk{suffix}; behaviour is selected by program name.
  * This bundle ships a minimal asset tree (hyphenation, language data).
    Full LaTeX support comes from a system TeX distribution if present;
    the bundled engines still search TEXMFHOME/TEXMFLOCAL/TEXMFDIST and
    standard system texmf directories.
"""


def make_tar_gz(root_name: str, stage_dir: Path, out: Path) -> None:
    with tarfile.open(out, "w:gz") as tf:
        for p in sorted(stage_dir.rglob("*")):
            if p.is_file():
                tf.add(p, arcname=f"{root_name}/{p.relative_to(stage_dir)}")


def make_zip(root_name: str, stage_dir: Path, out: Path, is_windows: bool) -> None:
    with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        for p in sorted(stage_dir.rglob("*")):
            if p.is_file():
                rel = f"{root_name}/{p.relative_to(stage_dir)}"
                zi = zipfile.ZipInfo.from_file(p, arcname=rel)
                if not is_windows:
                    zi.external_attr = (0o755 << 16)
                zf.writestr(zi, p.read_bytes())


def verify_archive(out: Path, root_name: str, is_windows: bool) -> int:
    """List archive, ensure expected members exist and it opens cleanly."""
    if out.suffix == ".zip":
        with zipfile.ZipFile(out) as zf:
            names = zf.namelist()
            if zf.testzip() is not None:
                die(f"zip integrity check failed for {out}")
    else:
        with tarfile.open(out, "r:gz") as tf:
            names = tf.getnames()
    s = ".exe" if is_windows else ""
    expect = [
        f"{root_name}/bin/pdflatex{s}",
        f"{root_name}/bin/xelatex{s}",
        f"{root_name}/bin/lualatex{s}",
        f"{root_name}/bin/tex-bibtex{s}",
        f"{root_name}/bin/bibtex{s}",
        f"{root_name}/bin/texmk{s}",
        f"{root_name}/bin/latexmk{s}",
        f"{root_name}/share/tex-suite/pdflatex.fmt",
        f"{root_name}/README.txt",
        f"{root_name}/manifest.json",
    ]
    for e in expect:
        if e not in names:
            die(f"archive {out.name} missing {e}")
    return len(names)


def main() -> None:
    ap = argparse.ArgumentParser(
        description="Build a self-contained tex-suite distribution bundle.",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    ap.add_argument("--platform", choices=["linux", "macos", "windows"],
                    default=None, help="target OS (default: auto-detect host)")
    ap.add_argument("--arch", choices=["x86_64", "aarch64"],
                    default=None, help="target arch (default: auto-detect host)")
    ap.add_argument("--build", action="store_true",
                    help="run `cargo build --release --workspace` first")
    ap.add_argument("--output-dir", default="dist",
                    help="directory to place distribution archives")
    ap.add_argument("--fmt", default=None,
                    help="explicit path to pdflatex.fmt (default: repo root, else build it)")
    ap.add_argument("--target-dir", default=None,
                    help="cargo target directory override (default: target/release)")
    args = ap.parse_args()

    platform_name = args.platform or detect_platform()
    arch = args.arch or detect_arch()
    is_windows = platform_name == "windows"
    version = workspace_version()

    if args.build:
        run_cargo_build()

    release_dir = Path(args.target_dir) if args.target_dir else REPO / "target" / "release"
    if not release_dir.is_dir():
        die(f"release dir {release_dir} missing; rerun with --build")

    root_name = f"tex-suite-{platform_name}-{arch}"
    out_dir = Path(args.output_dir)
    if not out_dir.is_absolute():
        out_dir = REPO / out_dir
    out_dir.mkdir(parents=True, exist_ok=True)

    stage = Path(tempfile.mkdtemp(prefix="texsuite-stage-"))
    try:
        stage_root = stage / root_name
        stage_bin = stage_root / "bin"
        stage_share = stage_root / "share" / "tex-suite"
        stage_texmf = stage_share / "texmf"

        print(f"==> staging {root_name} (v{version})")
        binaries = collect_binaries(release_dir, stage_bin, is_windows)

        fmt = find_format_file(args.fmt)
        stage_share.mkdir(parents=True, exist_ok=True)
        shutil.copy2(fmt, stage_share / "pdflatex.fmt")
        print(f"    fmt: {fmt}")

        assets = stage_assets(stage_texmf)

        for inst in collect_installers(platform_name):
            dst = stage_root / inst.name
            shutil.copy2(inst, dst)
            print(f"    installer: {inst.relative_to(REPO)}")

        (stage_root / "README.txt").write_text(readme_text(version, platform_name, arch))

        # manifest.json: checksums of everything except the manifest itself
        files = {}
        for p in sorted(stage_root.rglob("*")):
            if p.is_file():
                files[str(p.relative_to(stage_root))] = sha256_file(p)
        manifest = {
            "name": "tex-suite",
            "version": version,
            "platform": platform_name,
            "arch": arch,
            "build_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "bundle_root": root_name,
            "binaries": [exe(b, is_windows) for b in BINARIES]
            + [exe(a, is_windows) for a in ALIASES.values()],
            "format_file": "share/tex-suite/pdflatex.fmt",
            "assets": assets,
            "files": files,
        }
        (stage_root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")

        if is_windows:
            archive = out_dir / f"tex-suite-v{version}-{platform_name}-{arch}.zip"
            make_zip(root_name, stage_root, archive, is_windows)
        else:
            archive = out_dir / f"tex-suite-v{version}-{platform_name}-{arch}.tar.gz"
            make_tar_gz(root_name, stage_root, archive)

        n = verify_archive(archive, root_name, is_windows)
        size = archive.stat().st_size
        print(f"==> {archive} ({size / 1e6:.1f} MB, {n} members, "
              f"{len(binaries)} binaries, sha256={sha256_file(archive)[:16]}…)")
        print(str(archive))
    finally:
        shutil.rmtree(stage, ignore_errors=True)


if __name__ == "__main__":
    main()
