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
import build_linux_packages  # noqa: E402
import package_dist  # noqa: E402


class PackageLayoutTests(unittest.TestCase):
    def make_release(self, root: Path, windows: bool) -> None:
        for index, name in enumerate(package_dist.BINARIES):
            path = root / package_dist.exe(name, windows)
            path.write_bytes(f"canonical-{index}".encode())

    def test_windows_staging_needs_only_the_shipped_binaries(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            release, stage = root / "release", root / "stage"
            release.mkdir()
            self.make_release(release, True)

            package_dist.collect_binaries(release, stage, True)

            self.assertEqual(
                sorted(path.name for path in stage.iterdir()),
                sorted(package_dist.exe(name, True) for name in package_dist.BINARIES),
            )

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


    def test_manifest_file_and_link_categories_are_disjoint(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "engine").write_bytes(b"engine")
            (root / "alias").symlink_to("engine")

            files, links = package_dist.inventory_stage(root)

            self.assertEqual(set(files), {"engine"})
            self.assertEqual(links, {"alias": "engine"})
            self.assertTrue(set(files).isdisjoint(links))


class LinuxPackageOwnershipTests(unittest.TestCase):
    def make_stage(self, root: Path) -> Path:
        stage = root / "stage"
        engine = stage / "usr" / "bin" / "ratex"
        engine.parent.mkdir(parents=True)
        engine.write_bytes(b"engine")
        engine.chmod(0o755)
        return stage

    def assert_root_owned(self, members: list) -> None:
        self.assertTrue(members)
        for member in members:
            self.assertEqual((member.uid, member.gid), (0, 0), member.name)

    @unittest.skipUnless(shutil.which("zstd") and shutil.which("tar"), "zstd/tar unavailable")
    def test_arch_package_payload_is_root_owned(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            package = build_linux_packages.build_arch_pkg(self.make_stage(root), root, "1.0")
            payload = subprocess.run(
                ["zstd", "-qdc", str(package)], capture_output=True, check=True
            ).stdout
            with tarfile.open(fileobj=io.BytesIO(payload)) as archive:
                self.assert_root_owned(archive.getmembers())

    @unittest.skipUnless(shutil.which("ar"), "ar unavailable")
    def test_deb_built_without_dpkg_deb_is_root_owned(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            with mock.patch.object(build_linux_packages.shutil, "which", return_value=None):
                package = build_linux_packages.build_deb(self.make_stage(root), root, "1.0")
            for member in ("control.tar.gz", "data.tar.xz"):
                payload = subprocess.run(
                    ["ar", "p", str(package), member], capture_output=True, check=True
                ).stdout
                with tarfile.open(fileobj=io.BytesIO(payload)) as archive:
                    self.assert_root_owned(archive.getmembers())


class FormatValidationTests(unittest.TestCase):
    def test_raw_version_8_remains_accepted(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "legacy.fmt"
            path.write_bytes(b"RUSTEXFM" + (8).to_bytes(2, "little") + (8).to_bytes(2, "little"))
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
        engine = bundle_bin / "ratex"
        engine.write_text("#!/bin/sh\necho 'ratex 0.0 (test)'\n")
        engine.chmod(0o755)
        return bundle

    def run_installer(
        self, script: str, root: Path, *args: str, env: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess:
        """Run a POSIX installer with HOME confined to ROOT/home."""
        home = root / "home"
        home.mkdir(exist_ok=True)
        environment = {
            key: value for key, value in os.environ.items() if key not in ("TEX_SUITE_DATA", "SUDO_USER")
        }
        environment["HOME"] = str(home)
        environment.update(env or {})
        command = ["sh", str(package_dist.REPO / "packaging" / script), *args]
        return subprocess.run(command, capture_output=True, text=True, env=environment)

    def run_linux_installer(
        self, bundle: Path, prefix: Path, data: Path | None = None, *extra: str
    ) -> subprocess.CompletedProcess:
        command = ["--bundle", str(bundle), "--prefix", str(prefix), "--no-path", "--skip-verify"]
        if data is not None:
            command.extend(("--data-dir", str(data)))
        command.extend(extra)
        return self.run_installer("install-linux.sh", bundle.parent, *command)

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
            self.assertFalse((prefix / "bin" / "ratex").exists())

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
            self.assertFalse((prefix / "bin" / "ratex").exists())
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
            self.assertFalse((prefix / "bin" / "ratex").exists())

    def test_linux_profile_block_keeps_prefix_literal(self) -> None:
        if os.name == "nt":
            self.skipTest("POSIX shell installer test")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = self.make_unix_bundle(root)
            marker = root / "executed"
            prefix = root / f"it's a \"pre$(touch {marker})fix`touch {marker}`"
            profile = root / "home" / ".profile"

            install = self.run_installer(
                "install-linux.sh", root, "--bundle", str(bundle), "--prefix", str(prefix), "--skip-verify"
            )
            self.assertEqual(install.returncode, 0, install.stderr)
            sourced = subprocess.run(
                ["sh", "-c", '. "$1" && printf "%s\\n%s" "$PATH" "$TEXMFLOCAL"', "sh", str(profile)],
                capture_output=True,
                text=True,
                env={"PATH": "/usr/bin:/bin", "HOME": str(root / "home")},
            )

            self.assertEqual(sourced.returncode, 0, sourced.stderr)
            path, texmflocal = sourced.stdout.split("\n")
            self.assertEqual(path, f"{prefix}/bin:/usr/bin:/bin")
            self.assertEqual(texmflocal, f"{prefix}/share/tex-suite/texmf")
            self.assertFalse(marker.exists())

    def test_linux_uninstall_removes_only_its_own_profile_block(self) -> None:
        if os.name == "nt":
            self.skipTest("POSIX shell installer test")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = self.make_unix_bundle(root)
            home = root / "home"
            home.mkdir()
            dotfile = home / "dotfiles-bashrc"
            dotfile.write_text("user=1\n")
            (home / ".bashrc").symlink_to(dotfile)
            active, scratch = root / "active", root / "scratch"
            for args in (
                ("--bundle", str(bundle), "--prefix", str(active), "--skip-verify"),
                ("--bundle", str(bundle), "--prefix", str(scratch), "--no-path", "--skip-verify"),
                ("--uninstall", "--prefix", str(scratch)),
            ):
                result = self.run_installer("install-linux.sh", root, *args)
                self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn(f"{active}/bin", dotfile.read_text())

            result = self.run_installer("install-linux.sh", root, "--uninstall", "--prefix", str(active))

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((home / ".bashrc").is_symlink())
            self.assertEqual(dotfile.read_text(), "user=1\n")

    def test_linux_uninstall_keeps_unterminated_foreign_block(self) -> None:
        if os.name == "nt":
            self.skipTest("POSIX shell installer test")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            home = root / "home"
            home.mkdir()
            text = "a=1\n# >>> tex-suite >>>\nexport PATH=\"/elsewhere/bin:$PATH\"\nuser_line=2\n"
            (home / ".profile").write_text(text)

            result = self.run_installer("install-linux.sh", root, "--uninstall")

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((home / ".profile").read_text(), text)

    def test_linux_explicit_prefix_ignores_exported_data_dir(self) -> None:
        if os.name == "nt":
            self.skipTest("POSIX shell installer test")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = self.make_unix_bundle(root)
            first = self.run_linux_installer(bundle, root / "first")
            self.assertEqual(first.returncode, 0, first.stderr)

            second = self.run_installer(
                "install-linux.sh", root, "--bundle", str(bundle), "--prefix", str(root / "second"),
                "--no-path", "--skip-verify",
                env={"TEX_SUITE_DATA": str(root / "first" / "share" / "tex-suite")},
            )

            self.assertEqual(second.returncode, 0, second.stderr)
            self.assertTrue((root / "second" / "share" / "tex-suite" / "texmf").is_dir())

    def test_linux_uninstall_removes_directories_it_created(self) -> None:
        if os.name == "nt":
            self.skipTest("POSIX shell installer test")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = self.make_unix_bundle(root)
            fresh = root / "fresh prefix"
            existing = root / "existing"
            (existing / "bin").mkdir(parents=True)
            for prefix in (fresh, existing):
                for _ in range(2):  # a reinstall must keep the created-directory records
                    result = self.run_linux_installer(bundle, prefix)
                    self.assertEqual(result.returncode, 0, result.stderr)
                result = self.run_linux_installer(bundle, prefix, None, "--uninstall")
                self.assertEqual(result.returncode, 0, result.stderr)

            self.assertFalse(fresh.exists())
            self.assertEqual(sorted(p.name for p in existing.iterdir()), ["bin"])

    def test_linux_link_install_from_relative_bundle(self) -> None:
        if os.name == "nt":
            self.skipTest("POSIX shell installer test")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self.make_unix_bundle(root)
            prefix = root / "prefix"
            home = root / "home"
            home.mkdir()

            result = subprocess.run(
                ["sh", str(package_dist.REPO / "packaging" / "install-linux.sh"), "--bundle", "bundle",
                 "--prefix", str(prefix), "--no-path", "--link"],
                capture_output=True, text=True, cwd=root, env={**os.environ, "HOME": str(home)},
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((prefix / "bin" / "ratex").exists())

    def test_linux_unsafe_payload_directory_fails_before_install(self) -> None:
        if os.name == "nt":
            self.skipTest("POSIX shell installer test")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = self.make_unix_bundle(root)
            (bundle / "share" / "tex-suite" / "texmf" / "bad\tdir").mkdir()
            prefix = root / "prefix"

            result = self.run_linux_installer(bundle, prefix)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("unsafe texmf payload directory", result.stderr)
            self.assertFalse((prefix / "bin" / "ratex").exists())

    def test_macos_uninstall_keeps_other_prefix_app_support_data(self) -> None:
        if os.name == "nt":
            self.skipTest("POSIX shell installer test")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = self.make_unix_bundle(root)
            stub = root / "stub"
            stub.mkdir()
            uname = stub / "uname"
            uname.write_text('#!/bin/sh\ncase "$1" in -s) echo Darwin;; *) echo arm64;; esac\n')
            uname.chmod(0o755)
            env = {"PATH": f"{stub}{os.pathsep}{os.environ.get('PATH', '')}"}
            app_support = root / "home" / "Library" / "Application Support" / "tex-suite"
            install = self.run_installer(
                "install-macos.sh", root, "--bundle", str(bundle), "--app-support",
                "--prefix", str(root / "owner"), "--no-path", "--skip-verify", env=env,
            )
            self.assertEqual(install.returncode, 0, install.stderr)

            other = self.run_installer("install-macos.sh", root, "--uninstall", "--prefix", str(root / "other"), env=env)

            self.assertEqual(other.returncode, 0, other.stderr)
            self.assertTrue((app_support / "texmf" / "tex" / "generic" / "hyphen" / "hyphen.tex").is_file())
            own = self.run_installer("install-macos.sh", root, "--uninstall", "--prefix", str(root / "owner"), env=env)
            self.assertEqual(own.returncode, 0, own.stderr)
            self.assertFalse(app_support.exists())
            self.assertFalse((root / "owner").exists())



if __name__ == "__main__":
    unittest.main()
