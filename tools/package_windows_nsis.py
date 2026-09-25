#!/usr/bin/env python3
"""Repack NSIS from the final Windows EXE and verify the embedded program."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def embedded_exe_sha256(archive: Path, extractor: str) -> str:
    process = subprocess.Popen(
        [extractor, "e", "-so", str(archive), "norishell.exe"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    assert process.stdout is not None
    digest = hashlib.sha256()
    for chunk in iter(lambda: process.stdout.read(1024 * 1024), b""):
        digest.update(chunk)
    _, error = process.communicate()
    if process.returncode != 0:
        raise RuntimeError(f"Could not extract norishell.exe: {error.decode(errors='replace')}")
    return digest.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--nsi", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    exe = args.exe.resolve(strict=True)
    nsi = args.nsi.resolve(strict=True)
    output = args.output.resolve()
    makensis = shutil.which("makensis")
    extractor = shutil.which("7zz") or shutil.which("7z")
    if not makensis or not extractor:
        parser.error("makensis and 7zz/7z are required")

    script = nsi.read_text(encoding="utf-8-sig")
    source_match = re.search(r'^!define MAINBINARYSRCPATH "([^"]+)"$', script, re.MULTILINE)
    output_match = re.search(r'^!define OUTFILE "([^"]+)"$', script, re.MULTILINE)
    if not source_match or not output_match:
        parser.error("NSIS script does not declare its executable source and output")
    if Path(source_match.group(1)).resolve() != exe:
        parser.error("NSIS script refers to a different executable")
    generated = (nsi.parent / output_match.group(1)).resolve()
    if generated == exe or generated == output:
        parser.error("NSIS temporary output must be separate from the source and deliverable")

    expected = sha256(exe)
    generated.unlink(missing_ok=True)
    subprocess.run([makensis, "-V2", str(nsi)], cwd=nsi.parent, check=True)
    if not generated.is_file():
        raise RuntimeError("NSIS did not create its declared output")
    if sha256(exe) != expected or embedded_exe_sha256(generated, extractor) != expected:
        raise RuntimeError("NSIS embedded an EXE that differs from the final compiled EXE")

    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=output.parent, suffix=".exe", delete=False) as staging_file:
        staging = Path(staging_file.name)
    try:
        shutil.copyfile(generated, staging)
        if embedded_exe_sha256(staging, extractor) != expected:
            raise RuntimeError("The staged installer differs from the final compiled EXE")
        os.replace(staging, output)
    finally:
        staging.unlink(missing_ok=True)
    print(json.dumps({"installer": str(output), "installerSha256": sha256(output), "embeddedExeSha256": expected}))


if __name__ == "__main__":
    main()
