# Contributing

Thanks for helping improve Suunto MCP.

## Local Setup

```bash
uv sync --dev
cp .env.example .env
```

Fill `.env` only with your own Suunto API Zone credentials. Never commit real
credentials, token files, webhook payload stores, or exported activity data.

## Development Checks

Run the full local check suite before opening a pull request:

```bash
uv run ruff format .
uv run ruff check .
uv run pyright
uv run pytest
```

Live API tests are intentionally opt-in because Suunto developer accounts have
strict call limits:

```bash
SUUNTO_LIVE_TESTS=true \
SUUNTO_LIVE_TOKEN_FILE=.suunto-mcp/oauth-test-token.json \
uv run pytest tests/live
```

## Pull Request Expectations

- Keep changes focused and documented.
- Add or update tests for behavior changes.
- Preserve write-tool safety gates unless a change is explicitly about those gates.
- Redact tokens, subscription keys, signed upload URLs, and personal activity data
  from logs, screenshots, fixtures, and issue text.
