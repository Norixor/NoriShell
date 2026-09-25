#!/usr/bin/env python3
"""Verify the complete Release matrix and signed updater manifest."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import re
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
PLATFORMS = {
    "darwin-aarch64": "macos_arm64",
    "darwin-x86_64": "macos_x64",
    "windows-x86_64": "windows_x64",
    "windows-aarch64": "windows_arm64",
}


def expected_names(version: str, sync_version: str) -> set[str]:
    names = {
        f"NoriShell_{version}_{platform}.{extension}"
        for platform in ("macos_arm64", "macos_x64")
        for extension in ("dmg", "zip")
    }
    names.update(f"NoriShell_{version}_{platform}.zip" for platform in ("windows_x64", "windows_arm64"))
    names.update(f"NoriShell_{version}_{platform}-setup.exe" for platform in ("windows_x64", "windows_arm64"))
    names.update(f"NoriShell_{version}_{platform}.app.tar.gz" for platform in ("macos_arm64", "macos_x64"))
    names.update(f"NoriShell_{version}_{platform}.app.tar.gz.sig" for platform in ("macos_arm64", "macos_x64"))
    names.update(f"NoriShell_{version}_{platform}-setup.exe.sig" for platform in ("windows_x64", "windows_arm64"))
    names.update((
        f"NoriShell_SelfHostSync_Server_{sync_version}.zip",
        f"NoriShell_SelfHostSync_Plugin_{sync_version}.zip",
        "latest.json",
    ))
    assert len(names) == 17
    return names


def verify_signature(asset: Path, signature: Path, public_key: str, version: str) -> None:
    with tempfile.TemporaryDirectory(prefix="norishell-release-signature-") as directory:
        tmp = Path(directory)
        key_file = tmp / "public.key"
        signature_file = tmp / "signature"
        key_file.write_bytes(base64.b64decode(public_key, validate=True))
        raw_signature = base64.b64decode(signature.read_text().strip(), validate=True)
        signature_file.write_bytes(raw_signature)
        subprocess.run(
            ["minisign", "-V", "-q", "-p", str(key_file), "-m", str(asset), "-x", str(signature_file)],
            check=True,
        )
        lines = raw_signature.decode().splitlines()
        if len(lines) < 3 or not re.fullmatch(
            rf"trusted comment: timestamp:\d+\tfile:{re.escape(asset.name)}\tversion:{re.escape(version)}",
            lines[2],
        ):
            raise ValueError(f"signed version metadata differs: {asset.name}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--sync-version", required=True)
    args = parser.parse_args()
    assets = args.assets.resolve(strict=True)
    expected = expected_names(args.version, args.sync_version)
    actual = {path.name for path in assets.iterdir() if path.is_file()}
    if actual != expected | {"SHA256SUMS.txt"}:
        missing = sorted(expected - actual)
        extra = sorted(actual - expected - {"SHA256SUMS.txt"})
        raise ValueError(f"Release asset matrix differs: missing={missing}, extra={extra}")

    checksums: dict[str, str] = {}
    for line in (assets / "SHA256SUMS.txt").read_text().splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (\S+)", line)
        if not match or match.group(2) in checksums:
            raise ValueError("malformed or duplicate SHA256SUMS entry")
        checksums[match.group(2)] = match.group(1)
    if checksums.keys() != expected:
        raise ValueError("SHA256SUMS does not cover the exact Release matrix")
    for name, expected_digest in checksums.items():
        actual_digest = hashlib.sha256((assets / name).read_bytes()).hexdigest()
        if actual_digest != expected_digest:
            raise ValueError(f"checksum differs: {name}")

    manifest = json.loads((assets / "latest.json").read_text())
    if set(manifest) != {"version", "platforms"} or manifest["version"] != args.version:
        raise ValueError("update manifest version or shape differs")
    if set(manifest["platforms"]) != set(PLATFORMS):
        raise ValueError("update manifest platform matrix differs")
    updater = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())["plugins"]["updater"]
    if updater.get("requireSignedVersion") is not True:
        raise ValueError("the application does not require signed update versions")
    public_key = updater["pubkey"]
    base = f"https://github.com/Norixor/NoriShell/releases/download/v{args.version}/"
    for target, platform in PLATFORMS.items():
        filename = (
            f"NoriShell_{args.version}_{platform}.app.tar.gz"
            if target.startswith("darwin")
            else f"NoriShell_{args.version}_{platform}-setup.exe"
        )
        entry = manifest["platforms"][target]
        if set(entry) != {"url", "signature"} or entry["url"] != base + filename:
            raise ValueError(f"update URL differs: {target}")
        signature = assets / f"{filename}.sig"
        if entry["signature"] != signature.read_text().strip():
            raise ValueError(f"update manifest signature differs: {target}")
        verify_signature(assets / filename, signature, public_key, args.version)
    print(f"Verified 17 Release assets, SHA256SUMS and four updater signatures for v{args.version}")


if __name__ == "__main__":
    main()
