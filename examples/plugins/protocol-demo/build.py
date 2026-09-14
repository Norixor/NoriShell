#!/usr/bin/env python3
"""Build, ABI-verify, package, and hash the Framed TCP protocol demo."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import zipfile


def run(command, *, cwd, environment):
    subprocess.run(command, cwd=cwd, env=environment, check=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    source = Path(__file__).resolve().parent
    manifest = json.loads((source / "manifest.json").read_text(encoding="utf-8"))
    expected_name = f"NoriShell-Framed-TCP-Protocol-Demo-{manifest['version']}.zip"
    if args.output.name != expected_name:
        parser.error(f"--output must be named {expected_name} to match manifest.json")
    repository = source.parents[2]
    toolchain = "1.97.1"
    target = "wasm32-unknown-unknown"
    environment = os.environ.copy()
    environment["CARGO_INCREMENTAL"] = "0"
    environment["RUSTC"] = subprocess.check_output(
        ["rustup", "which", "--toolchain", toolchain, "rustc"], text=True
    ).strip()

    with tempfile.TemporaryDirectory(prefix="norishell-protocol-demo-") as temporary:
        temporary = Path(temporary)
        target_dir = temporary / "target"
        run(
            [
                "rustup", "run", toolchain, "cargo", "build", "--locked", "--offline",
                "--release", "--target", target, "--target-dir", str(target_dir),
                "--manifest-path", str(source / "Cargo.toml"), "--package", "norishell-protocol-demo",
            ],
            cwd=repository,
            environment=environment,
        )
        wasm = target_dir / target / "release" / "norishell_protocol_demo.wasm"
        fixture = subprocess.Popen(
            ["python3", str(source / "fixture_server.py"), "--port", "0"],
            cwd=repository,
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            line = fixture.stdout.readline() if fixture.stdout else ""
            port = json.loads(line)["port"]
            run(
                [
                    "rustup", "run", toolchain, "cargo", "run", "--locked", "--offline",
                    "--release", "--target-dir", str(target_dir / "harness"),
                    "--manifest-path", str(source / "Cargo.toml"), "--package",
                    "norishell-protocol-demo-harness", "--", "--wasm", str(wasm), "--port", str(port),
                ],
                cwd=repository,
                environment=environment,
            )
            fixture.wait(timeout=10)
            if fixture.returncode:
                raise RuntimeError(f"fixture failed: {fixture.stderr.read() if fixture.stderr else ''}")
        finally:
            if fixture.poll() is None:
                fixture.terminate()
                fixture.wait(timeout=5)

        args.output.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=args.output.parent, suffix=".zip", delete=False) as temporary_zip:
            staging = Path(temporary_zip.name)
        try:
            entries = [
                ("manifest.json", source / "manifest.json"),
                ("plugin.wasm", wasm),
                ("assets/protocols.json", source / "assets" / "protocols.json"),
            ]
            with zipfile.ZipFile(staging, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                for name, path in entries:
                    info = zipfile.ZipInfo(name, date_time=(2026, 9, 13, 0, 0, 0))
                    info.compress_type = zipfile.ZIP_DEFLATED
                    info.external_attr = 0o100644 << 16
                    archive.writestr(info, path.read_bytes())
            shutil.move(staging, args.output)
        finally:
            staging.unlink(missing_ok=True)
        run(
            [
                "rustup", "run", toolchain, "cargo", "run", "--locked", "--offline",
                "--release", "--target-dir", str(target_dir / "package-check"),
                "--manifest-path", str(repository / "Cargo.toml"), "--package",
                "norishell-plugin-sdk", "--bin", "norishell-plugin-dev", "--", "check",
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
