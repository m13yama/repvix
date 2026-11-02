"""Exercise the real installer with local release assets and a fake curl."""

import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
RELEASES = "https://github.com/m13yama/repvix/releases"
PAYLOAD = b"#!/bin/sh\nprintf 'repvix fixture\\n'\n"


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="repvix installer ")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bin = self.root / "bin"
        self.assets = self.root / "assets"
        self.destination = self.root / "installed bin"
        self.scratch = self.root / "scratch"
        for directory in (self.bin, self.assets, self.scratch):
            directory.mkdir()
        # A controlled PATH also lets us exercise macOS's shasum fallback.
        for command in ("cat", "tar", "gzip", "mktemp", "mkdir", "rm", "install", "mv",
                        "sha256sum", "shasum"):
            executable = shutil.which(command)
            if executable:
                (self.bin / command).symlink_to(executable)
        self.env = os.environ.copy()
        for key in ("REPVIX_VERSION", "REPVIX_INSTALL_DIR"):
            self.env.pop(key, None)
        self.env.update({
            "PATH": str(self.bin),
            "TMPDIR": str(self.scratch),
            "REPVIX_INSTALL_DIR": str(self.destination),
            "FIXTURE_ROOT": str(self.root),
            "FIXTURE_OS": "Linux",
            "FIXTURE_ARCH": "x86_64",
            "FIXTURE_TAG": "v9.8.7",
        })
        self.write_command("uname", """import os, sys
print(os.environ['FIXTURE_OS' if sys.argv[1] == '-s' else 'FIXTURE_ARCH'])
""")
        self.write_command("curl", """import json, os, pathlib, shutil, sys
root = pathlib.Path(os.environ['FIXTURE_ROOT'])
args = sys.argv[1:]
url = args[-1]
with (root / 'requests').open('a') as log:
    log.write(json.dumps(args) + '\\n')
if os.environ.get('FIXTURE_FAIL') and url.endswith(os.environ['FIXTURE_FAIL']):
    sys.exit(22)
base = 'https://github.com/m13yama/repvix/releases'
if url == base + '/latest':
    print(base + '/tag/' + os.environ['FIXTURE_TAG'], end='')
else:
    prefix = base + '/download/' + os.environ['FIXTURE_TAG'] + '/'
    if not url.startswith(prefix):
        sys.exit('Unexpected download URL: ' + url)
    source = root / 'assets' / url[len(prefix):]
    if not source.is_file():
        sys.exit(22)
    shutil.copyfile(source, args[args.index('--output') + 1])
""")
        self.asset = self.make_archive("x86_64-unknown-linux-musl")

    def write_command(self, name, source):
        path = self.bin / name
        path.write_text(f"#!{sys.executable}\n{source}")
        path.chmod(0o755)

    def make_archive(self, target, member="repvix", symlink=False):
        archive = self.assets / f"repvix-{target}.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            info = tarfile.TarInfo(member)
            info.mode = 0o755
            if symlink:
                info.type = tarfile.SYMTYPE
                info.linkname = "missing-target"
                tar.addfile(info)
            else:
                info.size = len(PAYLOAD)
                tar.addfile(info, io.BytesIO(PAYLOAD))
        self.checksum(archive)
        return archive

    def checksum(self, archive):
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        Path(f"{archive}.sha256").write_text(f"{digest}  {archive.name}\n")

    def run_installer(self, *args, success=True):
        # Feed the script over stdin, just like curl | bash (also uses Bash 3.2 on macOS).
        result = subprocess.run(
            ["/bin/bash", "-s", "--", *args], input=(ROOT / "install.sh").read_text(),
            env=self.env, text=True, capture_output=True, timeout=20,
        )
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        self.assertEqual(list(self.scratch.iterdir()), [], "temporary downloads leaked")
        self.assertEqual(list(self.destination.glob(".repvix.*")), [], "staging file leaked")
        return result

    def requests(self):
        log = self.root / "requests"
        return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []

    def existing_binary(self):
        self.destination.mkdir(exist_ok=True)
        binary = self.destination / "repvix"
        binary.write_bytes(b"existing installation")
        return binary

    def test_latest_and_all_platforms(self):
        for system, machine, target in (
            ("Linux", "x86_64", "x86_64-unknown-linux-musl"),
            ("Linux", "aarch64", "aarch64-unknown-linux-musl"),
            ("Darwin", "x86_64", "x86_64-apple-darwin"),
            ("Darwin", "arm64", "aarch64-apple-darwin"),
        ):
            with self.subTest(system=system, machine=machine):
                self.env.update(FIXTURE_OS=system, FIXTURE_ARCH=machine)
                archive = self.make_archive(target)
                result = self.run_installer()
                binary = self.destination / "repvix"
                self.assertEqual(binary.read_bytes(), PAYLOAD)
                self.assertTrue(os.access(binary, os.X_OK))
                self.assertIn("export PATH=", result.stdout)
                self.assertEqual([args[-1] for args in self.requests()[-3:]], [
                    f"{RELEASES}/latest",
                    f"{RELEASES}/download/v9.8.7/{archive.name}",
                    f"{RELEASES}/download/v9.8.7/{archive.name}.sha256",
                ])

    def test_explicit_version_and_directory_override_environment(self):
        self.env["REPVIX_VERSION"] = "v0.0.1"
        destination = self.root / "custom bin"
        self.run_installer("--version", "9.8.7", "--install-dir", str(destination))
        self.assertEqual((destination / "repvix").read_bytes(), PAYLOAD)
        self.assertFalse(self.destination.exists())
        self.assertEqual(len(self.requests()), 2)

    def test_environment_version_and_prerelease(self):
        self.env.update(REPVIX_VERSION="v9.8.7-rc.1", FIXTURE_TAG="v9.8.7-rc.1")
        self.run_installer()
        self.assertEqual(len(self.requests()), 2)

    def test_shasum_fallback(self):
        if not (self.bin / "shasum").exists():
            self.skipTest("shasum not available")
        (self.bin / "sha256sum").unlink(missing_ok=True)
        self.run_installer()

    def test_missing_checksum_tool(self):
        for command in ("sha256sum", "shasum"):
            (self.bin / command).unlink(missing_ok=True)
        result = self.run_installer(success=False)
        self.assertIn("requires sha256sum or shasum", result.stderr)
        self.assertEqual(self.requests(), [])

    def test_failed_downloads_preserve_existing_installation(self):
        binary = self.existing_binary()
        for suffix in ("/latest", ".tar.gz", ".sha256"):
            with self.subTest(suffix=suffix):
                self.env["FIXTURE_FAIL"] = suffix
                self.run_installer(success=False)
                self.assertEqual(binary.read_bytes(), b"existing installation")

    def test_checksum_mismatch_preserves_existing_installation(self):
        binary = self.existing_binary()
        self.asset.write_bytes(self.asset.read_bytes() + b"corrupt")
        result = self.run_installer(success=False)
        self.assertIn("checksum mismatch", result.stderr)
        self.assertEqual(binary.read_bytes(), b"existing installation")

    def test_invalid_checksum_file(self):
        for contents in ("", "invalid\n", "0" * 64 + "  other-file\n"):
            with self.subTest(contents=contents):
                Path(f"{self.asset}.sha256").write_text(contents)
                result = self.run_installer(success=False)
                self.assertIn("invalid checksum file", result.stderr)
                self.assertFalse(self.destination.exists())

    def test_bad_archives_preserve_existing_installation(self):
        binary = self.existing_binary()
        for case in ("corrupt", "missing binary", "symlink"):
            with self.subTest(case=case):
                if case == "corrupt":
                    self.asset.write_bytes(b"not a tar archive")
                    self.checksum(self.asset)
                else:
                    self.make_archive("x86_64-unknown-linux-musl",
                                      member="other" if case == "missing binary" else "repvix",
                                      symlink=case == "symlink")
                self.run_installer(success=False)
                self.assertEqual(binary.read_bytes(), b"existing installation")

    def test_upgrade_replaces_symlink_without_changing_its_target(self):
        original = self.root / "other installation"
        original.write_bytes(b"keep me")
        self.destination.mkdir()
        binary = self.destination / "repvix"
        binary.symlink_to(original)
        self.run_installer()
        self.assertFalse(binary.is_symlink())
        self.assertEqual(binary.read_bytes(), PAYLOAD)
        self.assertEqual(original.read_bytes(), b"keep me")

    def test_invalid_destination(self):
        self.destination.write_text("a file, not a directory")
        self.run_installer(success=False)
        self.assertEqual(self.destination.read_text(), "a file, not a directory")

    def test_rejects_unsupported_platforms_before_downloading(self):
        for system, machine in (("FreeBSD", "x86_64"), ("Linux", "armv7l")):
            with self.subTest(system=system, machine=machine):
                self.env.update(FIXTURE_OS=system, FIXTURE_ARCH=machine)
                result = self.run_installer(success=False)
                self.assertIn("unsupported", result.stderr)
                self.assertEqual(self.requests(), [])

    def test_help_and_invalid_arguments_do_not_download(self):
        self.assertIn("Usage:", self.run_installer("--help").stdout)
        for args in (("--unknown",), ("--version",), ("--install-dir", ""),
                     ("--version", "../../invalid")):
            with self.subTest(args=args):
                self.run_installer(*args, success=False)
        self.assertEqual(self.requests(), [])

    @unittest.skipUnless(os.environ.get("REPVIX_TEST_ARCHIVE"), "no native release archive supplied")
    def test_native_release_archive(self):
        archive = Path(os.environ["REPVIX_TEST_ARCHIVE"])
        shutil.copyfile(archive, self.assets / archive.name)
        shutil.copyfile(f"{archive}.sha256", self.assets / f"{archive.name}.sha256")
        self.env.update(FIXTURE_OS=platform.system(), FIXTURE_ARCH=platform.machine())
        self.run_installer()
        version = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M)[1]
        result = subprocess.run([self.destination / "repvix", "--version"],
                                text=True, capture_output=True, check=True)
        self.assertEqual(result.stdout.strip(), f"repvix {version}")


if __name__ == "__main__":
    unittest.main()
