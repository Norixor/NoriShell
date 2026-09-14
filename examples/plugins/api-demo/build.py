#!/usr/bin/env python3
"""Build and ABI-verify the protocol-13 SDK demo without packaging a release."""

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
    target = "wasm32-unknown-unknown"
    toolchain = "1.97.1"
    environment = os.environ.copy()
    # The installed wasm32 standard library belongs to this local Rust toolchain.
    environment["RUSTC"] = subprocess.check_output(
        ["rustup", "which", "--toolchain", toolchain, "rustc"], text=True
    ).strip()
    with tempfile.TemporaryDirectory(prefix="norishell-api-demo-") as temporary:
        target_dir = Path(temporary) / "target"
        subprocess.run(
            [
                "rustup", "run", toolchain, "cargo", "build", "--locked", "--offline", "--release", "--target", target,
                "--target-dir", str(target_dir), "--manifest-path", str(source / "Cargo.toml"),
            ],
            cwd=repository,
            env=environment,
            check=True,
        )
        wasm = target_dir / target / "release" / "norishell_api_demo.wasm"
        subprocess.run(
            [
                "rustup", "run", toolchain, "cargo", "run", "--locked", "--offline", "-p", "norishell-plugin-sdk",
                "--example", "verify_api_demo", "--target-dir", str(target_dir / "verify"), "--", str(wasm),
            ],
            cwd=repository,
            env=environment,
            check=True,
        )
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=args.output.parent, suffix=".zip", delete=False) as temporary_zip:
            staging = Path(temporary_zip.name)
        try:
            with zipfile.ZipFile(staging, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                for name in ("manifest.json", "plugin.wasm"):
                    info = zipfile.ZipInfo(name, date_time=(2026, 9, 10, 0, 0, 0))
                    info.compress_type = zipfile.ZIP_DEFLATED
                    info.external_attr = 0o100644 << 16
                    archive.writestr(
                        info,
                        (source / "manifest.json").read_bytes() if name == "manifest.json" else wasm.read_bytes(),
                    )
            shutil.move(staging, args.output)
        finally:
            staging.unlink(missing_ok=True)

    digest = hashlib.sha256(args.output.read_bytes()).hexdigest()
    checksum_output = args.output.with_name(f"{args.output.name}.sha256")
    with tempfile.NamedTemporaryFile(
        dir=checksum_output.parent,
        suffix=".sha256",
        mode="w",
        encoding="utf-8",
        delete=False,
    ) as temporary_checksum:
        staging_checksum = Path(temporary_checksum.name)
        temporary_checksum.write(f"{digest}  {args.output.name}\n")
    try:
        shutil.move(staging_checksum, checksum_output)
    finally:
        staging_checksum.unlink(missing_ok=True)
    print(
        json.dumps(
            {
                "file": str(args.output),
                "checksumFile": str(checksum_output),
                "sha256": digest,
                "bytes": args.output.stat().st_size,
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
