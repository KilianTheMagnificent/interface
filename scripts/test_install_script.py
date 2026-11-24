"""install.sh puts nothing on the Mac unless the download is Interface signed by its maintainer.

    python3 -m unittest scripts/test_install_script.py
"""
from __future__ import annotations
import base64
import functools
import hashlib
import http.server
import json
import os
import platform
import plistlib
import shutil
import subprocess
import tempfile
import threading
import unittest
from pathlib import Path

INSTALLER = Path(__file__).resolve().parent.parent / "install.sh"
VERSION = "9.9.9"
ARCHIVE = f"Interface-{VERSION}-arm64-mac.zip"


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


@unittest.skipUnless(platform.system() == "Darwin" and platform.machine() == "arm64",
                     "the installer only runs on an Apple Silicon Mac")
class InstallScriptTests(unittest.TestCase):
    def setUp(self):
        self.directory = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.served = self.directory / "served"
        self.applications = self.directory / "Applications"
        self.served.mkdir()
        self.applications.mkdir()
        handler = functools.partial(QuietHandler, directory=str(self.served))
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        self.address = f"http://127.0.0.1:{self.server.server_address[1]}"

    def make_app(self, sign: str | None) -> Path:
        """A stand-in Interface.app: unsigned, or signed by somebody who is not the maintainer."""
        app = self.directory / "build/Interface.app"
        (app / "Contents/MacOS").mkdir(parents=True)
        shutil.copy("/usr/bin/true", app / "Contents/MacOS/Interface")
        with open(app / "Contents/Info.plist", "wb") as handle:
            plistlib.dump({"CFBundleIdentifier": "com.andreromero.interface", "CFBundleExecutable": "Interface",
                           "CFBundleShortVersionString": VERSION, "CFBundlePackageType": "APPL",
                           "LSMinimumSystemVersion": "12.0"}, handle)
        if sign is None:
            subprocess.run(["codesign", "--remove-signature", app / "Contents/MacOS/Interface"],
                           check=True, capture_output=True)
        else:
            subprocess.run(["codesign", "--force", "--sign", sign, app], check=True, capture_output=True)
        return app

    def publish(self, app: Path, *, sha512: str | None = None, name: str = ARCHIVE,
                product: str = "com.andreromero.interface", version: str = VERSION) -> None:
        archive = self.served / ARCHIVE
        subprocess.run(["ditto", "-c", "-k", "--keepParent", app, archive], check=True)
        actual = base64.b64encode(hashlib.sha512(archive.read_bytes()).digest()).decode()
        payload = json.dumps({"schema": 1, "product": product, "channel": "stable", "version": version,
                              "artifact": {"name": name, "size": archive.stat().st_size,
                                           "sha512": sha512 or actual}}).encode()
        (self.served / "interface-update.json").write_text(json.dumps(
            {"schema": 1, "payload": base64.b64encode(payload).decode(), "signatures": []}))

    def install(self) -> subprocess.CompletedProcess:
        environment = dict(os.environ, INTERFACE_RELEASE_FEED=self.address,
                           INTERFACE_INSTALL_DIR=str(self.applications), INTERFACE_NO_OPEN="1")
        return subprocess.run(["/bin/bash", str(INSTALLER)], env=environment, capture_output=True, text=True)

    def assert_refused(self, result: subprocess.CompletedProcess, reason: str) -> None:
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(reason, result.stderr)
        self.assertEqual(list(self.applications.iterdir()), [], "a refused download must leave nothing behind")

    def test_a_build_signed_by_someone_else_is_not_installed(self):
        self.publish(self.make_app(sign="-"))
        self.assert_refused(self.install(), "not signed by Interface's maintainer")

    def test_an_unsigned_build_is_not_installed(self):
        self.publish(self.make_app(sign=None))
        self.assert_refused(self.install(), "signature is broken")

    def test_a_download_that_does_not_match_its_checksum_is_not_installed(self):
        self.publish(self.make_app(sign="-"), sha512=base64.b64encode(bytes(64)).decode())
        self.assert_refused(self.install(), "does not match the checksum")

    def test_a_release_naming_another_download_is_refused(self):
        self.publish(self.make_app(sign="-"), name="../../Other.zip")
        self.assert_refused(self.install(), "unexpected download")

    def test_a_release_for_another_product_is_refused(self):
        self.publish(self.make_app(sign="-"), product="com.example.other")
        self.assert_refused(self.install(), "not Interface's")

    def test_a_feed_with_nothing_published_is_refused(self):
        self.assert_refused(self.install(), "no release could be read")

    def test_the_real_feed_and_signer_are_the_maintainers(self):
        text = INSTALLER.read_text()
        self.assertIn('RELEASES="KilianTheMagnificent/interface-releases"', text)
        self.assertIn('TEAM_ID="36B25Z964W"', text)
        self.assertIn('BUNDLE_ID="com.andreromero.interface"', text)
        channel = json.loads((INSTALLER.parent / "crates/interface-updater/release-channel.json").read_text())
        self.assertEqual(channel["feeds"], [
            "https://github.com/KilianTheMagnificent/interface-releases/releases/latest/download/interface-update.json"])
        self.assertEqual(channel["product"], "com.andreromero.interface")


if __name__ == "__main__":
    unittest.main()
