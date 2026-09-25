#!/usr/bin/env python3
"""Build the self-hosted sync server as six standalone Go binaries."""

import argparse
import os
from pathlib import Path
import subprocess


TARGETS = (
    ("darwin", "arm64", "macos_arm64"),
    ("darwin", "amd64", "macos_x64"),
    ("windows", "amd64", "windows_x64"),
    ("windows", "arm64", "windows_arm64"),
    ("linux", "amd64", "linux_x64"),
    ("linux", "arm64", "linux_arm64"),
)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, default=Path("dist"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parent
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    for goos, goarch, label in TARGETS:
        suffix = ".exe" if goos == "windows" else ""
        binary = output / f"norishell-sync-server_{label}{suffix}"
        env = dict(os.environ, GOOS=goos, GOARCH=goarch, CGO_ENABLED="0")
        subprocess.run(
            ["go", "build", "-trimpath", "-ldflags=-s -w", "-o", str(binary), "."],
            cwd=root,
            env=env,
            check=True,
        )
        print(binary)


if __name__ == "__main__":
    main()
