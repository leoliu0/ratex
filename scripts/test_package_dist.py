#!/usr/bin/env python3
"""Regression tests for the distribution's single-copy binary layout."""

import contextlib
import io
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
from unittest import mock
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import package_dist  # noqa: E402


class PackageLayoutTests(unittest.TestCase):
    def make_release(self, root: Path, windows: bool) -> None:
        for index, name in enumerate(package_dist.BINARIES):
            path = root / package_dist.exe(name, windows)
            path.write_bytes(f"canonical-{index}".encode())
        if windows:
            (root / package_dist.exe(package_dist.WINDOWS_LAUNCHER, True)).write_bytes(
                b"small-launcher\0" + package_dist.WINDOWS_LAUNCHER_MARKER
            )

    def test_unix_aliases_are_relative_symlinks(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            release, stage = root / "release", root / "stage"
            release.mkdir()
            self.make_release(release, False)
            package_dist.collect_binaries(release, stage, False)

            regular = {path.name for path in stage.iterdir() if not path.is_symlink()}
            self.assertEqual(regular, set(package_dist.BINARIES))
            for alias, target in package_dist.ALIASES.items():
                path = stage / alias
                self.assertTrue(path.is_symlink(), alias)
                self.assertEqual(path.readlink(), Path(target))

            archive = root / "bundle.tar.gz"
            package_dist.make_tar_gz("bundle", stage, archive)
            with tarfile.open(archive) as bundle:
                for alias, target in package_dist.ALIASES.items():
                    member = bundle.getmember(f"bundle/{alias}")
                    self.assertTrue(member.issym(), alias)
                    self.assertEqual(member.linkname, target)

    def test_windows_aliases_use_the_small_launcher(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            release, stage = root / "release", root / "stage"
            release.mkdir()
            self.make_release(release, True)
            package_dist.collect_binaries(release, stage, True)

            for alias in package_dist.ALIASES:
                path = stage / package_dist.exe(alias, True)
                self.assertEqual(
                    path.read_bytes(),
                    b"small-launcher\0" + package_dist.WINDOWS_LAUNCHER_MARKER,
                )

    def test_windows_packaging_rejects_a_stale_full_engine(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            release, stage = root / "release", root / "stage"
            release.mkdir()
            self.make_release(release, False)
            # Give the synthetic Windows payload all canonical .exe names but
            # an old engine in the slot now reserved for the generic launcher.
            for index, name in enumerate(package_dist.BINARIES):
                (release / package_dist.exe(name, True)).write_bytes(
                    f"canonical-{index}".encode()
                )
            (release / "xelatex.exe").write_bytes(b"old full TeX engine")
            with contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    package_dist.collect_binaries(release, stage, True)

    def test_zip_members_are_deflated(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            stage = root / "stage"
            stage.mkdir()
            payload = b"compressible package payload\n" * 4096
            (stage / "payload.bin").write_bytes(payload)
            archive = root / "bundle.zip"

            package_dist.make_zip("bundle", stage, archive, True)

            with zipfile.ZipFile(archive) as bundle:
                member = bundle.getinfo("bundle/payload.bin")
                self.assertEqual(member.compress_type, zipfile.ZIP_DEFLATED)
                self.assertLess(member.compress_size, member.file_size)

    def test_asset_provenance_is_stable_and_relative(self) -> None:
        repo_asset = package_dist.REPO / "texmf" / "tex" / "generic" / "hyphen" / "hyphen.tex"
        self.assertEqual(
            package_dist.asset_source_label(repo_asset, []),
            "repo:texmf/tex/generic/hyphen/hyphen.tex",
        )
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            asset = root / "tex" / "generic" / "hyphen" / "hyphen.tex"
            self.assertEqual(
                package_dist.asset_source_label(asset, [root]),
                "texmf:tex/generic/hyphen/hyphen.tex",
            )

    def test_manifest_file_and_link_categories_are_disjoint(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "engine").write_bytes(b"engine")
            (root / "alias").symlink_to("engine")

            files, links = package_dist.inventory_stage(root)

            self.assertEqual(set(files), {"engine"})
            self.assertEqual(links, {"alias": "engine"})
            self.assertTrue(set(files).isdisjoint(links))


class FormatValidationTests(unittest.TestCase):
    def test_raw_version_8_remains_accepted(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "legacy.fmt"
            path.write_bytes(b"RUSTEXFM" + (8).to_bytes(2, "little") + (7).to_bytes(2, "little"))
            self.assertEqual(package_dist.validate_format_file(str(path)), path)

    @unittest.skipUnless(shutil.which("zstd"), "zstd command is unavailable")
    def test_embedded_compressed_format_is_accepted(self) -> None:
        path = package_dist.REPO / "crates" / "tex-cli" / "assets" / "default.fmt.zst"
        self.assertEqual(package_dist.validate_format_file(str(path)), path)

    @unittest.skipUnless(shutil.which("zstd"), "zstd command is unavailable")
    def test_compressed_payload_is_decoded_before_validation(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "foreign.fmt.zst"
            compressed = subprocess.run(
                [shutil.which("zstd"), "-q", "-c"],
                input=b"this is not a RUSTEXFM payload",
                capture_output=True,
                check=True,
            ).stdout
            path.write_bytes(compressed)
            with contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    package_dist.validate_format_file(str(path))

    def test_compressed_validation_reports_a_missing_decoder(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "format.fmt.zst"
            path.write_bytes(package_dist.ZSTD_MAGIC + b"placeholder")
            stderr = io.StringIO()
            with mock.patch.object(package_dist.shutil, "which", return_value=None):
                with contextlib.redirect_stderr(stderr):
                    with self.assertRaises(SystemExit):
                        package_dist.validate_format_file(str(path))
            self.assertIn("zstd command is not installed", stderr.getvalue())


class InstallerUpgradeTests(unittest.TestCase):
    def make_unix_bundle(self, root: Path) -> Path:
        bundle = root / "bundle"
        bundle_bin = bundle / "bin"
        bundle_texmf = bundle / "share" / "tex-suite" / "texmf"
        bundle_bin.mkdir(parents=True)
        asset = bundle_texmf / "tex" / "generic" / "hyphen" / "hyphen.tex"
        asset.parent.mkdir(parents=True)
        asset.write_text("% managed hyphen data\n")
        engine = bundle_bin / "pdflatex"
        engine.write_text("#!/bin/sh\nexit 0\n")
        engine.chmod(0o755)
        return bundle

    def run_linux_installer(
        self, bundle: Path, prefix: Path, data: Path | None = None, *extra: str
    ) -> subprocess.CompletedProcess:
        command = [
            "sh",
            str(package_dist.REPO / "packaging" / "install-linux.sh"),
            "--bundle",
            str(bundle),
            "--prefix",
            str(prefix),
            "--no-path",
            "--skip-verify",
        ]
        if data is not None:
            command.extend(("--data-dir", str(data)))
        command.extend(extra)
        return subprocess.run(command, capture_output=True, text=True)

    def test_linux_upgrade_removes_only_owned_legacy_format_paths(self) -> None:
        if os.name == "nt":
            self.skipTest("POSIX shell installer test")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = self.make_unix_bundle(root)

            prefix = root / "prefix"
            installed_bin = prefix / "bin"
            installed_data = prefix / "share" / "tex-suite"
            installed_bin.mkdir(parents=True)
            installed_data.mkdir(parents=True)
            data_format = installed_data / "pdflatex.fmt"
            data_format.write_bytes(b"old external format")
            (installed_bin / "pdflatex.fmt").symlink_to(data_format)
            sentinel = installed_data / "keep.me"
            sentinel.write_bytes(b"unrelated")
            (installed_data / ".tex-suite-managed-files-v1").write_text(
                "TEX-SUITE-MANAGED-FILES-1\n"
                f"PREFIX\t{prefix}\n"
                f"DATA\t{installed_data}\n"
                "B\tpdflatex.fmt\n"
                "F\tpdflatex.fmt\n"
            )

            result = self.run_linux_installer(bundle, prefix)
            self.assertEqual(result.returncode, 0, result.stderr)

            self.assertFalse((installed_bin / "pdflatex.fmt").exists())
            self.assertFalse((installed_bin / "pdflatex.fmt").is_symlink())
            self.assertFalse(data_format.exists())
            self.assertEqual(sentinel.read_bytes(), b"unrelated")

    def test_linux_install_refuses_unowned_texmf_collision_before_copying(self) -> None:
        if os.name == "nt":
            self.skipTest("POSIX shell installer test")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = self.make_unix_bundle(root)
            prefix = root / "prefix"
            data = root / "custom-data"
            collision = data / "texmf" / "tex" / "generic" / "hyphen" / "hyphen.tex"
            collision.parent.mkdir(parents=True)
            collision.write_text("user sentinel\n")
            unrelated = data / "keep.me"
            unrelated.write_text("unrelated\n")

            result = self.run_linux_installer(bundle, prefix, data)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("refusing to overwrite unowned path", result.stderr)
            self.assertEqual(collision.read_text(), "user sentinel\n")
            self.assertEqual(unrelated.read_text(), "unrelated\n")
            self.assertFalse((prefix / "bin" / "pdflatex").exists())

    def test_linux_uninstall_removes_manifest_files_and_preserves_unrelated(self) -> None:
        if os.name == "nt":
            self.skipTest("POSIX shell installer test")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = self.make_unix_bundle(root)
            prefix = root / "prefix"
            data = root / "custom-data"
            data.mkdir()
            unrelated = data / "keep.me"
            unrelated.write_text("unrelated\n")

            install = self.run_linux_installer(bundle, prefix, data)
            self.assertEqual(install.returncode, 0, install.stderr)
            managed = data / "texmf" / "tex" / "generic" / "hyphen" / "hyphen.tex"
            self.assertTrue(managed.is_file())
            nested_unrelated = data / "texmf" / "local-user.tex"
            nested_unrelated.write_text("user data\n")

            uninstall = self.run_linux_installer(
                bundle, prefix, data, "--uninstall"
            )

            self.assertEqual(uninstall.returncode, 0, uninstall.stderr)
            self.assertFalse(managed.exists())
            self.assertFalse((prefix / "bin" / "pdflatex").exists())
            self.assertEqual(unrelated.read_text(), "unrelated\n")
            self.assertEqual(nested_unrelated.read_text(), "user data\n")
            self.assertTrue(data.is_dir())

    def test_linux_upgrade_removes_dropped_managed_files(self) -> None:
        if os.name == "nt":
            self.skipTest("POSIX shell installer test")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = self.make_unix_bundle(root)
            prefix = root / "prefix"
            data = root / "data"
            first = self.run_linux_installer(bundle, prefix, data)
            self.assertEqual(first.returncode, 0, first.stderr)
            managed = data / "texmf" / "tex" / "generic" / "hyphen" / "hyphen.tex"
            self.assertTrue(managed.is_file())
            unrelated = data / "keep.me"
            unrelated.write_text("unrelated\n")
            (bundle / "share" / "tex-suite" / "texmf" / "tex" / "generic" / "hyphen" / "hyphen.tex").unlink()

            second = self.run_linux_installer(bundle, prefix, data)

            self.assertEqual(second.returncode, 0, second.stderr)
            self.assertFalse(managed.exists())
            self.assertEqual(unrelated.read_text(), "unrelated\n")

    def test_linux_install_refuses_unrecognized_manifest(self) -> None:
        if os.name == "nt":
            self.skipTest("POSIX shell installer test")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = self.make_unix_bundle(root)
            prefix = root / "prefix"
            data = root / "data"
            data.mkdir()
            marker = data / ".tex-suite-managed-files-v1"
            marker.write_text("not an ownership manifest\n")

            result = self.run_linux_installer(bundle, prefix, data)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("unrecognized ownership manifest", result.stderr)
            self.assertEqual(marker.read_text(), "not an ownership manifest\n")
            self.assertFalse((prefix / "bin" / "pdflatex").exists())

    def test_windows_uninstall_never_recursively_deletes_install_root(self) -> None:
        script = (package_dist.REPO / "packaging" / "install-windows.ps1").read_text()
        self.assertIn("Assert-OwnedDestinations", script)
        self.assertIn("Refusing an unrecognized install manifest", script)
        self.assertIn("Refusing to overwrite unowned path", script)
        self.assertIn("Remove-ManagedInstallFiles", script)
        self.assertIn("tex-suite-install-v2", script)
        self.assertNotRegex(
            script,
            r"Remove-Item\s+-LiteralPath\s+\$Root\s+-Recurse",
        )

    def test_macos_quarantine_cleanup_never_recurses_through_custom_data(self) -> None:
        script = (package_dist.REPO / "packaging" / "install-macos.sh").read_text()
        self.assertNotIn('xattr -dr com.apple.quarantine "$DATA_DIR"', script)
        self.assertIn('[ ! -L "$BIN_DIR/$_t" ]', script)
        self.assertIn('done < "$DATA_MANIFEST"', script)


if __name__ == "__main__":
    unittest.main()
