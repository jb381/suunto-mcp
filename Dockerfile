# syntax=docker/dockerfile:1
FROM python:3.12-slim AS builder

WORKDIR /build

# Install uv for building
COPY --from=ghcr.io/astral-sh/uv:latest /uv /usr/local/bin/uv

# Install build deps
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
    gcc \
    && rm -rf /var/lib/apt/lists/*

# Cache dependencies (include lockfile for reproducible builds)
COPY pyproject.toml uv.lock README.md LICENSE ./
RUN uv sync --locked --no-dev --no-install-project

# Copy source
COPY src/ src/
RUN uv sync --locked --no-dev

# ── Runtime image
FROM python:3.12-slim

WORKDIR /app

# Install uv so we can run with proper path resolution
COPY --from=ghcr.io/astral-sh/uv:latest /uv /usr/local/bin/uv

# Install curl for health checks
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy the built project
COPY --from=builder /build /app

# Make sure the venv bin is on PATH for direct entrypoint use
ENV PATH="/app/.venv/bin:$PATH"

# Headless defaults (can be overridden at runtime via .env or env vars)
ENV SUUNTO_TOKEN_STORE=file
ENV SUUNTO_TOKEN_FILE=/data/tokens.json
ENV SUUNTO_API_QUOTA_FILE=/data/api-quota.json
ENV SUUNTO_LOCAL_DATA_DIR=/data/data
ENV SUUNTO_EXPORT_DIR=/data/exports
ENV SUUNTO_WEBHOOK_STORE=sqlite
ENV SUUNTO_WEBHOOK_STORE_PATH=/data/webhooks.sqlite3

# Default transport: streamable HTTP on port 8000
EXPOSE 8000

ENTRYPOINT ["uv", "run", "suunto-mcp"]
CMD ["--transport", "streamable-http", "--host", "0.0.0.0", "--port", "8000"]
