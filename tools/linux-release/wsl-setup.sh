#!/usr/bin/env bash
# One-time toolchain setup for building the Linux x64 packages inside WSL2 (Ubuntu 22.04),
# mirroring the Dockerfile. Run as root: bash wsl-setup.sh
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
NODE_VERSION=22.17.0; RUST_VERSION=1.97.1; PNPM_VERSION=10.32.1

apt-get update
apt-get install -y --no-install-recommends \
  build-essential pkg-config curl wget file ca-certificates git rsync xz-utils python3 \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
  libssl-dev libxdo-dev libsoup-3.0-dev patchelf dpkg fakeroot xdg-utils \
  desktop-file-utils squashfs-tools zsync libfuse2 \
  libasound2-dev libudev-dev libdbus-1-dev libxcb1-dev libxkbcommon-dev

arch="$(dpkg --print-architecture)"; case "$arch" in amd64) n=x64;; arm64) n=arm64;; esac
if ! command -v node >/dev/null; then
  curl -fsSL --retry 5 "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${n}.tar.xz" | tar -xJ -C /usr/local --strip-components=1
fi
npm install -g "pnpm@${PNPM_VERSION}"

export RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo
if [ ! -x /opt/cargo/bin/rustc ]; then
  curl -fsSL --retry 5 https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "$RUST_VERSION" -c clippy -c rustfmt
fi
cat > /etc/profile.d/norishell-build.sh <<'P'
export RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH APPIMAGE_EXTRACT_AND_RUN=1 CI=true
P
. /etc/profile.d/norishell-build.sh
echo "rustc: $(rustc --version)  node: $(node -v)  pnpm: $(pnpm -v)"
