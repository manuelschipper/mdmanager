"""Exercise the installer's download verification and replacement boundary."""

import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest


INSTALLER = Path(__file__).resolve().parents[1] / "homepage/install.sh"


class InstallerTest(unittest.TestCase):
    def test_install_and_reject_unverified_replacements(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            commands = root / "commands"
            commands.mkdir()
            (commands / "curl").write_text("""#!/bin/sh
set -eu
while [ "$#" -gt 1 ]; do
    case "$1" in
        -o) output=$2; shift 2 ;;
        -w) shift 2 ;;
        *) shift ;;
    esac
done
case "$1" in
    */releases/latest) printf 'https://github.com/manuelschipper/mdmanager/releases/tag/v0.1.0' ;;
    */releases/download/v0.1.0/*) cp "$FIXTURE/${1##*/}" "$output" ;;
    *) exit 1 ;;
esac
""")
            (commands / "uname").write_text("""#!/bin/sh
case "$1" in
    -s) echo "$TEST_OS" ;;
    -m) echo "$TEST_ARCH" ;;
esac
""")
            for command in commands.iterdir():
                command.chmod(0o755)
            env = dict(os.environ, PATH=f"{commands}:{os.environ['PATH']}",
                       FIXTURE=str(root), MDMANAGER_INSTALL_DIR=str(root / "bin"))
            env.pop("MDMANAGER_VERSION", None)
            destination = root / "bin/mdmanager"
            for os_name, arch, target in [
                ("Linux", "x86_64", "x86_64-unknown-linux-musl"),
                ("Linux", "aarch64", "aarch64-unknown-linux-musl"),
                ("Darwin", "x86_64", "x86_64-apple-darwin"),
                ("Darwin", "arm64", "aarch64-apple-darwin"),
            ]:
                with self.subTest(target=target):
                    env.update(TEST_OS=os_name, TEST_ARCH=arch)
                    asset = root / f"mdmanager-{target}.tar.gz"
                    binary = b'#!/bin/sh\necho "mdmanager 0.1.0"\n'
                    with tarfile.open(asset, "w:gz") as archive:
                        entry = tarfile.TarInfo("mdmanager")
                        entry.size = len(binary)
                        entry.mode = 0o755
                        archive.addfile(entry, io.BytesIO(binary))
                    digest = hashlib.sha256(asset.read_bytes()).hexdigest()
                    checksums = root / "sha256sums.txt"
                    checksums.write_text(f"{digest}  {asset.name}\n")
                    result = subprocess.run(["sh", str(INSTALLER)], env=env, capture_output=True)
                    self.assertEqual(result.returncode, 0, result.stderr.decode())
                    self.assertEqual(destination.read_bytes(), binary)
                    self.assertTrue(os.access(destination, os.X_OK))

                    destination.write_bytes(b"existing installation")
                    for bad_checksum in ["", f"{'0' * 64}  {asset.name}\n"]:
                        checksums.write_text(bad_checksum)
                        result = subprocess.run(["sh", str(INSTALLER)], env=env, capture_output=True)
                        self.assertNotEqual(result.returncode, 0)
                        self.assertEqual(destination.read_bytes(), b"existing installation")

            env.update(TEST_OS="FreeBSD", TEST_ARCH="x86_64")
            result = subprocess.run(["sh", str(INSTALLER)], env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(destination.read_bytes(), b"existing installation")
