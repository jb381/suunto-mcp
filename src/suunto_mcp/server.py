from __future__ import annotations

import json
from typing import Any, cast

from fastmcp import FastMCP
from starlette.requests import Request
from starlette.responses import JSONResponse

from suunto_mcp import package_version
from suunto_mcp.config import settings
from suunto_mcp.quota import quota_status
from suunto_mcp.tools import (
    activity,
    guides,
    imports,
    oauth,
    register_safety_tools,
    routes,
    uploads,
    webhooks,
    workouts,
)
from suunto_mcp.webhooks import (
    classify_webhook_kind,
    store_webhook_event,
    verify_webhook_signature,
)

INSTRUCTIONS = """# Suunto MCP Server

MCP server for accessing and analyzing Suunto activity, workout, and health data.

## Quick Start

### With a Suunto API subscription key
1. Set SUUNTO_SUBSCRIPTION_KEY, SUUNTO_CLIENT_ID, SUUNTO_CLIENT_SECRET, and SUUNTO_REDIRECT_URI
2. Use `suunto_get_authorization_url` to get a browser URL
3. After the user authorizes, use `suunto_exchange_authorization_code` to store their tokens
4. Use `suunto_list_accounts` to see active accounts
5. Start querying: `suunto_list_workouts`, `suunto_get_workout`, etc.

### Without Suunto API credentials (local / no-key mode)
1. Import FIT or GPX files from your local filesystem
2. Use `suunto_import_fit_file` or `suunto_import_gpx_file` to register them
3. Parse and analyze with `suunto_parse_fit_file`, `suunto_parse_gpx_file`, or
   `suunto_parse_health_export`
4. All imported data can be assigned to an `account_id` for multi-user workflows

## Available API Areas
- **Workouts**: list, get, export FIT, parse FIT
- **24/7 Data**: activity samples, sleep, recovery, daily statistics
- **Routes**: list, get, export GPX, import GPX
- **Local Imports**: parse FIT/GPX files, Apple Health exports
- **OAuth**: manage Suunto user authorizations
- **Guides**: SuuntoPlus guide management
- **Uploads**: push FIT workouts to Suunto App
"""

mcp = FastMCP(
    "Suunto MCP",
    instructions=INSTRUCTIONS,
    version=package_version(),
)


@mcp.resource("suunto://config")
def config_resource() -> dict[str, Any]:
    """Return redacted Suunto MCP configuration."""
    return settings.redacted_display()


@mcp.resource("suunto://api-catalog")
def api_catalog_resource() -> dict[str, Any]:
    """Return the implemented Suunto MCP tool catalog."""
    return {
        "oauth": [
            "suunto_get_authorization_url",
            "suunto_exchange_authorization_code",
            "suunto_import_token_set",
            "suunto_import_token_file",
            "suunto_refresh_token",
            "suunto_deauthorize",
            "suunto_list_accounts",
            "suunto_auth_status",
        ],
        "workouts": [
            "suunto_list_workouts",
            "suunto_get_workout",
            "suunto_get_workout_fit",
            "suunto_parse_workout_fit",
            "suunto_get_complete_workout",
            "suunto_add_workout_description_info",
            "suunto_list_workouts_deprecated_v2",
            "suunto_get_workout_deprecated_v2",
            "suunto_get_workout_fit_deprecated_v2",
        ],
        "activity_247": [
            "suunto_get_activity_samples",
            "suunto_get_daily_activity_statistics",
            "suunto_get_sleep_data",
            "suunto_get_recovery_data",
            "suunto_estimate_247_summary_calls",
            "suunto_get_247_summary",
            "suunto_get_legacy_daily_activity",
            "suunto_get_legacy_daily_activity_statistics",
        ],
        "routes": [
            "suunto_list_routes",
            "suunto_get_route",
            "suunto_export_route_gpx",
            "suunto_parse_route_gpx",
            "suunto_import_route_gpx",
        ],
        "uploads": [
            "suunto_init_workout_upload",
            "suunto_upload_workout_fit_to_signed_url",
            "suunto_get_upload_status",
            "suunto_wait_for_upload_status",
            "suunto_upload_workout_fit",
        ],
        "guides": [
            "suunto_list_guides",
            "suunto_get_guide",
            "suunto_validate_guide_json",
            "suunto_create_guide_zip",
            "suunto_create_guide",
            "suunto_update_guide",
            "suunto_delete_guide",
        ],
        "imports": [
            "suunto_parse_fit_file",
            "suunto_parse_gpx_file",
            "suunto_import_fit_file",
            "suunto_import_gpx_file",
            "suunto_import_activity_json",
            "suunto_parse_activity_csv",
            "suunto_import_activity_csv",
            "suunto_parse_health_export",
            "suunto_import_health_export",
            "suunto_list_imported_files",
            "suunto_get_imported_activity",
        ],
        "webhooks": [
            "suunto_verify_webhook_signature",
            "suunto_store_webhook_event",
            "suunto_list_webhook_events",
            "suunto_get_webhook_event",
            "suunto_delete_webhook_event",
            "suunto_suggest_webhook_followups",
        ],
        "webhook_endpoints": [
            "POST /webhooks/suunto",
            "POST /webhooks/suunto/workout",
            "POST /webhooks/suunto/route",
            "POST /webhooks/suunto/247/activity",
            "POST /webhooks/suunto/247/sleep",
            "POST /webhooks/suunto/247/recovery",
            "POST /webhooks/suunto/legacy-workout",
        ],
        "safety": [
            "suunto_api_quota_status",
        ],
    }


@mcp.resource("suunto://coverage")
def coverage_resource() -> dict[str, Any]:
    """Return a compact Suunto API coverage audit for implemented tools and routes."""
    return {
        "sources": {
            "api_catalog": "https://apizone.suunto.com/apis",
            "oauth": "https://apizone.suunto.com/how-to-start",
            "webhooks": "https://apizone.suunto.com/webhooks",
            "routes": "https://apizone.suunto.com/route-description",
            "uploads": "https://apizone.suunto.com/how-to-workout-upload",
            "guides": "https://apizone.suunto.com/suuntoplus-guide-description",
            "fit": "https://apizone.suunto.com/fit-description",
        },
        "api_groups": {
            "SUUNTO AUTHORISATION API": {
                "operations": [
                    "GET /oauth/authorize",
                    "POST /oauth/token",
                    "POST /oauth/deauthorize",
                ],
                "tools": [
                    "suunto_get_authorization_url",
                    "suunto_exchange_authorization_code",
                    "suunto_refresh_token",
                    "suunto_deauthorize",
                ],
            },
            "SUUNTO WORKOUT API": {
                "operations": [
                    "GET /v3/workouts",
                    "GET /v3/workouts/{id}",
                    "GET /v3/workouts/{id}/fit",
                    "POST /v1/workouts/addinfo/{id}",
                ],
                "tools": [
                    "suunto_list_workouts",
                    "suunto_get_workout",
                    "suunto_get_workout_fit",
                    "suunto_parse_workout_fit",
                    "suunto_get_complete_workout",
                    "suunto_add_workout_description_info",
                ],
            },
            "SUUNTO WORKOUT API (DEPRECATED)": {
                "operations": [
                    "GET /v2/workouts",
                    "GET /v2/workouts/{workoutKey}",
                    "GET /v2/workouts/{workoutKey}/fit",
                ],
                "tools": [
                    "suunto_list_workouts_deprecated_v2",
                    "suunto_get_workout_deprecated_v2",
                    "suunto_get_workout_fit_deprecated_v2",
                ],
            },
            "SUUNTO 247 DATA API": {
                "operations": [
                    "GET /247samples/activity",
                    "GET /247samples/sleep",
                    "GET /247samples/recovery",
                    "GET /247samples/daily-activity-statistics",
                    "Local 24/7 summary quota estimate",
                ],
                "tools": [
                    "suunto_get_activity_samples",
                    "suunto_get_sleep_data",
                    "suunto_get_recovery_data",
                    "suunto_get_daily_activity_statistics",
                    "suunto_estimate_247_summary_calls",
                    "suunto_get_247_summary",
                ],
            },
            "SUUNTO DAILY ACTIVITY API": {
                "operations": [
                    "GET /247",
                    "GET /247/daily-activity-statistics",
                ],
                "tools": [
                    "suunto_get_legacy_daily_activity",
                    "suunto_get_legacy_daily_activity_statistics",
                ],
            },
            "SUUNTO ROUTE API": {
                "operations": [
                    "GET /v2/route",
                    "GET /v2/route/{id}",
                    "GET /v2/route/{id}/export",
                    "POST /v2/route/import",
                    "POST /webhooks/suunto/route",
                ],
                "tools": [
                    "suunto_list_routes",
                    "suunto_get_route",
                    "suunto_export_route_gpx",
                    "suunto_parse_route_gpx",
                    "suunto_import_route_gpx",
                ],
            },
            "SUUNTO UPLOAD API": {
                "operations": [
                    "POST /v2/upload",
                    "PUT signed object-storage URL",
                    "GET /v2/upload/{id}",
                    "Poll GET /v2/upload/{id}",
                ],
                "tools": [
                    "suunto_init_workout_upload",
                    "suunto_upload_workout_fit_to_signed_url",
                    "suunto_get_upload_status",
                    "suunto_wait_for_upload_status",
                    "suunto_upload_workout_fit",
                ],
            },
            "SUUNTO GUIDES API": {
                "operations": [
                    "GET /v2/guides/items",
                    "GET /v2/guides/files/{id}",
                    "POST /v2/guides/files",
                    "PUT /v2/guides/files/{id}",
                    "DELETE /v2/guides/files/{id}",
                ],
                "tools": [
                    "suunto_list_guides",
                    "suunto_get_guide",
                    "suunto_validate_guide_json",
                    "suunto_create_guide_zip",
                    "suunto_create_guide",
                    "suunto_update_guide",
                    "suunto_delete_guide",
                ],
            },
            "WEBHOOK NOTIFICATIONS": {
                "operations": [
                    "POST /webhooks/suunto",
                    "POST /webhooks/suunto/workout",
                    "POST /webhooks/suunto/route",
                    "POST /webhooks/suunto/247/activity",
                    "POST /webhooks/suunto/247/sleep",
                    "POST /webhooks/suunto/247/recovery",
                    "POST /webhooks/suunto/legacy-workout",
                ],
                "tools": [
                    "suunto_verify_webhook_signature",
                    "suunto_store_webhook_event",
                    "suunto_list_webhook_events",
                    "suunto_get_webhook_event",
                    "suunto_delete_webhook_event",
                    "suunto_suggest_webhook_followups",
                ],
            },
            "NO-KEY LOCAL IMPORTS": {
                "operations": [
                    "Parse local FIT files",
                    "Parse local GPX files",
                    "Parse local Apple Health export.xml files",
                    "Register normalized JSON files",
                    "Parse and register CSV/TSV files",
                    "List and re-parse imported files",
                ],
                "tools": [
                    "suunto_parse_fit_file",
                    "suunto_import_fit_file",
                    "suunto_parse_gpx_file",
                    "suunto_import_gpx_file",
                    "suunto_parse_health_export",
                    "suunto_import_health_export",
                    "suunto_import_activity_json",
                    "suunto_parse_activity_csv",
                    "suunto_import_activity_csv",
                    "suunto_list_imported_files",
                    "suunto_get_imported_activity",
                ],
            },
        },
        "safety": {
            "write_gate": (
                "SUUNTO_ENABLE_WRITE_TOOLS must be true for route import, uploads, "
                "guide mutations, and workout add-info."
            ),
            "quota_guard": (
                "SUUNTO_API_CALLS_PER_MINUTE and SUUNTO_API_WEEKLY_CALL_LIMIT guard "
                "Cloud API requests locally."
            ),
            "http_auth": "Non-local HTTP/SSE transport requires SUUNTO_MCP_API_TOKEN.",
        },
    }


@mcp.resource("suunto://quota")
def quota_resource() -> dict[str, Any]:
    """Return local Suunto API quota ledger status."""
    return quota_status(settings)


def _webhook_signature_valid(raw_body: bytes, request: Request) -> bool | None:
    signature = request.headers.get("x-hmac-sha256-signature")
    if not settings.WEBHOOK_SECRET:
        return None
    if not signature:
        return False
    return verify_webhook_signature(raw_body, signature)


async def _store_webhook_payload(
    *,
    request: Request,
    raw_body: bytes,
    kind: str,
    payload: dict[str, Any],
) -> JSONResponse:
    signature_valid = _webhook_signature_valid(raw_body, request)
    if signature_valid is False:
        return JSONResponse({"ok": False, "error": "invalid_signature"}, status_code=401)

    stored = store_webhook_event(
        {
            "kind": classify_webhook_kind(payload, fallback=kind),
            "signature_verified": signature_valid,
            "content_type": request.headers.get("content-type"),
            "payload": payload,
        }
    )
    return JSONResponse({"ok": True, "event_id": stored["id"]})


async def _json_webhook_payload(raw_body: bytes) -> dict[str, Any] | JSONResponse:
    try:
        payload_obj = json.loads(raw_body)
    except json.JSONDecodeError:
        return JSONResponse({"ok": False, "error": "invalid_json"}, status_code=400)
    if not isinstance(payload_obj, dict):
        return JSONResponse({"ok": False, "error": "expected_json_object"}, status_code=400)
    return cast(dict[str, Any], payload_obj)


@mcp.custom_route("/webhooks/suunto", methods=["POST"], include_in_schema=False)
async def suunto_webhook(request: Request) -> JSONResponse:
    raw_body = await request.body()
    payload = await _json_webhook_payload(raw_body)
    if isinstance(payload, JSONResponse):
        return payload
    return await _store_webhook_payload(
        request=request,
        raw_body=raw_body,
        kind="suunto-json",
        payload=payload,
    )


async def _suunto_typed_json_webhook(request: Request, fallback_kind: str) -> JSONResponse:
    raw_body = await request.body()
    payload = await _json_webhook_payload(raw_body)
    if isinstance(payload, JSONResponse):
        return payload
    return await _store_webhook_payload(
        request=request,
        raw_body=raw_body,
        kind=fallback_kind,
        payload=payload,
    )


@mcp.custom_route("/webhooks/suunto/workout", methods=["POST"], include_in_schema=False)
async def suunto_workout_webhook(request: Request) -> JSONResponse:
    return await _suunto_typed_json_webhook(request, "suunto-workout")


@mcp.custom_route("/webhooks/suunto/route", methods=["POST"], include_in_schema=False)
async def suunto_route_webhook(request: Request) -> JSONResponse:
    return await _suunto_typed_json_webhook(request, "suunto-route")


@mcp.custom_route("/webhooks/suunto/247/activity", methods=["POST"], include_in_schema=False)
async def suunto_247_activity_webhook(request: Request) -> JSONResponse:
    return await _suunto_typed_json_webhook(request, "suunto-247-activity")


@mcp.custom_route("/webhooks/suunto/247/sleep", methods=["POST"], include_in_schema=False)
async def suunto_247_sleep_webhook(request: Request) -> JSONResponse:
    return await _suunto_typed_json_webhook(request, "suunto-247-sleep")


@mcp.custom_route("/webhooks/suunto/247/recovery", methods=["POST"], include_in_schema=False)
async def suunto_247_recovery_webhook(request: Request) -> JSONResponse:
    return await _suunto_typed_json_webhook(request, "suunto-247-recovery")


@mcp.custom_route("/webhooks/suunto/legacy-workout", methods=["POST"], include_in_schema=False)
async def suunto_legacy_workout_webhook(request: Request) -> JSONResponse:
    raw_body = await request.body()
    form = await request.form()
    payload = {
        "username": form.get("username"),
        "workoutid": form.get("workoutid"),
    }
    return await _store_webhook_payload(
        request=request,
        raw_body=raw_body,
        kind="suunto-legacy-workout",
        payload=payload,
    )


oauth.register(mcp)
workouts.register(mcp)
activity.register(mcp)
routes.register(mcp)
uploads.register(mcp)
guides.register(mcp)
imports.register(mcp)
webhooks.register(mcp)
register_safety_tools(mcp)
