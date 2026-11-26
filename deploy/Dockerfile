# syntax=docker/dockerfile:1.7
#
# DentalClarity public demo image. Build from the repository root:
#
#   docker build -f deploy/Dockerfile -t dentalclarity-demo .
#
# Stages:
#   build    Rust toolchain, wasm32 target, Trunk; builds the web app and the server.
#   runtime  Debian slim with the binary, the synthetic practice and the knowledge base.
#            Runs as the unprivileged user `dentalclarity` (uid 10001). Default target.
#   rootfix  Same image, but starts as root only to give /data to uid 10001 and then
#            drops privileges. For hosts that mount volumes owned by root (Fly.io).
#
# No secrets are read at build time. OPENROUTER_API_KEY is passed at runtime only.

ARG RUST_IMAGE=rust:1.100-slim-trixie
ARG RUNTIME_IMAGE=debian:trixie-slim

FROM ${RUST_IMAGE} AS build

# Prebuilt Trunk release, checked against the published SHA-256.
ARG TRUNK_VERSION=0.22.2
ARG TRUNK_SHA256_AMD64=26fca8a9952e46cd7d8b8b9a2e2c1786c090fc14b8e10b6678ab4d0f9f1a385f
ARG TRUNK_SHA256_ARM64=be7b50f2d24e9cfbaea5551be2abf3fbabac10412113213a8efe94448e514414
ARG TARGETARCH

# The slim Rust image has no download tool; curl fetches the Trunk release.
RUN set -eu; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates curl; \
    rm -rf /var/lib/apt/lists/*

RUN rustup target add wasm32-unknown-unknown

RUN set -eu; \
    case "${TARGETARCH:-amd64}" in \
      amd64) arch=x86_64;  sum="${TRUNK_SHA256_AMD64}" ;; \
      arm64) arch=aarch64; sum="${TRUNK_SHA256_ARM64}" ;; \
      *) echo "unsupported architecture: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    file="trunk-${arch}-unknown-linux-gnu.tar.gz"; \
    curl -fsSL -o "/tmp/${file}" "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/${file}"; \
    echo "${sum}  /tmp/${file}" | sha256sum -c -; \
    tar -xzf "/tmp/${file}" -C /usr/local/bin trunk; \
    rm "/tmp/${file}"; \
    trunk --version

WORKDIR /src
COPY . .

# Cache mounts keep the crate registry, compiled dependencies and the tools Trunk
# downloads (wasm-bindgen, wasm-opt) between builds. Only the final binary leaves
# the cache, so the image layer stays small.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/root/.cache/trunk \
    --mount=type=cache,target=/src/target \
    set -eu; \
    cd crates/dc-web; \
    trunk build --release; \
    cd /src; \
    cargo build --release --locked -p dc-server; \
    install -D -m 0755 target/release/dentalclarity /out/dentalclarity


FROM ${RUNTIME_IMAGE} AS runtime

RUN set -eu; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates; \
    rm -rf /var/lib/apt/lists/*; \
    groupadd --system --gid 10001 dentalclarity; \
    useradd --system --uid 10001 --gid 10001 --home-dir /app --shell /usr/sbin/nologin dentalclarity; \
    mkdir -p /app /data; \
    chown 10001:10001 /data; \
    chmod 0700 /data

WORKDIR /app
COPY --from=build /out/dentalclarity /usr/local/bin/dentalclarity
COPY fixtures/synthetic/practice.toml /app/fixtures/synthetic/practice.toml
COPY knowledge /app/knowledge
COPY --chmod=0755 deploy/entrypoint.sh /usr/local/bin/entrypoint.sh

ENV DC_MODE=demo \
    DC_BIND=0.0.0.0:8080 \
    DC_DATA_DIR=/data \
    DC_FIXTURE=/app/fixtures/synthetic/practice.toml \
    DC_KNOWLEDGE_DIR=/app/knowledge \
    RUST_LOG=info

VOLUME ["/data"]
EXPOSE 8080

# The server shuts down gracefully on SIGINT (Ctrl+C), not SIGTERM.
STOPSIGNAL SIGINT

# Plain bash, so the image needs no curl.
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD ["bash", "-c", "exec 3<>/dev/tcp/127.0.0.1/8080 && printf 'GET /api/health HTTP/1.0\\r\\n\\r\\n' >&3 && grep -q '\"status\":\"ok\"' <&3"]

USER 10001:10001
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["/usr/local/bin/dentalclarity"]


FROM runtime AS rootfix
USER root
