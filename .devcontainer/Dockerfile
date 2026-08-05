# CAN monitor TUI - Rust development container
#
# Provides:
#   - Rust stable toolchain (>= 1.88, MSRV for ratatui 0.30.2; base image ships 1.97.1)
#   - aarch64-unknown-linux-gnu cross-compilation target (RK3588) via
#     cargo-zigbuild (zig cc backend) + gcc-aarch64-linux-gnu / libc6-dev-arm64-cross
#   - can-utils (cansend / candump / canplayer) for SocketCAN + vcan testing
#   - libusb-dev (legacy 0.1 API, provides usb.h) for libcontrolcan .a link mode
FROM mcr.microsoft.com/devcontainers/rust:1-bookworm

# Zig backend used by cargo-zigbuild (not packaged in Debian bookworm).
ARG ZIG_VERSION=0.13.0

# --- Cross toolchain, CAN utilities, build deps ----------------------------
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get install -y --no-install-recommends \
        gcc-aarch64-linux-gnu \
        libc6-dev-arm64-cross \
        can-utils \
        pkg-config \
        libusb-dev \
        iproute2 \
        ca-certificates \
        curl \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# --- Zig (official tarball, pinned LTS) ------------------------------------
# ziglang.org throttles single connections from CN egress (~30KB/s), so download
# in 8 parallel byte-range segments and concatenate. Verified working on this
# network. zig cc does not search system library paths -> library link-search
# must be provided explicitly by build.rs (see Task 8); zig here is only the
# C backend.
RUN set -eux; \
    ZARCH="$(uname -m)"; \
    ZURL="https://ziglang.org/download/${ZIG_VERSION}/zig-linux-${ZARCH}-${ZIG_VERSION}.tar.xz"; \
    SIZE=$(curl -fsSL -sI "$ZURL" | tr -d '\r' | awk 'tolower($1)=="content-length:"{print $2}'); \
    test -n "$SIZE" || { echo "cannot determine zig tarball size" >&2; exit 1; }; \
    SEGS=8; \
    SEGSIZE=$((SIZE / SEGS)); \
    i=0; while [ "$i" -lt "$SEGS" ]; do \
        START=$((i * SEGSIZE)); \
        END=$(((i + 1) * SEGSIZE - 1)); \
        [ "$i" -eq $((SEGS - 1)) ] && END=$((SIZE - 1)); \
        ( curl -fsSL --retry 5 --retry-delay 2 -r "${START}-${END}" -o "/tmp/zig.part.${i}" "$ZURL" ) & \
        i=$((i + 1)); \
    done; \
    wait; \
    cat /tmp/zig.part.0 /tmp/zig.part.1 /tmp/zig.part.2 /tmp/zig.part.3 \
        /tmp/zig.part.4 /tmp/zig.part.5 /tmp/zig.part.6 /tmp/zig.part.7 > /tmp/zig.tar.xz; \
    rm -f /tmp/zig.part.*; \
    xz -t /tmp/zig.tar.xz; \
    tar -xJf /tmp/zig.tar.xz -C /usr/local; \
    mv "/usr/local/zig-linux-${ZARCH}-${ZIG_VERSION}" /usr/local/zig; \
    ln -sf /usr/local/zig/zig /usr/local/bin/zig; \
    rm /tmp/zig.tar.xz; \
    zig version

# --- cargo-zigbuild ---------------------------------------------------------
RUN cargo install cargo-zigbuild --locked

# --- Rust toolchain: ensure stable >= 1.88, add aarch64 target --------------
# Prebuilt libcontrolcan .a is glibc-linked -> glibc target (aarch64-unknown-linux-gnu),
# NOT musl.
# static.rust-lang.org is ~300B/s from CN egress -> use rsproxy.cn mirror (tested
# reachable). Cargo/crates.io is unaffected.
ENV RUSTUP_DIST_SERVER=https://rsproxy.cn \
    RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup
RUN rustup update stable \
    && rustup default stable \
    && rustup target add aarch64-unknown-linux-gnu

# --- Verification (fails the build if anything is missing) ------------------
RUN rustc --version \
    && cargo-zigbuild --version \
    && zig version \
    && rustup target list --installed \
    && which cansend candump canplayer \
    && which aarch64-linux-gnu-gcc

# Cross-compile smoke test to confirm toolchain wiring (no project sources are
# added to the image; this is a throwaway crate in /tmp).
RUN mkdir -p /tmp/cross-smoke/src \
    && printf 'fn main() { println!("hello aarch64"); }\n' > /tmp/cross-smoke/src/main.rs \
    && printf '[package]\nname="cross-smoke"\nversion="0.1.0"\nedition="2021"\n' > /tmp/cross-smoke/Cargo.toml \
    && cargo zigbuild --manifest-path /tmp/cross-smoke/Cargo.toml \
        --target aarch64-unknown-linux-gnu --release -q \
    && file /tmp/cross-smoke/target/aarch64-unknown-linux-gnu/release/cross-smoke \
    && rm -rf /tmp/cross-smoke
