# Build the React client.
FROM node:22-bookworm-slim AS web-build
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

# Build the Node plugin host.
FROM node:22-bookworm-slim AS pluginhost-build
WORKDIR /src/pluginhost
COPY pluginhost/package.json pluginhost/package-lock.json ./
RUN npm ci
COPY pluginhost/ ./
RUN npm run build && npm prune --omit=dev

# Build the bundled OMDb plugin against the compiled plugin-host SDK.
FROM node:22-bookworm-slim AS plugin-build
WORKDIR /src/pluginhost
COPY --from=pluginhost-build /src/pluginhost/ ./
WORKDIR /src/plugins/omdb
COPY plugins/omdb/package.json plugins/omdb/package-lock.json ./
RUN npm ci
COPY plugins/omdb/ ./
RUN npm run build

# Build the Rust server.
FROM rust:1-bookworm AS rust-build
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
RUN cargo build --release -p jf-server

# Runtime includes FFmpeg, Node for plugins, and the compiled web client.
FROM node:22-bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 jellyfin \
    && useradd --system --uid 10001 --gid jellyfin --home-dir /app --no-create-home jellyfin \
    && mkdir -p /app/web/dist /app/pluginhost /app/plugins /data \
    && chown -R jellyfin:jellyfin /app /data

WORKDIR /app
COPY --from=rust-build --chown=jellyfin:jellyfin /src/target/release/jellyfin-clone /usr/local/bin/jellyfin-clone
COPY --from=web-build --chown=jellyfin:jellyfin /src/web/dist/ /app/web/dist/
COPY --from=pluginhost-build --chown=jellyfin:jellyfin /src/pluginhost/ /app/pluginhost/
COPY --from=plugin-build --chown=jellyfin:jellyfin /src/plugins/omdb/ /app/plugins/omdb/

ENV JF_DATA_DIR=/data \
    JF_WEB_DIR=/app/web/dist \
    JF_PLUGIN_HOST=/app/pluginhost/dist/index.js \
    JF_PLUGINS_DIR=/app/plugins \
    FFMPEG_PATH=/usr/bin/ffmpeg \
    FFPROBE_PATH=/usr/bin/ffprobe

VOLUME ["/data"]
EXPOSE 8096
USER jellyfin
ENTRYPOINT ["jellyfin-clone"]
CMD ["--host", "0.0.0.0", "--port", "8096", "--data", "/data"]
