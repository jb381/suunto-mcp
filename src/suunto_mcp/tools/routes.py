# pyright: reportUnusedFunction=false
from __future__ import annotations

from pathlib import Path
from typing import Any

from fastmcp import FastMCP

from suunto_mcp.binary import OutputMode, handle_binary_output
from suunto_mcp.client import SuuntoClient
from suunto_mcp.gpx import parse_gpx_text
from suunto_mcp.tools import merge_params, require_write_tools_enabled


def register(mcp: FastMCP) -> None:
    @mcp.tool(
        name="suunto_list_routes", description="List routes for an authorized Suunto account."
    )
    async def list_routes(
        account_id: str | None = None,
        page: int | None = None,
        size: int | None = None,
        since: str | None = None,
        sort: str | None = None,
        query_params: dict[str, Any] | None = None,
    ) -> Any:
        params = merge_params(
            {"page": page, "size": size, "since": since, "sort": sort}, query_params
        )
        async with SuuntoClient(account_id=account_id) as client:
            return await client.get_json("/v2/route", params=params)

    @mcp.tool(name="suunto_get_route", description="Fetch one route metadata document by route id.")
    async def get_route(route_id: str, account_id: str | None = None) -> Any:
        async with SuuntoClient(account_id=account_id) as client:
            return await client.get_json(f"/v2/route/{route_id}")

    @mcp.tool(name="suunto_export_route_gpx", description="Export a Suunto route as GPX.")
    async def export_route_gpx(
        route_id: str,
        account_id: str | None = None,
        output_mode: OutputMode = "metadata",
    ) -> dict[str, Any]:
        async with SuuntoClient(account_id=account_id) as client:
            data, content_type = await client.get_bytes(
                f"/v2/route/{route_id}/export",
                accept="application/gpx+xml",
            )
        return handle_binary_output(
            data,
            suggested_filename=f"suunto-route-{route_id}.gpx",
            content_type=content_type or "application/gpx+xml",
            output_mode=output_mode,
        )

    @mcp.tool(
        name="suunto_parse_route_gpx",
        description="Export a Suunto route as GPX and parse tracks, routes, and waypoints.",
    )
    async def parse_route_gpx(
        route_id: str,
        account_id: str | None = None,
        include_points: bool = True,
        point_limit: int = 5000,
    ) -> dict[str, Any]:
        async with SuuntoClient(account_id=account_id) as client:
            data, _content_type = await client.get_bytes(
                f"/v2/route/{route_id}/export",
                accept="application/gpx+xml",
            )
        return parse_gpx_text(
            data.decode("utf-8"), include_points=include_points, point_limit=point_limit
        )

    @mcp.tool(
        name="suunto_import_route_gpx",
        description=(
            "Import a local GPX route into Suunto App. Requires SUUNTO_ENABLE_WRITE_TOOLS=true."
        ),
    )
    async def import_route_gpx(
        gpx_path: str,
        account_id: str | None = None,
        activity_ids: list[int] | None = None,
    ) -> Any:
        require_write_tools_enabled()
        data = Path(gpx_path).expanduser().read_bytes()
        params = (
            {"activities": ",".join(str(value) for value in activity_ids)} if activity_ids else None
        )
        async with SuuntoClient(account_id=account_id) as client:
            response, content_type = await client.post_bytes(
                "/v2/route/import",
                params=params,
                content=data,
                content_type="application/gpx+xml",
                accept="application/json",
            )
        if content_type and "json" in content_type:
            import json

            return json.loads(response.decode("utf-8"))
        return {"status": "uploaded", "response_text": response.decode("utf-8", errors="replace")}
