#!/usr/bin/env python3
"""Unit tests for the Release packaging and verification scripts.

Run with: python3 -m unittest discover -s tools -p "test_*.py"
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import package_updater_release as packaging  # noqa: E402
import verify_release_assets as verify  # noqa: E402

CERT = packaging.ANDROID_CERT_SHA256
BADGING = (
    "package: name='app.norishell.android' versionCode='1008' versionName='0.1.8' "
    "platformBuildVersionName='16' platformBuildVersionCode='36'\n"
    "sdkVersion:'29'\n"
    "targetSdkVersion:'36'\n"
    "native-code: 'arm64-v8a'\n"
)


class ReleaseMatrixTest(unittest.TestCase):
    def test_matrix_requires_the_android_apk(self) -> None:
        names = verify.expected_names("0.1.8", "0.1.7")
        self.assertEqual(len(names), packaging.EXPECTED_ASSET_COUNT)
        self.assertIn("NoriShell_0.1.8_android_arm64.apk", names)
        self.assertIn("latest.json", names)
        self.assertFalse(any(name.endswith(".sig") for name in names))
        # 15 user download packages: everything except updater archives and the manifest.
        packages = {n for n in names if not n.endswith(".app.tar.gz") and n != "latest.json"}
        self.assertEqual(len(packages), 15)


class AndroidApkChecksTest(unittest.TestCase):
    def test_version_code_follows_tauri(self) -> None:
        self.assertEqual(packaging.android_version_code("0.1.8"), 1008)
        self.assertEqual(packaging.android_version_code("1.2.3"), 1_002_003)
        with self.assertRaises(ValueError):
            packaging.android_version_code("0.1.8-beta.1")

    def test_release_badging_is_accepted(self) -> None:
        packaging.check_android_badging(BADGING, "0.1.8")

    def test_wrong_version_package_or_debuggable_is_rejected(self) -> None:
        cases = [
            BADGING.replace("versionName='0.1.8'", "versionName='0.1.7'"),
            BADGING.replace("versionCode='1008'", "versionCode='1007'"),
            BADGING.replace("app.norishell.android", "app.norishell.debug"),
            BADGING + "application-debuggable\n",
            BADGING.replace("'arm64-v8a'", "'arm64-v8a' 'x86_64'"),
        ]
        for badging in cases:
            with self.subTest(badging=badging), self.assertRaises(ValueError):
                packaging.check_android_badging(badging, "0.1.8")

    def test_only_the_pinned_release_certificate_is_accepted(self) -> None:
        line = "Signer #1 certificate SHA-256 digest: {}\n"
        packaging.check_android_certificates(line.format(CERT))
        debug = "0" * 64
        for certs in (line.format(debug), "", line.format(CERT) + line.replace("#1", "#2").format(debug)):
            with self.subTest(certs=certs), self.assertRaises(ValueError):
                packaging.check_android_certificates(certs)


if __name__ == "__main__":
    unittest.main()
