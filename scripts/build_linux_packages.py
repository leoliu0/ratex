#!/usr/bin/env python3
"""Build Linux native distribution packages (.deb, .rpm spec, Arch PKGBUILD)."""

import argparse
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import package_dist
REPO = Path(__file__).resolve().parent.parent


def root_owned(info: tarfile.TarInfo) -> tarfile.TarInfo:
    """Record package payload as root-owned, like dpkg-deb --root-owner-group."""
    info.uid = info.gid = 0
    info.uname = info.gname = "root"
    return info


def build_deb(stage_root: Path, output_dir: Path, version: str, arch: str) -> Path:
    """Pack stage_root into a standard Debian .deb package."""
    deb_arch = {"x86_64": "amd64", "aarch64": "arm64"}[arch]
    deb_name = f"ratex_{version}_{deb_arch}.deb"
    deb_path = output_dir.resolve() / deb_name

    installed_size = sum(p.stat().st_size for p in stage_root.rglob("*") if p.is_file())
    control_content = f"""Package: ratex
Version: {version}
Section: tex
Priority: optional
Architecture: {deb_arch}
Installed-Size: {(installed_size + 1023) // 1024}
Maintainer: Leo Liu <leoliu0@users.noreply.github.com>
Description: Ultra-fast, pure-Rust TeX engine and typesetting toolchain
 Ratex is an ultra-fast, pure-Rust TeX engine and typesetting toolchain.
Provides: ratex
"""
    if arch == "aarch64":
        control_content += "Depends: libc6 (>= 2.36)\n"
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
                tar.add(item, arcname=f"./{item.name}", filter=root_owned)

        # 3. data.tar.xz
        data_tar = tmp / "data.tar.xz"
        with tarfile.open(data_tar, "w:xz", format=tarfile.GNU_FORMAT) as tar:
            for item in stage_root.iterdir():
                tar.add(item, arcname=f"./{item.name}", filter=root_owned)

        # 4. ar archive
        # debian-binary, control.tar.gz, data.tar.xz in exact order
        cmd = ["ar", "rc", str(deb_path), "debian-binary", "control.tar.gz", "data.tar.xz"]
        subprocess.run(cmd, cwd=tmp, check=True)

    return deb_path


def build_arch_pkg(stage_root: Path, output_dir: Path, version: str, arch: str) -> Path:
    """Build Arch Linux package (.pkg.tar.zst) from stage."""
    pkg_name = f"ratex-{version}-1-{arch}.pkg.tar.zst"
    pkg_path = output_dir.resolve() / pkg_name

    with tempfile.TemporaryDirectory(prefix="arch-build-") as tmpdir:
        tmp = Path(tmpdir)
        pkgdir = tmp / "pkg"
        shutil.copytree(stage_root, pkgdir)

        installed_size = sum(p.stat().st_size for p in stage_root.rglob("*") if p.is_file())
        # .PKGINFO
        pkginfo = f"""pkgname = ratex
pkgver = {version}-1
pkgdesc = Ultra-fast, pure-Rust TeX engine and typesetting toolchain
url = https://github.com/leoliu0/ratex
builddate = {int(os.environ.get("SOURCE_DATE_EPOCH", time.time()))}
packager = Leo Liu <leoliu0@users.noreply.github.com>
size = {installed_size}
arch = {arch}
license = MIT
license = Apache-2.0
license = LPPL-1.3c
license = GPL-2.0-only
license = GPL-2.0-or-later WITH Font-exception-2.0
license = OFL-1.1
license = custom:GUST
license = custom:IPA
license = custom:Arphic
license = custom:Wadalab
provides = ratex
"""
        if arch == "aarch64":
            pkginfo += "depend = glibc>=2.36\n"
        (pkgdir / ".PKGINFO").write_text(pkginfo)

        # Pack into .pkg.tar.zst; pacman installs the recorded owners verbatim.
        cmd = ["tar", "-I", "zstd -15 -T0", "--owner=0", "--group=0", "--numeric-owner",
               "-cf", str(pkg_path), "."]
        subprocess.run(cmd, cwd=pkgdir, check=True)

    return pkg_path
def build_rpm(stage_root: Path, output_dir: Path, version: str, arch: str) -> Path:
    """Build RPM package from stage using rpmbuild."""
    rpm_dir = tempfile.mkdtemp(prefix="rpm-build-")
    try:
        top = Path(rpm_dir)
        for sub in ["BUILD", "RPMS", "SOURCES", "SPECS", "SRPMS"]:
            (top / sub).mkdir()
        runtime_requires = "\nRequires:       glibc >= 2.36" if arch == "aarch64" else ""
        spec_content = f"""Name:           ratex
Version:        {version}
Release:        1
Summary:        Ultra-fast, pure-Rust TeX engine and typesetting toolchain
License:        (MIT or Apache-2.0) and LPPL-1.3c and GPL-2.0-only and (GPL-2.0-or-later with Font-exception-2.0) and OFL-1.1 and GUST and Arphic and IPA and Wadalab
URL:            https://github.com/leoliu0/ratex
BuildArch:      {arch}
Provides:       ratex{runtime_requires}

%description
Ultra-fast pure-Rust TeX engine and toolchain.

%files
/usr/bin/*
/usr/share/tex-suite
"""
        (top / "SPECS/ratex.spec").write_text(spec_content)
        cmd = [
            "rpmbuild", "-bb", "SPECS/ratex.spec", "--target", arch,
            "--define", f"_topdir {top}",
            "--define", f"_rpmdir {output_dir.resolve()}",
            "--buildroot", str(stage_root.resolve()),
        ]
        subprocess.run(cmd, cwd=top, check=True)
        rpm_file = output_dir.resolve() / arch / f"ratex-{version}-1.{arch}.rpm"
        dest_file = output_dir.resolve() / f"ratex-{version}-1.{arch}.rpm"
        if rpm_file.is_file():
            shutil.move(str(rpm_file), str(dest_file))
            shutil.rmtree(output_dir.resolve() / arch, ignore_errors=True)
        return dest_file
    finally:
        shutil.rmtree(rpm_dir, ignore_errors=True)



def main():
    parser = argparse.ArgumentParser(description="Build Linux distribution packages.")
    parser.add_argument("--output-dir", default="dist", help="Output directory")
    parser.add_argument("--arch", choices=["x86_64", "aarch64"],
                        default=None, help="target arch (default: auto-detect host)")
    args = parser.parse_args()
    arch = args.arch or package_dist.detect_arch()
    if arch not in ("x86_64", "aarch64"):
        parser.error(f"unsupported host architecture: {arch}; use --arch x86_64 or aarch64")

    version = package_dist.workspace_version()
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
            print("Building release ratex binary...")
            subprocess.run(
                ["cargo", "build", "--locked", "--release", "-p", "tex-cli", "--bin", "ratex"],
                cwd=REPO,
                check=True,
            )
        shutil.copy2(target_bin, usr_bin / "ratex")
        package_dist.stage_font_redistribution(usr_share / "texmf" / "doc" / "fonts")
        shutil.copy2(REPO / "LICENSE", usr_share / "LICENSE")

        # Build Debian package (.deb)
        print("==> Building Debian package (.deb)...")
        deb_file = build_deb(stage, out_dir, version, arch)
        print(f"Created: {deb_file} ({deb_file.stat().st_size / 1024 / 1024:.1f} MB)")

        # Build Arch package (.pkg.tar.zst)
        print("==> Building Arch package (.pkg.tar.zst)...")
        arch_file = build_arch_pkg(stage, out_dir, version, arch)
        print(f"Created: {arch_file} ({arch_file.stat().st_size / 1024 / 1024:.1f} MB)")
        # Build RPM package (.rpm)
        if shutil.which("rpmbuild"):
            print("==> Building RPM package (.rpm)...")
            rpm_file = build_rpm(stage, out_dir, version, arch)
            print(f"Created: {rpm_file} ({rpm_file.stat().st_size / 1024 / 1024:.1f} MB)")



if __name__ == "__main__":
    main()
