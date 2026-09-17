#!/usr/bin/env python3
"""Build Linux native distribution packages (.deb, .rpm spec, Arch PKGBUILD)."""

import argparse
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import package_dist
REPO = Path(__file__).resolve().parent.parent


def get_version() -> str:
    cargo_toml = (REPO / "Cargo.toml").read_text()
    for line in cargo_toml.splitlines():
        if line.startswith("version = "):
            return line.split('"')[1]
    return "0.1.0"


def build_deb(stage_root: Path, output_dir: Path, version: str, arch: str = "amd64") -> Path:
    """Pack stage_root into a standard Debian .deb package."""
    deb_name = f"ratex_{version}_{arch}.deb"
    deb_path = output_dir.resolve() / deb_name

    control_content = f"""Package: ratex
Version: {version}
Section: tex
Priority: optional
Architecture: {arch}
Maintainer: Leo Liu <leoliu0@users.noreply.github.com>
Description: Ultra-fast, pure-Rust TeX engine and typesetting toolchain
 Ratex is an ultra-fast, pure-Rust TeX engine and typesetting toolchain.
Provides: ratex
"""
    if shutil.which("dpkg-deb"):
        with tempfile.TemporaryDirectory(prefix="deb-stage-") as tmpdir:
            tmp = Path(tmpdir)
            for item in stage_root.iterdir():
                if item.is_dir():
                    shutil.copytree(item, tmp / item.name)
                else:
                    shutil.copy2(item, tmp / item.name)
            debian_dir = tmp / "DEBIAN"
            debian_dir.mkdir()
            (debian_dir / "control").write_text(control_content)
            cmd = ["dpkg-deb", "--build", "--root-owner-group", str(tmp), str(deb_path)]
            subprocess.run(cmd, check=True)
            return deb_path

    with tempfile.TemporaryDirectory(prefix="deb-build-") as tmpdir:
        tmp = Path(tmpdir)
        # 1. debian-binary
        (tmp / "debian-binary").write_text("2.0\n")

        # 2. control.tar.gz
        control_dir = tmp / "control"
        control_dir.mkdir()
        (control_dir / "control").write_text(control_content)

        control_tar = tmp / "control.tar.gz"
        with tarfile.open(control_tar, "w:gz", format=tarfile.GNU_FORMAT) as tar:
            for item in control_dir.iterdir():
                tar.add(item, arcname=f"./{item.name}")

        # 3. data.tar.xz
        data_tar = tmp / "data.tar.xz"
        with tarfile.open(data_tar, "w:xz", format=tarfile.GNU_FORMAT) as tar:
            for item in stage_root.iterdir():
                tar.add(item, arcname=f"./{item.name}")

        # 4. ar archive
        # debian-binary, control.tar.gz, data.tar.xz in exact order
        cmd = ["ar", "rc", str(deb_path), "debian-binary", "control.tar.gz", "data.tar.xz"]
        subprocess.run(cmd, cwd=tmp, check=True)

    return deb_path


def build_arch_pkg(stage_root: Path, output_dir: Path, version: str) -> Path:
    """Build Arch Linux package (.pkg.tar.zst) from stage."""
    pkg_name = f"ratex-{version}-1-x86_64.pkg.tar.zst"
    pkg_path = output_dir.resolve() / pkg_name

    with tempfile.TemporaryDirectory(prefix="arch-build-") as tmpdir:
        tmp = Path(tmpdir)
        pkgdir = tmp / "pkg"
        shutil.copytree(stage_root, pkgdir)

        # .PKGINFO
        pkginfo = f"""pkgname = ratex
pkgver = {version}-1
pkgdesc = Ultra-fast, pure-Rust TeX engine and typesetting toolchain
url = https://github.com/leoliu0/ratex
builddate = 1710000000
packager = Leo Liu <leoliu0@users.noreply.github.com>
size = 50000000
arch = x86_64
license = MIT
license = Apache-2.0
provides = ratex
"""
        (pkgdir / ".PKGINFO").write_text(pkginfo)

        # Pack into .pkg.tar.zst
        cmd = ["tar", "-I", "zstd -15 -T0", "-cf", str(pkg_path), "."]
        subprocess.run(cmd, cwd=pkgdir, check=True)

    return pkg_path
def build_rpm(stage_root: Path, output_dir: Path, version: str) -> Path:
    """Build RPM package from stage using rpmbuild."""
    rpm_dir = tempfile.mkdtemp(prefix="rpm-build-")
    try:
        top = Path(rpm_dir)
        for sub in ["BUILD", "RPMS", "SOURCES", "SPECS", "SRPMS"]:
            (top / sub).mkdir()
        spec_content = f"""Name:           ratex
Version:        {version}
Release:        1
Summary:        Ultra-fast, pure-Rust TeX engine and typesetting toolchain
License:        MIT or Apache-2.0
URL:            https://github.com/leoliu0/ratex
BuildArch:      x86_64
Provides:       ratex

%description
Ultra-fast pure-Rust TeX engine and toolchain.

%files
/usr/bin/*
/usr/share/tex-suite
"""
        (top / "SPECS/ratex.spec").write_text(spec_content)
        cmd = [
            "rpmbuild", "-bb", "SPECS/ratex.spec",
            "--define", f"_topdir {top}",
            "--define", f"_rpmdir {output_dir.resolve()}",
            "--buildroot", str(stage_root.resolve()),
        ]
        subprocess.run(cmd, cwd=top, check=True)
        rpm_file = output_dir.resolve() / "x86_64" / f"ratex-{version}-1.x86_64.rpm"
        dest_file = output_dir.resolve() / f"ratex-{version}-1.x86_64.rpm"
        if rpm_file.is_file():
            shutil.move(str(rpm_file), str(dest_file))
            shutil.rmtree(output_dir.resolve() / "x86_64", ignore_errors=True)
        return dest_file
    finally:
        shutil.rmtree(rpm_dir, ignore_errors=True)



def main():
    parser = argparse.ArgumentParser(description="Build Linux distribution packages.")
    parser.add_argument("--output-dir", default="dist", help="Output directory")
    args = parser.parse_args()

    version = get_version()
    out_dir = Path(args.output_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    # Stage directory: usr/bin and usr/share/tex-suite
    with tempfile.TemporaryDirectory(prefix="stage-") as tmpdir:
        stage = Path(tmpdir)
        usr_bin = stage / "usr" / "bin"
        usr_bin.mkdir(parents=True)
        usr_share = stage / "usr" / "share" / "tex-suite"
        usr_share.mkdir(parents=True)

        target_bin = REPO / "target" / "release" / "ratex"
        if not target_bin.exists():
            target_bin = REPO / "target" / "release" / "texmk"
        if not target_bin.exists():
            print("Building release binaries...")
            subprocess.run(["cargo", "build", "--release", "--workspace"], cwd=REPO, check=True)
            target_bin = REPO / "target" / "release" / "ratex"
            if not target_bin.exists():
                target_bin = REPO / "target" / "release" / "texmk"

        # Copy canonical binary
        shutil.copy2(target_bin, usr_bin / "ratex")
        stage_texmf = usr_share / "texmf"
        package_dist.stage_assets(stage_texmf)

        # Build Debian package (.deb)
        print("==> Building Debian package (.deb)...")
        deb_file = build_deb(stage, out_dir, version)
        print(f"Created: {deb_file} ({deb_file.stat().st_size / 1024 / 1024:.1f} MB)")

        # Build Arch package (.pkg.tar.zst)
        print("==> Building Arch package (.pkg.tar.zst)...")
        arch_file = build_arch_pkg(stage, out_dir, version)
        print(f"Created: {arch_file} ({arch_file.stat().st_size / 1024 / 1024:.1f} MB)")
        # Build RPM package (.rpm)
        if shutil.which("rpmbuild"):
            print("==> Building RPM package (.rpm)...")
            rpm_file = build_rpm(stage, out_dir, version)
            print(f"Created: {rpm_file} ({rpm_file.stat().st_size / 1024 / 1024:.1f} MB)")



if __name__ == "__main__":
    main()
