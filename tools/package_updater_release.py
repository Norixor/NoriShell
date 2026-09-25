#!/usr/bin/env python3
"""Stage signed Tauri update assets for the four desktop release targets."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
import time
import zipfile
from pathlib import Path

from package_windows_nsis import embedded_exe_sha256


ROOT = Path(__file__).resolve().parent.parent
PLATFORMS = {
    "darwin-aarch64": ("macos_arm64", "aarch64-apple-darwin"),
    "darwin-x86_64": ("macos_x64", "x86_64-apple-darwin"),
    "windows-x86_64": ("windows_x64", None),
    "windows-aarch64": ("windows_arm64", None),
}


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def verify_signature(asset: Path, signature: Path, public_key: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="norishell-update-verify-") as directory:
        tmp = Path(directory)
        key = tmp / "public.key"
        sig = tmp / "signature"
        key.write_bytes(base64.b64decode(public_key.read_text().strip(), validate=True))
        sig.write_bytes(base64.b64decode(signature.read_text().strip(), validate=True))
        subprocess.run(
            ["minisign", "-V", "-q", "-p", str(key), "-m", str(asset), "-x", str(sig)],
            check=True,
        )


EXPECT_SIGN = r'''log_user 0
set timeout 60
spawn minisign -S -q -s $env(NORISHELL_SIGN_KEY) -m $env(NORISHELL_SIGN_ASSET) -x $env(NORISHELL_SIGN_OUTPUT) -t $env(NORISHELL_SIGN_COMMENT)
expect {
  -re "Password:" { send "\r"; exp_continue }
  eof {}
  timeout { exit 124 }
}
set result [wait]
exit [lindex $result 3]
'''


def sign(asset: Path, key: Path, public_key: Path, version: str) -> str:
    signature = Path(f"{asset}.sig")
    signature.unlink(missing_ok=True)
    with tempfile.TemporaryDirectory(prefix="norishell-update-sign-") as directory:
        raw_key = Path(directory) / "secret.key"
        raw_signature = Path(directory) / "signature"
        raw_key.write_bytes(base64.b64decode(key.read_text().strip(), validate=True))
        raw_key.chmod(0o600)
        comment = f"timestamp:{int(time.time())}\tfile:{asset.name}\tversion:{version}"
        environment = {
            **os.environ,
            "NORISHELL_SIGN_KEY": str(raw_key),
            "NORISHELL_SIGN_ASSET": str(asset),
            "NORISHELL_SIGN_OUTPUT": str(raw_signature),
            "NORISHELL_SIGN_COMMENT": comment,
        }
        subprocess.run(["expect", "-c", EXPECT_SIGN], env=environment, check=True, capture_output=True)
        raw = raw_signature.read_bytes()
        if raw.decode().splitlines()[2] != f"trusted comment: {comment}":
            raise ValueError(f"signed version metadata differs: {asset}")
        signature.write_text(base64.b64encode(raw).decode() + "\n")
    verify_signature(asset, signature, public_key)
    return signature.read_text().strip()


def mac_executable_in_tar(archive: Path) -> bytes:
    with tarfile.open(archive, "r:gz") as contents:
        matches = [member for member in contents.getmembers() if member.name.endswith("NoriShell.app/Contents/MacOS/norishell")]
        if len(matches) != 1 or not matches[0].isfile():
            raise ValueError(f"updater archive has no single NoriShell executable: {archive}")
        stream = contents.extractfile(matches[0])
        if stream is None:
            raise ValueError(f"unable to read executable: {archive}")
        return stream.read()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--stage", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--private-key", type=Path, required=True)
    args = parser.parse_args()
    stage = args.stage.resolve(strict=True)
    key = args.private_key.resolve(strict=True)
    public_key = Path(f"{key}.pub").resolve(strict=True)
    config = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())
    if config["version"] != args.version:
        raise ValueError("application version differs from updater version")
    if config["plugins"]["updater"]["pubkey"] != public_key.read_text().strip():
        raise ValueError("the application trusts a different updater public key")
    if config["plugins"]["updater"].get("requireSignedVersion") is not True:
        raise ValueError("the application does not require signed update versions")

    release_url = f"https://github.com/Norixor/NoriShell/releases/download/v{args.version}/"
    platforms: dict[str, dict[str, str]] = {}
    for target, (name, rust_target) in PLATFORMS.items():
        if rust_target:
            source = ROOT / "target" / rust_target / "release/bundle/macos/NoriShell.app.tar.gz"
            archive = stage / f"NoriShell_{args.version}_{name}.app.tar.gz"
            shutil.copy2(source, archive)
            app_zip = stage / f"NoriShell_{args.version}_{name}.zip"
            with zipfile.ZipFile(app_zip) as contents:
                packaged_exe = contents.read("NoriShell.app/Contents/MacOS/norishell")
            if digest(mac_executable_in_tar(archive)) != digest(packaged_exe):
                raise ValueError(f"updater archive differs from application ZIP: {target}")
        else:
            archive = stage / f"NoriShell_{args.version}_{name}-setup.exe"
            app_zip = stage / f"NoriShell_{args.version}_{name}.zip"
            with zipfile.ZipFile(app_zip) as contents:
                if contents.namelist() != ["norishell.exe"]:
                    raise ValueError(f"Windows ZIP has unexpected contents: {app_zip}")
                packaged_exe_hash = digest(contents.read("norishell.exe"))
            if not archive.is_file():
                raise ValueError(f"missing Windows installer: {archive}")
            extractor = shutil.which("7zz") or shutil.which("7z")
            if not extractor or embedded_exe_sha256(archive, extractor) != packaged_exe_hash:
                raise ValueError(f"Windows installer differs from application ZIP: {target}")
        signature = sign(archive, key, public_key, args.version)
        platforms[target] = {"url": release_url + archive.name, "signature": signature}

    manifest = {"version": args.version, "platforms": platforms}
    (stage / "latest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    assets = sorted(path for path in stage.iterdir() if path.name.startswith("NoriShell_") or path.name == "latest.json")
    if len(assets) != 17:
        raise ValueError(f"expected 17 application, sync and updater assets; found {len(assets)}")
    (stage / "SHA256SUMS.txt").write_text(
        "".join(f"{digest(path.read_bytes())}  {path.name}\n" for path in assets)
    )
    print(json.dumps({"version": args.version, "platforms": list(platforms), "assets": len(assets)}))


if __name__ == "__main__":
    main()
