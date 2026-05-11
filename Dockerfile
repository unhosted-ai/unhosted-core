# Multi-stage build for the unhosted daemon.
#
# What this image contains:
#   - the `unhosted` binary (daemon + cli)
#
# What it does NOT contain:
#   - llama.cpp (point at one on the host or in a sibling container)
#   - any model files (use `unhosted pull <model>` once mounted)
#
# Typical run:
#   docker run --rm -p 7777:7777 \
#     -v ~/.cache/unhosted:/root/.cache/unhosted \
#     -v ~/.config/unhosted:/root/.config/unhosted \
#     -e UNHOSTED_LLAMA_SERVER_URL=http://host.docker.internal:8080 \
#     ghcr.io/unhosted-ai/unhosted:latest serve --addr 0.0.0.0:7777

FROM --platform=$BUILDPLATFORM rust:1.86-slim-bookworm AS build
ARG TARGETPLATFORM
ARG BUILDPLATFORM
WORKDIR /src

# System deps for cargo + reqwest's tls
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Cache dependencies separately from the source so changes to src
# don't re-download every crate.
COPY Cargo.toml Cargo.lock rust-toolchain.toml LICENSE README.md ./
COPY crates ./crates
RUN cargo build --release -p unhosted-cli && \
    strip target/release/unhosted

FROM debian:bookworm-slim

# OCI annotations — picked up by GitHub Container Registry to link this
# package to its source repository, license, and docs on the package page.
LABEL org.opencontainers.image.title="unhosted"
LABEL org.opencontainers.image.description="AI that lives where you do — open-source software that pools the computers you already own into a single inference cluster."
LABEL org.opencontainers.image.source="https://github.com/unhosted-ai/unhosted-core"
LABEL org.opencontainers.image.url="https://github.com/unhosted-ai/unhosted-core"
LABEL org.opencontainers.image.documentation="https://github.com/unhosted-ai/unhosted-core/blob/main/README.md"
LABEL org.opencontainers.image.licenses="AGPL-3.0-or-later"
LABEL org.opencontainers.image.vendor="unhosted-ai"
LABEL org.opencontainers.image.authors="Unhosted contributors <noreply@unhosted.dev>"

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/target/release/unhosted /usr/local/bin/unhosted
COPY --from=build /src/LICENSE /usr/share/doc/unhosted/LICENSE
COPY --from=build /src/README.md /usr/share/doc/unhosted/README.md

EXPOSE 7777

# Default to running the daemon. Override with e.g. `pull llama3.2:1b` or
# `models` to use it as a one-off CLI.
ENTRYPOINT ["unhosted"]
CMD ["serve", "--addr", "0.0.0.0:7777"]
