#!/usr/bin/env python3
"""Build a self-contained tex-suite distribution package for one target OS.

Assembles the self-contained engine, font licenses and corresponding sources,
platform installers, a README and a SHA256 manifest, then produces:

  Linux/macOS: dist/tex-suite-v0.1.0-linux-x86_64.tar.gz
  Windows:     dist/tex-suite-v0.1.0-windows-x86_64.zip

Bundle layout (inside the archive root tex-suite-<platform>-<arch>/):
  bin/            texmk plus public command aliases
  share/tex-suite/texmf/doc/fonts/   licenses, lock and corresponding sources
  <installers>    install-*.sh / install*.ps1 / install*.bat at archive root
  README.txt
  manifest.json

The format, packages, fonts, maps, TeX engine, and BibTeX engine are embedded
in ratex. Do not stage a second TEXMF tree from the packaging host: it can
silently override the locked embedded resources.
"""

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

BINARIES = ["ratex"]
ALIASES = {}
# xelatex/lualatex are themselves tiny instances of the generic launcher.
# Reuse one of them to create every alias in a Windows zip.
WINDOWS_LAUNCHER = "xelatex"
WINDOWS_LAUNCHER_MARKER = b"tex-suite launcher: unsupported executable name"

ZSTD_MAGIC = b"\x28\xb5\x2f\xfd"
# A normal format expands to roughly 8 MiB. Keep explicit distribution
# overrides bounded so a mislabeled archive cannot exhaust the packaging host.
MAX_DECOMPRESSED_FORMAT_BYTES = 128 << 20


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
    if is_windows:
        # Zip archives do not portably preserve symlinks. Ship one tiny relay
        # for every public alias instead of duplicating the PDF engine.
        launcher = release_dir / exe(WINDOWS_LAUNCHER, True)
        if not launcher.is_file():
            die(f"launcher {launcher} not found; build the tex-cli workspace first")
        if WINDOWS_LAUNCHER_MARKER not in launcher.read_bytes():
            die(
                f"{launcher} is not the generic tex-suite launcher; "
                "rebuild the tex-cli workspace before packaging Windows"
            )
        for alias in ALIASES:
            adst = stage_bin / exe(alias, True)
            shutil.copy2(launcher, adst)
            copied.append(adst)
    else:
        for alias, target in ALIASES.items():
            adst = stage_bin / alias
            adst.symlink_to(target)
            copied.append(adst)
    return copied


def decode_zstd_file(path: Path) -> bytes:
    decoder = shutil.which("zstd")
    if decoder is None:
        die(
            f"cannot validate compressed format {path}: the zstd command is not installed; "
            "install zstd or pass an uncompressed .fmt"
        )

    process = subprocess.Popen(
        [decoder, "-q", "-d", "-c", "--", str(path)],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    assert process.stdout is not None
    data = bytearray()
    while True:
        chunk = process.stdout.read(1 << 20)
        if not chunk:
            break
        data.extend(chunk)
        if len(data) > MAX_DECOMPRESSED_FORMAT_BYTES:
            process.kill()
            process.communicate()
            die(
                f"compressed format {path} expands beyond "
                f"{MAX_DECOMPRESSED_FORMAT_BYTES // (1 << 20)} MiB"
            )
    stderr = process.stderr.read() if process.stderr is not None else b""
    returncode = process.wait()
    if returncode != 0:
        detail = stderr.decode(errors="replace").strip()
        suffix = f": {detail}" if detail else ""
        die(f"cannot decompress format {path} with zstd{suffix}")
    return bytes(data)


def validate_format_file(explicit: str) -> Path:
    """Validate an explicitly requested external format.

    Production bundles use the compressed format embedded in the engine. An
    external format remains available as an override for developers and
    downstream distributors.
    """
    format_source = (REPO / "crates/tex-core/src/format.rs").read_text()
    expected = tuple(
        int(re.search(rf"(?:pub )?const {key}: u16 = (\d+);", format_source).group(1))
        for key in ("VERSION", "SEMANTICS")
    )
    # Keep this in step with tex_core::format::parse_header, which accepts the
    # preceding v8 wire layout and fills its newly added fields with defaults.
    accepted_versions = {expected[0], 9, 8}

    def compatible(path: Path) -> bool:
        if not path.is_file():
            return False
        with path.open("rb") as source:
            data = source.read(12)
        if data[:4] == ZSTD_MAGIC:
            data = decode_zstd_file(path)
        if len(data) < 12 or data[:8] != b"RUSTEXFM":
            return False
        version = int.from_bytes(data[8:10], "little")
        semantics = int.from_bytes(data[10:12], "little")
        return version in accepted_versions and semantics == expected[1]

    path = Path(explicit)
    if not compatible(path):
        versions = "/".join(str(version) for version in sorted(accepted_versions))
        die(
            f"format {path} is missing or incompatible; expected wire version "
            f"{versions} and semantics {expected[1]}"
        )
    return path






def stage_font_redistribution(destination: Path) -> dict:
    """Stage the locked licenses and actual corresponding font sources offline."""
    assets_dir = REPO / "crates" / "tex-kpse" / "assets"
    lock_path = assets_dir / "packages.lock.json"
    lock = json.loads(lock_path.read_text(encoding="utf-8"))
    source_info = lock["source_distribution"]
    source = assets_dir / source_info["logical_name"]
    if (source.stat().st_size != source_info["size_bytes"]
            or sha256_file(source) != source_info["sha256"]):
        raise ValueError(f"font source archive does not match {lock_path}")
    legal_dir = assets_dir / "legal"
    if not (legal_dir / "NOTICES-FONTS.txt").is_file():
        raise FileNotFoundError(legal_dir / "NOTICES-FONTS.txt")
    destination.mkdir(parents=True, exist_ok=True)
    placed = {}
    for path in sorted(legal_dir.rglob("*")):
        if path.is_file():
            relative = path.relative_to(legal_dir)
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)
            placed[relative.as_posix()] = "locked-font-legal"
    shutil.copy2(source, destination / source.name)
    placed[source.name] = "locked-font-source"
    shutil.copy2(lock_path, destination / lock_path.name)
    placed[lock_path.name] = "locked-font-inventory"
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
    return f"""tex-suite v{version} — self-contained TeX engine ({platform_name}-{arch})
=======================================================================

Contents
  bin/                texmk{suffix}, with pdflatex{suffix}, xelatex{suffix},
                      lualatex{suffix}, tex-bibtex{suffix}, bibtex{suffix},
                      and latexmk{suffix} command aliases
  share/tex-suite/    optional compatibility assets
  manifest.json       file list with SHA256 checksums
  installer script    {installer}

Quickstart
  1. Extract this archive.
  2. Run the installer (see README output of `--help`): it copies bin/ to
     your PATH directory and share/tex-suite/ to the data directory.
  3. Verify:  pdflatex --version   (or run `pdflatex file.tex`)

Without the installer
  Run bin/texmk directly or add bin/ to PATH. No TeX installation or data
  environment variables are required.

Notes
  * The production format, package archive, fonts, maps, TeX engine, and
    BibTeX engine are embedded in texmk.
  * Linux/macOS aliases are symlinks. Windows aliases are small launchers;
    the full executable is stored only once.
  * Resolution is strictly self-contained using ratex's bundled installation
    assets. No external TeX Live installation or network access is required.
"""


def make_tar_gz(root_name: str, stage_dir: Path, out: Path) -> None:
    with tarfile.open(out, "w:gz") as tf:
        for p in sorted(stage_dir.rglob("*")):
            if p.is_file() or p.is_symlink():
                tf.add(p, arcname=f"{root_name}/{p.relative_to(stage_dir)}")


def make_zip(root_name: str, stage_dir: Path, out: Path, is_windows: bool) -> None:
    with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        for p in sorted(stage_dir.rglob("*")):
            if p.is_file():
                rel = f"{root_name}/{p.relative_to(stage_dir)}"
                zi = zipfile.ZipInfo.from_file(p, arcname=rel)
                if not is_windows:
                    zi.external_attr = (0o755 << 16)
                zi.compress_type = zipfile.ZIP_DEFLATED
                zf.writestr(zi, p.read_bytes())


def verify_archive(out: Path, root_name: str, is_windows: bool, has_format: bool) -> int:
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
        f"{root_name}/bin/ratex{s}",
        f"{root_name}/README.txt",
        f"{root_name}/manifest.json",
    ]
    expect.extend(
        f"{root_name}/share/tex-suite/texmf/doc/fonts/{name}"
        for name in ("NOTICES-FONTS.txt", "sources.tar.zst", "packages.lock.json")
    )
    if has_format:
        expect.append(f"{root_name}/share/tex-suite/pdflatex.fmt")
    for e in expect:
        if e not in names:
            die(f"archive {out.name} missing {e}")
    return len(names)


def inventory_stage(stage_root: Path) -> tuple[dict, dict]:
    """Return disjoint regular-file checksums and symlink targets."""
    files = {}
    links = {}
    for path in sorted(stage_root.rglob("*")):
        relative = str(path.relative_to(stage_root))
        if path.is_symlink():
            links[relative] = os.readlink(path)
        elif path.is_file():
            files[relative] = sha256_file(path)
    return files, links


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
                    help="optional external pdflatex.fmt override (the default is embedded)")
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

        stage_share.mkdir(parents=True, exist_ok=True)
        format_file = None
        if args.fmt:
            fmt = validate_format_file(args.fmt)
            shutil.copy2(fmt, stage_share / "pdflatex.fmt")
            format_file = "share/tex-suite/pdflatex.fmt"
            print(f"    external fmt: {fmt}")
        else:
            print("    fmt: compressed format embedded in texmk")

        assets = {
            f"doc/fonts/{name}": provenance
            for name, provenance in stage_font_redistribution(stage_texmf / "doc" / "fonts").items()
        }

        for inst in collect_installers(platform_name):
            dst = stage_root / inst.name
            shutil.copy2(inst, dst)
            print(f"    installer: {inst.relative_to(REPO)}")

        (stage_root / "README.txt").write_text(readme_text(version, platform_name, arch))
        for lic in ["LICENSE-MIT", "LICENSE-APACHE", "LICENSE"]:
            lic_path = REPO / lic
            if lic_path.is_file():
                shutil.copy2(lic_path, stage_root / lic)

        # manifest.json: checksums of everything except the manifest itself
        files, links = inventory_stage(stage_root)
        manifest = {
            "name": "tex-suite",
            "version": version,
            "platform": platform_name,
            "arch": arch,
            "build_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "bundle_root": root_name,
            "binaries": [exe(b, is_windows) for b in BINARIES]
            + [exe(a, is_windows) for a in ALIASES],
            "format_file": format_file,
            "assets": assets,
            "files": files,
            "links": links,
        }
        (stage_root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")

        if is_windows:
            archive = out_dir / f"tex-suite-v{version}-{platform_name}-{arch}.zip"
            make_zip(root_name, stage_root, archive, is_windows)
        else:
            archive = out_dir / f"tex-suite-v{version}-{platform_name}-{arch}.tar.gz"
            make_tar_gz(root_name, stage_root, archive)

        n = verify_archive(archive, root_name, is_windows, format_file is not None)
        size = archive.stat().st_size
        print(f"==> {archive} ({size / 1e6:.1f} MB, {n} members, "
              f"{len(binaries)} binaries, sha256={sha256_file(archive)[:16]}…)")
        print(str(archive))
    finally:
        shutil.rmtree(stage, ignore_errors=True)


if __name__ == "__main__":
    main()
