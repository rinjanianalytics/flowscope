# The web interface is a static export. The Rust build embeds it.
FROM --platform=$BUILDPLATFORM node:22-alpine AS build-web
RUN corepack enable
WORKDIR /build/web
# Cache for modules
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store pnpm install --frozen-lockfile
# Build. It writes the export to ../crates/flowscope-console/data/frontend.
COPY web ./
RUN mkdir -p /build/crates/flowscope-console/data && pnpm build

FROM rust:1.92-bookworm AS build-rust
# CMake builds the Kafka client.
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY . .
COPY --from=build-web /build/crates/flowscope-console/data/frontend crates/flowscope-console/data/frontend
# Without a version, the one of the workspace is used.
ARG VERSION
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    if [ -n "$VERSION" ]; then export FLOWSCOPE_VERSION="$VERSION"; fi \
 && cargo build --release --locked -p flowscope \
 && cp target/release/flowscope /flowscope \
# Keep the names of the functions for the backtraces, drop the rest.
 && strip --strip-debug /flowscope

FROM gcr.io/distroless/cc-debian12:latest
LABEL org.opencontainers.image.title="FlowScope"
LABEL org.opencontainers.image.description="Flow collector, enricher and visualizer"
LABEL org.opencontainers.image.source=https://github.com/rinjanianalytics/flowscope
LABEL org.opencontainers.image.licenses=AGPL-3.0-only
COPY --from=build-rust /flowscope /usr/local/bin/flowscope
EXPOSE 8080
HEALTHCHECK --interval=20s CMD [ "/usr/local/bin/flowscope", "healthcheck" ]
ENTRYPOINT [ "/usr/local/bin/flowscope" ]
