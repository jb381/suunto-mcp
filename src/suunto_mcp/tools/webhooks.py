# pyright: reportUnusedFunction=false
from __future__ import annotations

import base64
from typing import Any

from fastmcp import FastMCP

from suunto_mcp.webhooks import (
    delete_webhook_event,
    get_webhook_event,
    list_webhook_events,
    store_webhook_event,
    suggest_webhook_followups,
    verify_webhook_signature,
)


def register(mcp: FastMCP) -> None:
    @mcp.tool(
        name="suunto_verify_webhook_signature",
        description="Verify a Suunto webhook HMAC SHA-256 signature.",
    )
    def verify_signature(
        body: str,
        signature: str,
        body_is_base64: bool = False,
        secret: str | None = None,
    ) -> dict[str, Any]:
        raw_body = base64.b64decode(body) if body_is_base64 else body.encode("utf-8")
        return {"valid": verify_webhook_signature(raw_body, signature, secret=secret)}

    @mcp.tool(
        name="suunto_store_webhook_event",
        description="Store a webhook notification payload in the configured local webhook store.",
    )
    def store_event(event: dict[str, Any]) -> dict[str, Any]:
        return store_webhook_event(event)

    @mcp.tool(name="suunto_list_webhook_events", description="List locally stored webhook events.")
    def list_events(limit: int = 100) -> dict[str, Any]:
        return {"events": list_webhook_events(limit=limit)}

    @mcp.tool(
        name="suunto_get_webhook_event", description="Get one locally stored webhook event by id."
    )
    def get_event(event_id: str) -> dict[str, Any] | None:
        return get_webhook_event(event_id)

    @mcp.tool(
        name="suunto_delete_webhook_event",
        description="Delete one locally stored webhook event by id.",
    )
    def delete_event(event_id: str) -> dict[str, Any]:
        return {"deleted": delete_webhook_event(event_id)}

    @mcp.tool(
        name="suunto_suggest_webhook_followups",
        description="Suggest MCP fetch tools to call after receiving a Suunto webhook event.",
    )
    def suggest_followups(
        event: dict[str, Any] | None = None,
        event_id: str | None = None,
    ) -> dict[str, Any]:
        if event is None:
            if event_id is None:
                raise ValueError("Provide event or event_id.")
            event = get_webhook_event(event_id)
            if event is None:
                raise ValueError(f"No webhook event found for event_id={event_id!r}.")
        return suggest_webhook_followups(event)
