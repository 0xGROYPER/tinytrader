# Builds one tinytrader service from the workspace into a minimal, non-root image.
#   docker build -f services/Dockerfile --build-arg SERVICE=ticker -t tinytrader/ticker .
FROM rust:1.96.1-bookworm AS build
ARG SERVICE
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked -p "${SERVICE}" && \
    install -D "target/release/${SERVICE}" /out/app && \
    mkdir -p /out/data

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /out/app /app
# Owned by the runtime user, so a named volume mounted here starts writable.
COPY --from=build --chown=65532:65532 /out/data /data
USER 65532:65532
ENTRYPOINT ["/app"]
