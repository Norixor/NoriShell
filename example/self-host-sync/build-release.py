#!/usr/bin/env python3
"""Build Release assets for the self-hosted server and plugin."""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import tempfile
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent
REPOSITORY = ROOT.parents[1]
ZIP_TIME = (2020, 1, 1, 0, 0, 0)


def add_file(archive: zipfile.ZipFile, source: Path, name: str) -> None:
    metadata = zipfile.ZipInfo(name, ZIP_TIME)
    metadata.compress_type = zipfile.ZIP_DEFLATED
    metadata.external_attr = ((0o755 if os.access(source, os.X_OK) else 0o644) & 0xFFFF) << 16
    archive.writestr(metadata, source.read_bytes())


def package_server(source_directory: Path, destination: Path) -> None:
    files = sorted(path for path in source_directory.rglob("*") if path.is_file())
    binaries = [path for path in files if path.name.startswith("norishell-sync-server_")]
    if len(binaries) != 6:
        raise RuntimeError(f"expected six server binaries, found {len(binaries)}")
    with zipfile.ZipFile(destination, "w") as archive:
        add_file(archive, ROOT / "server" / "README.md", "README.md")
        for path in files:
            add_file(archive, path, path.relative_to(source_directory).as_posix())


def validate_server_version(source_directory: Path, expected_version: str) -> None:
    system = {"Darwin": "macos", "Linux": "linux", "Windows": "windows"}.get(platform.system())
    architecture = {
        "arm64": "arm64",
        "aarch64": "arm64",
        "x86_64": "x64",
        "amd64": "x64",
    }.get(platform.machine().lower())
    if system is None or architecture is None:
        raise RuntimeError("cannot verify sync server version on this build host")
    suffix = ".exe" if system == "windows" else ""
    executable = source_directory / f"norishell-sync-server_{system}_{architecture}{suffix}"
    actual = subprocess.check_output([str(executable), "--version"], text=True).strip()
    if actual != f"NoriShell Sync Server {expected_version}":
        raise RuntimeError(f"sync server version mismatch: {actual}")


def validate_plugin(path: Path, expected_version: str) -> None:
    with zipfile.ZipFile(path) as archive:
        names = set(archive.namelist())
        manifest = json.loads(archive.read("manifest.json")) if "manifest.json" in names else None
    if not {"manifest.json", "plugin.wasm", "assets/settings.json"} <= names:
        raise RuntimeError("plugin package is missing its manifest, Wasm, or settings")
    if manifest["version"] != expected_version:
        raise RuntimeError("plugin package version does not match source manifest")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--online", action="store_true", help="allow Cargo to fetch dependencies")
    args = parser.parse_args()
    app_version = json.loads((REPOSITORY / "package.json").read_text())["version"]
    sync_version = json.loads((ROOT / "plugin" / "manifest.json").read_text())["version"]
    output_directory = args.output_dir or REPOSITORY / "output" / "releases" / f"v{app_version}"
    output_directory.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="norishell-self-host-sync-") as temporary:
        stage = Path(temporary)
        server_directory = stage / "server"
        server_directory.mkdir()
        subprocess.run(
            ["python3", str(ROOT / "server" / "build.py"), "--output-dir", str(server_directory)],
            check=True,
            cwd=ROOT / "server",
        )
        validate_server_version(server_directory, sync_version)
        server_zip = stage / "server.zip"
        package_server(server_directory, server_zip)
        plugin_zip = stage / "sync_plugin.zip"
        plugin_command = ["python3", str(ROOT / "plugin" / "build.py"), "--output", str(plugin_zip)]
        if args.online:
            plugin_command.append("--online")
        subprocess.run(
            plugin_command,
            check=True,
            cwd=ROOT / "plugin",
        )
        validate_plugin(plugin_zip, sync_version)
        server_asset = output_directory / f"NoriShell_SelfHostSync_Server_{sync_version}.zip"
        plugin_asset = output_directory / f"NoriShell_SelfHostSync_Plugin_{sync_version}.zip"
        shutil.copy2(server_zip, server_asset)
        shutil.copy2(plugin_zip, plugin_asset)
        print(server_asset)
        print(plugin_asset)


if __name__ == "__main__":
    main()
