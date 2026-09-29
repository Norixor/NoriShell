#!/usr/bin/env bash
# Builds the Linux x64 packages natively in WSL2 on the Windows host (much faster than Docker + Rosetta).
# Requires the one-time setup: tools/linux-release/wsl-setup.sh in an Ubuntu-22.04 WSL distro (kept on D:).
# Usage: tools/linux-release/build-wsl.sh   (output matches build.sh x64)
set -euo pipefail
WIN=admin@100.70.54.15
DISTRO=Ubuntu-22.04
arch=x64
root="$(cd "$(dirname "$0")/../.." && pwd)"
version="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$root/src-tauri/tauri.conf.json")"
out="$root/output/linux/$version"; mkdir -p "$out"
revision="$(git -C "$root" rev-parse HEAD)"; dirty=false
[ -n "$(git -C "$root" status --porcelain)" ] && dirty=true

stage="$(mktemp -d)"; trap 'rm -rf "$stage"' EXIT
COPYFILE_DISABLE=1 tar -czf "$stage/src.tgz" -C "$root" \
  --exclude=./target --exclude=./node_modules --exclude=./output --exclude=./docs --exclude=./dist \
  --exclude=./.git --exclude=./AGENTS.md --exclude=./.build .
cat > "$stage/remote-build.sh" <<REMOTE
set -euo pipefail
. /etc/profile.d/norishell-build.sh
# The tree is unpacked into the WSL ext4 disk; target/ and node_modules/ persist in /root/work.
rm -rf /root/src && mkdir -p /root/src /root/work /root/out
tar -xzf /mnt/d/wsl/src.tgz -C /root/src
sed -i 's/\\r\$//' /mnt/d/wsl/inner-build.sh
SRC=/root/src WORK=/root/work OUT=/root/out ARCH=$arch VERSION=$version bash /mnt/d/wsl/inner-build.sh
cp -a /root/out/. /mnt/d/wsl/
REMOTE
scp -q "$stage/src.tgz" "$stage/remote-build.sh" "$root/tools/linux-release/inner-build.sh" "$WIN:D:/wsl/"
ssh -o BatchMode=yes "$WIN" "wsl -d $DISTRO -u root -- bash -c \"sed -i 's/\\r\$//' /mnt/d/wsl/remote-build.sh && bash /mnt/d/wsl/remote-build.sh\""

mkdir -p "$out"
scp -q "$WIN:D:/wsl/NoriShell_${version}_linux_${arch}.AppImage" "$WIN:D:/wsl/NoriShell_${version}_linux_${arch}.deb" \
  "$WIN:D:/wsl/.exe-${arch}.sha" "$WIN:D:/wsl/.toolchain-${arch}" "$out/"
python3 "$root/tools/linux-release/write-metadata.py" "$out" "$arch" "$version" "$revision" "$dirty"
