#!/usr/bin/env python3
"""Stage signed Tauri update assets and the manifest for all Release targets.

Updater platforms: macOS arm64/x64 (.app.tar.gz), Windows x64/arm64 (NSIS setup.exe)
and Linux x64/arm64 (AppImage, Preview). Linux .deb files are manual downloads and
are never signed. Signatures live only inside latest.json; no .sig file is ever
written to (or tolerated in) the upload stage. Linux build metadata JSON files are
read from --linux-metadata-dir and are not Release assets.
"""

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
    "linux-x86_64": ("linux_x64", None),
    "linux-aarch64": ("linux_arm64", None),
}
LINUX_ARCHES = {"x64": "linux_x64", "arm64": "linux_arm64"}
EXPECTED_ASSET_COUNT = 17


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def verify_signature(asset: Path, signature: str, public_key: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="norishell-update-verify-") as directory:
        tmp = Path(directory)
        key = tmp / "public.key"
        sig = tmp / "signature"
        key.write_bytes(base64.b64decode(public_key.read_text().strip(), validate=True))
        sig.write_bytes(base64.b64decode(signature.strip(), validate=True))
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
        signature = base64.b64encode(raw).decode()
    verify_signature(asset, signature, public_key)
    return signature


def mac_executable_in_tar(archive: Path) -> bytes:
    with tarfile.open(archive, "r:gz") as contents:
        matches = [member for member in contents.getmembers() if member.name.endswith("NoriShell.app/Contents/MacOS/norishell")]
        if len(matches) != 1 or not matches[0].isfile():
            raise ValueError(f"updater archive has no single NoriShell executable: {archive}")
        stream = contents.extractfile(matches[0])
        if stream is None:
            raise ValueError(f"unable to read executable: {archive}")
        return stream.read()


def check_linux_metadata(stage: Path, metadata_dir: Path, version: str) -> None:
    """Bind the staged Linux packages to the metadata written by the Linux builder."""
    for arch, name in LINUX_ARCHES.items():
        meta_file = metadata_dir / f"linux-build-{arch}.json"
        if not meta_file.is_file():
            raise ValueError(f"missing Linux build metadata: {meta_file}")
        meta = json.loads(meta_file.read_text())
        if meta.get("schema") != 1:
            raise ValueError(f"unsupported Linux build metadata schema: {meta_file}")
        if meta.get("version") != version:
            raise ValueError(f"Linux build metadata version differs from {version}: {meta_file}")
        if meta.get("arch") != arch:
            raise ValueError(f"Linux build metadata architecture is not {arch}: {meta_file}")
        if meta.get("dirty") is not False:
            raise ValueError(f"Linux build came from a dirty working tree: {meta_file}")
        for extension, key in (("AppImage", "appImageSha256"), ("deb", "debSha256")):
            package = stage / f"NoriShell_{version}_{name}.{extension}"
            if not package.is_file():
                raise ValueError(f"missing Linux package: {package}")
            if digest(package.read_bytes()) != meta.get(key):
                raise ValueError(f"Linux package differs from {meta_file} ({key}): {package}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--stage", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--private-key", type=Path, required=True)
    parser.add_argument("--linux-metadata-dir", type=Path, required=True)
    args = parser.parse_args()
    stage = args.stage.resolve(strict=True)
    stale = sorted(path.name for path in stage.iterdir() if path.name.endswith(".sig"))
    if stale:
        raise ValueError(f"signature files must not be staged for upload (remove them): {stale}")
    check_linux_metadata(stage, args.linux_metadata_dir.resolve(strict=True), args.version)
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
        elif name.startswith("linux_"):
            # The AppImage is the only Linux updater artifact; hashes were checked above.
            archive = stage / f"NoriShell_{args.version}_{name}.AppImage"
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
    if len(assets) != EXPECTED_ASSET_COUNT:
        raise ValueError(f"expected {EXPECTED_ASSET_COUNT} application, sync and updater assets; found {len(assets)}")
    (stage / "SHA256SUMS.txt").write_text(
        "".join(f"{digest(path.read_bytes())}  {path.name}\n" for path in assets)
    )
    print(json.dumps({"version": args.version, "platforms": list(platforms), "assets": len(assets)}))


if __name__ == "__main__":
    main()
