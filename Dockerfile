# syntax=docker/dockerfile:1
# cista 服务端镜像：多阶段构建，运行层不带编译器/工具链。
FROM rust:1-bookworm AS builder
WORKDIR /src

RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# 只构建服务端（工作区其余成员的源码由 .dockerignore 决定是否进入上下文）
COPY . .
RUN cargo build --release -p cista-server

FROM debian:13-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin cista \
    && mkdir -p /data \
    && chown cista:cista /data

COPY --from=builder /src/target/release/cista-server /usr/local/bin/cista-server

# Docker 没有 systemd 的 UMask=；入口脚本统一 0077，避免宿主 bind mount 里
# shared.db（含明文标题/文件名）被同机其他用户读取。
RUN printf '%s\n' '#!/bin/sh' 'umask 0077' 'exec /usr/local/bin/cista-server "$@"' \
        > /usr/local/bin/cista-entrypoint \
    && chmod 755 /usr/local/bin/cista-entrypoint

ENV CISTA_BIND=0.0.0.0:8787 \
    CISTA_DATA_DIR=/data \
    CISTA_LOG_FORMAT=json

USER cista
VOLUME ["/data"]
EXPOSE 8787

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s \
    CMD curl -fsS http://127.0.0.1:8787/healthz || exit 1

ENTRYPOINT ["/usr/local/bin/cista-entrypoint"]
