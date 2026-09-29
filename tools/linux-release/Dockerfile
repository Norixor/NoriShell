# Build environment for the Linux Preview packages. Ubuntu 22.04 keeps the
# glibc / WebKitGTK baseline low so the AppImage and deb run on newer distros.
FROM ubuntu:22.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential pkg-config curl wget file ca-certificates git rsync xz-utils \
      libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
      libssl-dev libxdo-dev libsoup-3.0-dev patchelf dpkg fakeroot xdg-utils \
      desktop-file-utils squashfs-tools zsync libfuse2 \
    && rm -rf /var/lib/apt/lists/*
ARG NODE_VERSION=22.17.0
ARG RUST_VERSION=1.97.1
ARG PNPM_VERSION=10.32.1
RUN arch="$(dpkg --print-architecture)"; case "$arch" in amd64) n=x64;; arm64) n=arm64;; esac; \
    curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${n}.tar.xz" | tar -xJ -C /usr/local --strip-components=1 \
    && npm install -g "pnpm@${PNPM_VERSION}"
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain ${RUST_VERSION} -c clippy -c rustfmt
# Kept as its own late layer so adding a package does not redo the toolchain downloads.
RUN for i in 1 2 3 4 5; do apt-get -o Acquire::Retries=5 update && apt-get -o Acquire::Retries=5 install -y --no-install-recommends libasound2-dev libudev-dev libdbus-1-dev libxcb1-dev libxkbcommon-dev && break; sleep 5; done \
    && dpkg -s libasound2-dev libudev-dev >/dev/null && rm -rf /var/lib/apt/lists/*
# The AppImage tools cannot use FUSE inside a container.
ENV APPIMAGE_EXTRACT_AND_RUN=1 CI=true
WORKDIR /work
