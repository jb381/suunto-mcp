# Suunto MCP

[![CI](https://github.com/jb381/suunto-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/jb381/suunto-mcp/actions/workflows/ci.yml)

Turn your Suunto history into something you can actually talk to.

Suunto MCP is a [FastMCP](https://gofastmcp.com/) server for Suunto Cloud API
workouts, routes, and 24/7 activity, sleep, and recovery data. It gives MCP
clients a practical, quota-aware way to explore training data, with supporting
tools for local FIT/GPX files, Apple Health exports, and Suunto webhook
workflows when you want more than the cloud API alone.

<p>
  <img src="assets/example-single-day.png" alt="Single-day Suunto MCP analysis example" width="49%">
  <img src="assets/example-bikepacking.png" alt="Bikepacking Suunto MCP analysis example" width="49%">
</p>

## What You Can Do

- Ask for recent workouts, full workout details, FIT downloads, and parsed
  workout summaries.
- Compare activity, sleep, recovery, and daily 24/7 statistics without burning
  through Suunto's weekly API quota by accident.
- Export and inspect routes as GPX.
- Receive Suunto webhooks, verify HMAC signatures, store events locally, and
  decide which follow-up fetch is worth making.
- Parse local FIT, GPX, CSV/TSV, normalized JSON, and Apple Health `export.xml`
  files, even without Suunto API credentials.
- Enable write tools only when you intentionally want route imports, workout
  uploads, SuuntoPlus Guide mutations, or workout add-info calls.

Write/push tools stay off unless you explicitly set
`SUUNTO_ENABLE_WRITE_TOOLS=true`.

## Quick Start

Requirements:

- Python 3.12 or newer
- [`uv`](https://docs.astral.sh/uv/)
- Suunto API Zone credentials for cloud access

Local file parsing works without Suunto credentials, so you can kick the tires
before wiring up cloud access.

```bash
git clone https://github.com/jb381/suunto-mcp.git
cd suunto-mcp
uv sync
cp .env.example .env
```

Edit `.env`:

```env
SUUNTO_CLIENT_ID=your_client_id
SUUNTO_CLIENT_SECRET=your_client_secret
SUUNTO_REDIRECT_URI=http://localhost:8080/callback
SUUNTO_SUBSCRIPTION_KEY=your_subscription_key

SUUNTO_TOKEN_STORE=keyring
SUUNTO_API_CALLS_PER_MINUTE=10
SUUNTO_API_WEEKLY_CALL_LIMIT=200
```

In Suunto API Zone, set the OAuth redirect URI to exactly the same value as
`SUUNTO_REDIRECT_URI`.

Run the stdio MCP server:

```bash
uv run suunto-mcp
```

## Docker

Build and run the server as a container for headless / production deployments:

```bash
docker build -t suunto-mcp .
docker run -d --name suunto-mcp                                     \
  -p 8000:8000                                                       \
  --env-file .env                                                    \
  -v suunto-mcp-data:/data                                           \
  suunto-mcp
```

The image ships with headless-friendly defaults:

- **Token storage**: `file` instead of `keyring` (no desktop session needed)
- **Data paths**: all persistent state lives under `/data` (tokens, quota ledger, exports, webhook store) — mount a volume there
- **Transport**: streamable HTTP on port 8000
- **API token required**: Binding HTTP on `0.0.0.0` requires setting `SUUNTO_MCP_API_TOKEN` in your `.env` file — the server will refuse to start without it. Use a strong random value and keep it secret.

### docker compose

A ready-to-use `docker-compose.yml` lives in the repository root. Create your
`.env` file (copy from `.env.example`), then:

```bash
docker compose up -d
```

The compose file mounts a named volume for persistent data and includes a
health check on the MCP endpoint.

### Kubernetes

A `kubernetes.yaml` manifest is provided for Kubernetes deployments. It
includes a Deployment, Service, PersistentVolumeClaim, a dedicated Namespace,
and a placeholder Secret for API credentials.

```bash
# Edit the secret with your credentials first, then:
kubectl apply -f kubernetes.yaml
```

## Add It To An MCP Client

Most MCP clients can run the server directly over stdio. Point the client at
your checkout:

```json
{
  "mcpServers": {
    "suunto": {
      "command": "uv",
      "args": ["run", "--directory", "/absolute/path/to/suunto-mcp", "suunto-mcp"]
    }
  }
}
```

[OpenCode](https://opencode.ai/) works too. Use a local server entry like this:

```json
{
  "suunto": {
    "enabled": true,
    "type": "local",
    "command": ["uv", "run", "--directory", "/path/to/suunto-mcp", "suunto-mcp"]
  }
}
```

Prefer HTTP while testing locally? Start the streamable HTTP transport:

```bash
uv run suunto-mcp --transport streamable-http --host 127.0.0.1 --port 8000
```

Then connect your client to:

```text
http://127.0.0.1:8000/mcp
```

If you bind HTTP, SSE, or streamable HTTP to a non-local host, set
`SUUNTO_MCP_API_TOKEN`. The server refuses non-local HTTP binds without a token.

## First Suunto Authorization

Once the server is running in your MCP client, do the first OAuth handshake:

1. Call `suunto_get_authorization_url`.
2. Open the returned URL and authorize with your Suunto App account.
3. Copy the `code` from the redirect URL.
4. Call `suunto_exchange_authorization_code` with that code.
5. Call `suunto_list_accounts`.
6. Try `suunto_list_workouts`.

For command-line OAuth debugging, the same flow is available by hand:

```bash
source .env
open "https://cloudapi-oauth.suunto.com/oauth/authorize?response_type=code&client_id=$SUUNTO_CLIENT_ID&redirect_uri=$SUUNTO_REDIRECT_URI"
```

Then exchange the returned code:

```bash
curl https://cloudapi-oauth.suunto.com/oauth/token \
  --user "$SUUNTO_CLIENT_ID:$SUUNTO_CLIENT_SECRET" \
  -d grant_type=authorization_code \
  -d redirect_uri="$SUUNTO_REDIRECT_URI" \
  -d code="PASTE_CODE_HERE"
```

Import the returned token JSON with `suunto_import_token_set` or
`suunto_import_token_file`, then you are ready to ask real questions.

## Example Workflows

Good starting prompts:

- "Show my last 10 Suunto workouts and flag anything unusually long or intense."
- "Fetch the full details for yesterday's run and summarize pace, heart rate, and
  elevation."
- "Parse this GPX file and tell me distance, ascent, and whether the route looks
  like a loop."
- "Import this Apple Health export and summarize workouts by type."
- "List stored Suunto webhook events and suggest which ones need a follow-up API
  fetch."
- "Estimate how many 24/7 summary calls this date range will use before fetching
  it."

The server also exposes a few handy MCP resources:

- `suunto://config` shows redacted runtime configuration.
- `suunto://quota` shows local API quota usage.
- `suunto://coverage` maps Suunto API areas to implemented tools.
- `suunto://api-catalog` lists the tool groups.

See [docs/tools.md](docs/tools.md) for the full tool reference.

## Optional Settings

File token storage, useful during development:

```env
SUUNTO_TOKEN_STORE=file
SUUNTO_TOKEN_FILE=.suunto-mcp/tokens.json
```

HTTP transport bearer auth:

```env
SUUNTO_MCP_API_TOKEN=generate-a-long-random-token
```

Webhook verification and storage:

```env
SUUNTO_WEBHOOK_SECRET=generate-a-long-random-secret
SUUNTO_WEBHOOK_STORE=sqlite
SUUNTO_WEBHOOK_STORE_PATH=.suunto-mcp/webhooks.sqlite3
```

Write tools, deliberately boring by default:

```env
SUUNTO_ENABLE_WRITE_TOOLS=false
```

Keep that false unless you intentionally want write operations callable.

## Webhooks

HTTP transports expose:

- `POST /webhooks/suunto`
- `POST /webhooks/suunto/workout`
- `POST /webhooks/suunto/route`
- `POST /webhooks/suunto/247/activity`
- `POST /webhooks/suunto/247/sleep`
- `POST /webhooks/suunto/247/recovery`
- `POST /webhooks/suunto/legacy-workout`

When `SUUNTO_WEBHOOK_SECRET` is configured, webhook routes require a valid
`X-HMAC-SHA256-Signature` header. For anything you want to keep, prefer SQLite
storage.

Use `suunto_suggest_webhook_followups` on a stored event to find the next likely
fetch tool for workout, route, and 24/7 notifications.

## Safety

The server is designed to be useful without being adventurous with your data:

- `.env`, `.suunto-mcp/`, token files, webhook stores, quota ledgers, and exports
  are gitignored.
- Tool responses do not include stored access or refresh token values.
- Signed upload URLs are redacted in upload initialization responses.
- Non-local HTTP/SSE/streamable HTTP transports require `SUUNTO_MCP_API_TOKEN`.
- Cloud API calls are paced and counted locally with `SUUNTO_API_CALLS_PER_MINUTE`
  and `SUUNTO_API_WEEKLY_CALL_LIMIT`.
- Write/push tools require `SUUNTO_ENABLE_WRITE_TOOLS=true`.

Set `SUUNTO_API_WEEKLY_CALL_LIMIT=0` only when you intentionally want to disable
the local weekly guard.

## Troubleshooting

- If keyring storage fails on a headless machine or CI runner, switch to
  `SUUNTO_TOKEN_STORE=file` and keep `SUUNTO_TOKEN_FILE` inside `.suunto-mcp/`.
- If a cloud tool says the stored access token is expired and no refresh token is
  available, repeat OAuth or import a token response that includes
  `refresh_token`.
- If quota blocks a request, inspect `suunto://quota` or call
  `suunto_api_quota_status`. By default the ledger is stored next to
  `.suunto-mcp/data` as `api-quota.json`.
- If live API tests do not run, that is expected. They are skipped unless
  `SUUNTO_LIVE_TESTS=true` and `SUUNTO_LIVE_TOKEN_FILE` points at a valid token
  file.

## Development

```bash
uv sync --dev
uv run ruff format --check .
uv run ruff check .
uv run pyright
uv run pytest
```

Live read-only tests are opt-in:

```bash
SUUNTO_LIVE_TESTS=true \
SUUNTO_LIVE_TOKEN_FILE=.suunto-mcp/oauth-test-token.json \
uv run pytest tests/live
```

## Release Checklist

Before publishing:

1. Confirm `.env`, `.suunto-mcp/`, exported files, and token files are absent from
   `git status --short`.
2. Run `uv sync --dev --locked`.
3. Run the full development check suite.
4. Run `uv build`.
5. Keep `SUUNTO_ENABLE_WRITE_TOOLS=false` in examples and defaults.
6. Update `CHANGELOG.md`.
7. Tag the release, for example `git tag v0.1.0`.

## Suunto Docs

- [API catalog](https://apizone.suunto.com/apis)
- [Quick start and OAuth](https://apizone.suunto.com/how-to-start)
- [FAQ and operational notes](https://apizone.suunto.com/faq)

## License

MIT. See [LICENSE](LICENSE).
