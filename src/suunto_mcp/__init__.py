from __future__ import annotations

import argparse
from importlib.metadata import PackageNotFoundError, version

from suunto_mcp.config import settings
from suunto_mcp.server_auth import StaticBearerTokenVerifier

LOCAL_HTTP_HOSTS = {"127.0.0.1", "localhost", "::1"}


def package_version() -> str:
    try:
        return version("suunto-mcp")
    except PackageNotFoundError:  # pragma: no cover - only used from an editable checkout edge
        return "0.1.0"


def _base_url(host: str, port: int) -> str:
    bracketed_host = f"[{host}]" if ":" in host and not host.startswith("[") else host
    return f"http://{bracketed_host}:{port}"


def _configure_http_auth(host: str, port: int) -> StaticBearerTokenVerifier | None:
    token = settings.MCP_API_TOKEN
    if token:
        return StaticBearerTokenVerifier(token, base_url=_base_url(host, port))
    if host not in LOCAL_HTTP_HOSTS:
        raise SystemExit(
            "Refusing to start HTTP/SSE transport on a non-local host without "
            "SUUNTO_MCP_API_TOKEN. Set SUUNTO_MCP_API_TOKEN or bind to 127.0.0.1."
        )
    return None


def main() -> None:
    from suunto_mcp.server import mcp

    parser = argparse.ArgumentParser(prog="suunto-mcp")
    parser.add_argument(
        "--transport",
        choices=["stdio", "http", "sse", "streamable-http"],
        default="stdio",
        help="FastMCP transport to run.",
    )
    parser.add_argument("--host", default="127.0.0.1", help="HTTP/SSE bind host.")
    parser.add_argument("--port", type=int, default=8000, help="HTTP/SSE bind port.")
    args = parser.parse_args()

    if args.transport == "stdio":
        mcp.run()
    else:
        mcp.auth = _configure_http_auth(args.host, args.port)
        mcp.run(transport=args.transport, host=args.host, port=args.port)
