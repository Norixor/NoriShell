#!/usr/bin/env python3
"""Build, package, validate, and hash the Core-brokered GitHub service demo."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


def run(command, *, cwd, environment):
    subprocess.run(command, cwd=cwd, env=environment, check=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    source = Path(__file__).resolve().parent
    repository = source.parents[2]
    expected = "NoriShell-Service-Demo-1.0.1.zip"
    if args.output.name != expected:
        parser.error(f"--output must be named {expected}")
    if args.output.exists():
        parser.error(f"refusing to overwrite an existing candidate: {args.output}")
    if not args.output.parent.is_dir():
        parser.error(f"output directory does not exist: {args.output.parent}")

    environment = os.environ.copy()
    environment["CARGO_INCREMENTAL"] = "0"
    environment["RUSTC"] = subprocess.check_output(
        ["rustup", "which", "--toolchain", "1.97.1", "rustc"], text=True
    ).strip()
    with tempfile.TemporaryDirectory(prefix="norishell-service-demo-") as temporary:
        target = Path(temporary) / "target"
        run(
            [
                "rustup", "run", "1.97.1", "cargo", "test", "--lib", "--locked", "--offline",
                "--target-dir", str(target / "tests"), "--manifest-path", str(source / "Cargo.toml"),
            ],
            cwd=repository,
            environment=environment,
        )
        run(
            [
                "rustup", "run", "1.97.1", "cargo", "build", "--locked", "--offline",
                "--release", "--target", "wasm32-unknown-unknown", "--target-dir", str(target / "wasm"),
                "--manifest-path", str(source / "Cargo.toml"),
            ],
            cwd=repository,
            environment=environment,
        )
        wasm = target / "wasm/wasm32-unknown-unknown/release/norishell_service_demo.wasm"
        run(
            [
                "rustup", "run", "1.97.1", "cargo", "run", "--locked", "--offline", "--release",
                "--target-dir", str(target / "pack"), "--manifest-path", str(repository / "Cargo.toml"),
                "--package", "norishell-plugin-sdk", "--bin", "norishell-plugin-dev", "--", "pack",
                str(source), "--wasm", str(wasm), "--output", str(args.output),
            ],
            cwd=repository,
            environment=environment,
        )
        run(
            [
                "rustup", "run", "1.97.1", "cargo", "run", "--locked", "--offline", "--release",
                "--target-dir", str(target / "check"), "--manifest-path", str(repository / "Cargo.toml"),
                "--package", "norishell-plugin-sdk", "--bin", "norishell-plugin-dev", "--", "check",
                str(args.output),
            ],
            cwd=repository,
            environment=environment,
        )

    digest = hashlib.sha256(args.output.read_bytes()).hexdigest()
    hash_file = args.output.with_suffix(args.output.suffix + ".sha256")
    hash_file.write_text(f"{digest}  {args.output.name}\n", encoding="ascii")
    print(json.dumps({"file": str(args.output), "bytes": args.output.stat().st_size,
                      "sha256": digest, "sha256File": str(hash_file)}, indent=2))


if __name__ == "__main__":
    main()
