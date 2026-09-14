#!/usr/bin/env python3
"""Build and ABI-verify the protocol-13 isolated WebView demo locally."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import zipfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    source = Path(__file__).resolve().parent
    repository = source.parents[2]
    toolchain = "1.97.1"
    environment = os.environ.copy()
    environment["RUSTC"] = subprocess.check_output(
        ["rustup", "which", "--toolchain", toolchain, "rustc"], text=True
    ).strip()
    with tempfile.TemporaryDirectory(prefix="norishell-isolated-demo-") as temporary:
        target_dir = Path(temporary) / "target"
        subprocess.run(
            [
                "rustup", "run", toolchain, "cargo", "build", "--locked", "--offline",
                "--release", "--target", "wasm32-unknown-unknown", "--target-dir",
                str(target_dir), "--manifest-path", str(source / "Cargo.toml"),
            ],
            cwd=repository,
            env=environment,
            check=True,
        )
        wasm = target_dir / "wasm32-unknown-unknown/release/norishell_isolated_demo.wasm"
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=args.output.parent, suffix=".zip", delete=False) as temporary_zip:
            staging = Path(temporary_zip.name)
        try:
            entries = [
                ("manifest.json", source / "manifest.json"),
                ("plugin.wasm", wasm),
                ("assets/isolated/isolated-demo.html", source / "assets/isolated/main.html"),
            ]
            with zipfile.ZipFile(staging, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                for name, path in entries:
                    info = zipfile.ZipInfo(name, date_time=(2026, 9, 10, 0, 0, 0))
                    info.compress_type = zipfile.ZIP_DEFLATED
                    info.external_attr = 0o100644 << 16
                    archive.writestr(info, path.read_bytes())
            subprocess.run(
                [
                    "rustup", "run", toolchain, "cargo", "run", "--locked", "--offline",
                    "-p", "norishell-plugin-sdk", "--example", "verify_isolated_demo",
                    "--target-dir", str(target_dir / "verify"), "--", str(wasm), str(staging),
                ],
                cwd=repository,
                env=environment,
                check=True,
            )
            digest = hashlib.sha256(staging.read_bytes()).hexdigest()
            shutil.move(staging, args.output)
        finally:
            staging.unlink(missing_ok=True)
    print(json.dumps({"file": str(args.output), "bytes": args.output.stat().st_size, "sha256": digest}, indent=2))


if __name__ == "__main__":
    main()
