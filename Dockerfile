# syntax=docker/dockerfile:1

# ---- Builder ----------------------------------------------------------------
FROM rust:1-bookworm AS builder
WORKDIR /src
# Copy the whole workspace and build only the binary (rusqlite is `bundled`,
# so no system SQLite is needed at build or run time).
COPY . .
RUN cargo build --release --bin minos

# ---- Runtime ----------------------------------------------------------------
FROM debian:bookworm-slim

# Python 3 backs the `python_sidecar` filters; ca-certificates lets sidecar
# scripts make TLS calls (the default requirements template includes
# `requests`).
RUN apt-get update \
 && apt-get install -y --no-install-recommends python3 ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# uv builds the per-service, hash-deduped sidecar venvs. Pulled from Astral's
# published image as documented at https://docs.astral.sh/uv/guides/integration/docker/
COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /usr/local/bin/

COPY --from=builder /src/target/release/minos /usr/local/bin/minos

# Persisted state (SQLite db + venvs) lives under /data; sidecar sockets are
# ephemeral under /run.
ENV MINOS_DB=/data/minos.db \
    MINOS_WEB_BIND=0.0.0.0:8080 \
    MINOS_VENV_ROOT=/data/venvs \
    MINOS_SOCKET_DIR=/run/minos/sidecars \
    RUST_LOG=info
RUN mkdir -p /data/venvs /run/minos/sidecars

VOLUME ["/data"]
EXPOSE 8080
ENTRYPOINT ["minos"]
