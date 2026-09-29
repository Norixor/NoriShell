#!/usr/bin/env bash
# Builds the Linux Preview packages (AppImage + deb) in an Ubuntu 22.04 container.
# Usage: tools/linux-release/build.sh <x64|arm64>
# Output: output/linux/<app-version>/NoriShell_<v>_linux_<arch>.{AppImage,deb} and linux-build-<arch>.json
# Signing is never done here; the updater signature is made on the Mac by package_updater_release.py.
set -euo pipefail

arch="${1:?usage: build.sh <x64|arm64>}"
case "$arch" in
  x64) platform=linux/amd64 ;;
  arm64) platform=linux/arm64 ;;
  *) echo "unknown arch: $arch" >&2; exit 2 ;;
esac

root="$(cd "$(dirname "$0")/../.." && pwd)"
version="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$root/src-tauri/tauri.conf.json")"
out="$root/output/linux/$version"
mkdir -p "$out"
revision="$(git -C "$root" rev-parse HEAD)"
# A clean tracked tree is built from `git archive HEAD`, so untracked files cannot leak into a release
# build; local edits build the working tree and are recorded as dirty.
# The source dir lives under the repo because colima only shares the home directory.
src="$root"
dirty=false
if git -C "$root" diff --quiet HEAD; then
  src="$root/output/linux/.src-$arch"
  rm -rf "$src"; mkdir -p "$src"
  git -C "$root" archive HEAD | tar -x -C "$src"
else
  dirty=true
fi

image="norishell-linux-build:$arch"
docker build --platform "$platform" -t "$image" "$root/tools/linux-release"

# The source is copied into a named volume so target/ and node_modules/ survive between builds
# without touching the Mac working tree.
docker run --rm --platform "$platform" \
  -v "$src":/src:ro -v "$out":/out -v "$root/tools/linux-release/inner-build.sh":/inner-build.sh:ro \
  -v "norishell-work-$arch":/work -v "norishell-cargo-$arch":/opt/cargo/registry \
  -e SRC=/src -e WORK=/work -e OUT=/out -e ARCH="$arch" -e VERSION="$version" \
  "$image" bash /inner-build.sh

python3 "$root/tools/linux-release/write-metadata.py" "$out" "$arch" "$version" "$revision" "$dirty"
