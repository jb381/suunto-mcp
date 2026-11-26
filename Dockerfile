FROM rust:1.96-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY protocol/ protocol/
RUN cargo build --locked --release

FROM debian:bookworm-slim

COPY --from=builder /build/target/release/suunto-mcp /usr/local/bin/suunto-mcp

ENV SUUNTO_TOKEN_STORE=file \
    SUUNTO_TOKEN_FILE=/data/tokens.json \
    SUUNTO_API_QUOTA_FILE=/data/api-quota.json \
    SUUNTO_LOCAL_DATA_DIR=/data/data \
    SUUNTO_EXPORT_DIR=/data/exports \
    SUUNTO_WEBHOOK_STORE=sqlite \
    SUUNTO_WEBHOOK_STORE_PATH=/data/webhooks.sqlite3

EXPOSE 8000
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
  CMD ["suunto-mcp", "--healthcheck"]

# Retain access to root-owned mode 0600 files in existing volumes.
ENTRYPOINT ["suunto-mcp"]
CMD ["--transport", "streamable-http", "--host", "0.0.0.0", "--port", "8000"]
