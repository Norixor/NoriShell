#!/usr/bin/env python3
"""Build and validate the self-hosted sync Wasm ZIP with the production package checker."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def run(command, *, cwd, environment):
    subprocess.run(command, cwd=cwd, env=environment, check=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--online", action="store_true", help="Allow Cargo to download dependencies on a fresh runner")
    parser.add_argument("--target-dir", type=Path, help="Reuse a Cargo build cache")
    args = parser.parse_args()
    source = Path(__file__).resolve().parent
    repository = source.parents[2]
    output = args.output.resolve()
    if output.name != "sync_plugin.zip":
        parser.error("output file must be named sync_plugin.zip")
    toolchain = "1.97.1"
    environment = os.environ.copy()
    environment["CARGO_INCREMENTAL"] = "0"
    environment["RUSTC"] = subprocess.check_output(
        ["rustup", "which", "--toolchain", toolchain, "rustc"], text=True
    ).strip()
    cargo_mode = [] if args.online else ["--offline"]
    with tempfile.TemporaryDirectory(prefix="norishell-self-host-sync-") as temporary:
        temporary = Path(temporary)
        target_dir = args.target_dir.resolve() if args.target_dir else temporary / "target"
        cargo = ["rustup", "run", toolchain, "cargo"]
        run(
            cargo + ["build", "--locked", *cargo_mode, "--release", "--target", "wasm32-unknown-unknown",
                     "--target-dir", str(target_dir), "--manifest-path", str(source / "Cargo.toml")],
            cwd=repository, environment=environment,
        )
        wasm = target_dir / "wasm32-unknown-unknown" / "release" / "norishell_self_host_sync.wasm"
        run(
            cargo + ["run", "--locked", *cargo_mode, "--release", "--target-dir", str(target_dir),
                     "--manifest-path", str(source / "verify" / "Cargo.toml"), "--", str(wasm)],
            cwd=repository, environment=environment,
        )
        tool = cargo + ["run", "--locked", *cargo_mode, "--release", "--target-dir", str(target_dir),
                        "-p", "norishell-plugin-sdk", "--bin", "norishell-plugin-dev", "--"]
        package = temporary / "sync_plugin.zip"
        run(tool + ["pack", str(source), "--wasm", str(wasm), "--output", str(package)],
            cwd=repository, environment=environment)
        run(tool + ["check", str(package)], cwd=repository, environment=environment)
        output.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=output.parent, suffix=".zip", delete=False) as staging_file:
            staging = Path(staging_file.name)
        try:
            shutil.copyfile(package, staging)
            os.replace(staging, output)
        finally:
            staging.unlink(missing_ok=True)
    digest = hashlib.sha256(output.read_bytes()).hexdigest()
    print(json.dumps({"file": str(output), "sha256": digest, "bytes": output.stat().st_size}, indent=2))


if __name__ == "__main__":
    main()
