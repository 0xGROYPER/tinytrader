# syntax=docker/dockerfile:1
# Container images of the services, built with Cargo. One image per service,
# chosen with the SERVICE build argument (the Cargo package name):
#   docker build --build-arg SERVICE=console -t tinytrader/console:local .
#   docker buildx bake                    # every service (docker-bake.hcl)
#   docker buildx bake ticker console     # some of them
# CI publishes these images as ghcr.io/<owner>/tinytrader-<service>
# (.github/workflows/images.yml).
#
# Every service's binary comes from one Cargo build, so building several
# images compiles the workspace once. The build runs on the build machine's
# own architecture and cross-compiles for the target (linux/amd64 or
# linux/arm64), so no emulation is needed for multi-architecture images.
#
# Each image holds the service as /app, CA certificates, and a /data owned
# by the runtime user, so a named volume mounted there starts writable. It
# runs as 65532 (nonroot), with no shell, like the Nix images (nix/images.nix).

# The toolchain in rust-toolchain.toml. Change both lines with it; CI checks
# they agree.
FROM --platform=$BUILDPLATFORM rust:1.96.1-slim-trixie@sha256:31ee7fc65186be7e0e0ccb3f2ca305f14e4739e7642a1ae65753aa5d7b874523 AS build
# Pins the toolchain the image ships with, so rustup doesn't fetch the extra
# components rust-toolchain.toml lists (rustfmt, clippy) for a build.
ENV RUSTUP_TOOLCHAIN=1.96.1 \
    CARGO_TARGET_DIR=/target \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
ARG TARGETARCH
# A cross compiler and the Rust target, when the target isn't this machine.
RUN set -eu; \
    case "$TARGETARCH" in \
      amd64) triple=x86_64-unknown-linux-gnu gcc=gcc-x86-64-linux-gnu ;; \
      arm64) triple=aarch64-unknown-linux-gnu gcc=gcc-aarch64-linux-gnu ;; \
      *) echo "unsupported architecture: $TARGETARCH (amd64 or arm64)" >&2; exit 1 ;; \
    esac; \
    echo "$triple" > /triple; \
    if [ "$(dpkg --print-architecture)" != "$TARGETARCH" ]; then \
      apt-get update; \
      apt-get install -y --no-install-recommends "$gcc" "libc6-dev-$TARGETARCH-cross"; \
      rm -rf /var/lib/apt/lists/*; \
      rustup target add "$triple"; \
    fi

WORKDIR /src
# Only what the Rust build reads (as nix/rust.nix), so edits to docs,
# examples or Nix files reuse the cached build.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY apps apps
COPY libs libs
COPY services services
# Every binary of the workspace but the terminal view, into /out.
# JOBS caps the build's parallel jobs (default: every core), e.g. to leave
# room for services running on the same machine.
ARG JOBS
RUN --mount=type=cache,id=tinytrader-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=tinytrader-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=tinytrader-target-$TARGETARCH,target=/target \
    set -eu; \
    triple="$(cat /triple)"; \
    cargo build --release --locked ${JOBS:+--jobs "$JOBS"} --target "$triple" --workspace --exclude tinytrader-tui --bins; \
    mkdir -p /out/bin /out/data; \
    find "/target/$triple/release" -maxdepth 1 -type f -perm -u+x -exec cp {} /out/bin/ \;; \
    ls /out/bin

# The service named by SERVICE, checked so a typo fails here, clearly.
FROM build AS pick
ARG SERVICE
RUN [ -n "${SERVICE:-}" ] && [ -x "/out/bin/$SERVICE" ] || { \
      echo "set --build-arg SERVICE=<name>, one of:" $(ls /out/bin) >&2; exit 1; }

# One service. distroless/cc: glibc, libgcc, CA certificates, a nonroot
# user (65532) and /tmp; no shell or package manager.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2 AS service
ARG SERVICE
COPY --from=pick /out/bin/${SERVICE} /app
COPY --from=build --chown=65532:65532 /out/data /data
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
USER 65532:65532
ENTRYPOINT ["/app"]
LABEL org.opencontainers.image.title="tinytrader-${SERVICE}" \
      org.opencontainers.image.licenses="MIT"
