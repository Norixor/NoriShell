#!/usr/bin/env python3
# Usage: write-metadata.py <out-dir> <arch> <version> <revision> <dirty:true|false>
import hashlib, json, pathlib, sys, time
out, arch, version, revision, dirty = pathlib.Path(sys.argv[1]), *sys.argv[2:]
def sha(p): return hashlib.sha256(p.read_bytes()).hexdigest()
base = f"NoriShell_{version}_linux_{arch}"
meta = {
    "schema": 1, "version": version, "arch": arch, "revision": revision, "dirty": dirty == "true",
    "executableSha256": (out / f".exe-{arch}.sha").read_text().strip(),
    "appImageSha256": sha(out / f"{base}.AppImage"),
    "debSha256": sha(out / f"{base}.deb"),
    "toolchain": (out / f".toolchain-{arch}").read_text().strip(),
    "builtAtUnix": int(time.time()),
}
(out / f"linux-build-{arch}.json").write_text(json.dumps(meta, indent=2) + "\n")
(out / f".exe-{arch}.sha").unlink(); (out / f".toolchain-{arch}").unlink()
print(json.dumps(meta, indent=2))
