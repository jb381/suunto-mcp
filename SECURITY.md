# Security Policy

## Supported Versions

Security fixes target the current `main` branch until the project starts publishing
versioned releases.

## Reporting a Vulnerability

Please report suspected vulnerabilities privately by emailing the maintainer listed
in `pyproject.toml`. Do not include live Suunto access tokens, refresh tokens,
subscription keys, webhook secrets, or MCP bearer tokens in reports.

Helpful reports include:

- Affected tool, endpoint, or configuration.
- Steps to reproduce with redacted credentials.
- Impact and any known workarounds.

## Operational Security Notes

- Keep `.env`, token stores, quota ledgers, webhook stores, and exports out of git.
- Prefer `SUUNTO_TOKEN_STORE=keyring` for personal desktop use and `file` only when
  the token path is protected by filesystem permissions.
- Set `SUUNTO_MCP_API_TOKEN` before binding HTTP or SSE transports to non-local
  hosts.
- Set `SUUNTO_WEBHOOK_SECRET` when exposing webhook routes beyond local testing.
- Keep `SUUNTO_ENABLE_WRITE_TOOLS=false` unless you intentionally want route imports,
  workout uploads, guide mutations, or workout add-info tools to be callable.
