# syntax=docker/dockerfile:1.7
#
# Public Carryforth Local Relay image — published as
# ghcr.io/lgyanami/carryforth-relay:<tag> by the canonical release lane.
#
# Builds the `buzz-relay` binary (Rust 1.95), then assembles it into a small
# debian-slim runtime with `git` available (the relay shells out to git for
# repo hydrate / receive-pack / upload-pack — see crates/buzz-relay/src/api/git).
#
# Carryforth's first public Relay artifact deliberately contains no Web or
# Admin SPA. Those source trees are not part of the supported release surface.
#
# Multi-arch is handled by running this same Dockerfile on native amd64 and
# native arm64 runners (see .github/workflows/docker.yml). The Dockerfile
# itself is platform-agnostic; do not add --platform pins.

ARG RUST_VERSION=1.95
ARG DEBIAN_VERSION=bookworm

# Optional extra CA bundle for builds behind a TLS-intercepting corporate proxy
# (e.g. a Cloudflare/Zscaler gateway that re-signs TLS). Empty by default, so
# public CI builds are unaffected. Point it at a PEM file in the build context:
#   docker build --build-arg EXTRA_CA_CERTS=path/to/proxy-ca.pem ...
# Consumed by the network-touching stages below (cargo + pnpm).
ARG EXTRA_CA_CERTS=

# ─── Stage 1: cargo-chef base ───────────────────────────────────────────────
FROM rust:${RUST_VERSION}-${DEBIAN_VERSION} AS chef
# Trust an optional corporate-proxy CA before any network fetch (no-op if unset).
ARG EXTRA_CA_CERTS
COPY --chmod=0644 ${EXTRA_CA_CERTS:-Dockerfile} /tmp/extra-ca/src
RUN if [ -n "${EXTRA_CA_CERTS}" ]; then \
        cp /tmp/extra-ca/src /usr/local/share/ca-certificates/extra-proxy-ca.crt \
        && update-ca-certificates \
        && echo "CARGO_HTTP_CAINFO=/etc/ssl/certs/ca-certificates.crt" >> /etc/environment; \
    fi
ENV CARGO_HTTP_CAINFO=/etc/ssl/certs/ca-certificates.crt
RUN cargo install cargo-chef --locked --version 0.1.71
WORKDIR /build

# ─── Stage 2: plan dependency graph ─────────────────────────────────────────
# Only the manifests are needed to compute the recipe; this layer rebuilds
# only when Cargo.{toml,lock} or crate manifests change, not on every source
# edit.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ─── Stage 3: cook dependencies, then build the binary ──────────────────────
FROM chef AS builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        pkg-config \
        libssl-dev \
        ca-certificates \
        git \
    && rm -rf /var/lib/apt/lists/*
COPY --from=planner /build/recipe.json recipe.json
# Cook the full workspace recipe — relay deps include workspace siblings, so
# scoping to -p buzz-relay misses transitive deps and re-builds them later.
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked -p buzz-relay --bin buzz-relay \
                                   -p buzz-admin --bin buzz-admin \
                                   -p buzz-pair-relay --bin buzz-pair-relay \
    && strip target/release/buzz-relay \
    && strip target/release/buzz-admin \
    && strip target/release/buzz-pair-relay

# ─── Stage 4: runtime ───────────────────────────────────────────────────────
FROM debian:${DEBIAN_VERSION}-slim AS runtime

# OCI annotations identify the source repository and allow GHCR to link the
# package to it. GHCR package visibility remains an explicit repository-owner
# setting; the publisher verifies anonymous pull after creating the manifest.
LABEL org.opencontainers.image.title="Carryforth Relay" \
      org.opencontainers.image.description="Local-first WebSocket relay for Carryforth" \
      org.opencontainers.image.source="https://github.com/lgYanami/Carryforth" \
      org.opencontainers.image.url="https://github.com/lgYanami/Carryforth" \
      org.opencontainers.image.documentation="https://github.com/lgYanami/Carryforth#readme" \
      org.opencontainers.image.licenses="Apache-2.0"

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        git \
        openssl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 1000 buzz \
    && useradd  --system --uid 1000 --gid 1000 --home-dir /var/lib/buzz \
                --create-home --shell /usr/sbin/nologin buzz \
    && install -d -o buzz -g buzz -m 0750 /data/git

COPY --from=builder    /build/target/release/buzz-relay /usr/local/bin/buzz-relay
COPY --from=builder    /build/target/release/buzz-admin /usr/local/bin/buzz-admin
COPY --from=builder    /build/target/release/buzz-pair-relay /usr/local/bin/buzz-pair-relay
COPY LICENSE NOTICE UPSTREAM.md /usr/share/licenses/carryforth/
COPY release/THIRD_PARTY_ASSETS.md /usr/share/licenses/carryforth/THIRD_PARTY_ASSETS.md

# 3000: app (WS + REST)  ·  8080: /_liveness, /_readiness  ·  9102: /metrics
EXPOSE 3000 8080 9102

USER buzz:buzz
WORKDIR /var/lib/buzz

ENTRYPOINT ["/usr/local/bin/buzz-relay"]
