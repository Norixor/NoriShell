#!/usr/bin/env bash
# Shared build steps, run inside the Ubuntu 22.04 environment (Docker container or WSL2).
# Env: SRC (read-only source checkout), WORK (build dir that keeps target/ and node_modules/),
#      OUT (output dir), ARCH (x64|arm64), VERSION.
set -euo pipefail
rsync -a --delete \
  --exclude=/target --exclude=/node_modules --exclude=/output --exclude=/docs --exclude=/dist \
  --exclude=/.git --exclude=/AGENTS.md --exclude=/.build \
  "$SRC"/ "$WORK"/
cd "$WORK"
pnpm install --frozen-lockfile
# target/ persists between builds; stale bundles of older versions must not match the copy globs below.
rm -rf target/release/bundle
# EXTRA_CONFIG (optional JSON) is only for local updater tests: version, pubkey and endpoint overrides.
extra=(); [ -n "${EXTRA_CONFIG:-}" ] && extra=(--config "$EXTRA_CONFIG")
pnpm tauri build --bundles deb,appimage --config '{"bundle":{"createUpdaterArtifacts":false}}' "${extra[@]}"
bundle=target/release/bundle
cp "$bundle"/appimage/*.AppImage "$OUT/NoriShell_${VERSION}_linux_${ARCH}.AppImage"
cp "$bundle"/deb/*.deb "$OUT/NoriShell_${VERSION}_linux_${ARCH}.deb"
sha256sum target/release/norishell | cut -d' ' -f1 > "$OUT/.exe-${ARCH}.sha"
echo "rustc: $(rustc --version)  node: $(node -v)  pnpm: $(pnpm -v)" > "$OUT/.toolchain-${ARCH}"
