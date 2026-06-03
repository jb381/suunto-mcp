# pyright: reportUnusedFunction=false
from __future__ import annotations

import json
from pathlib import Path
from typing import Any, cast

from fastmcp import FastMCP

from suunto_mcp.binary import OutputMode, handle_binary_output
from suunto_mcp.client import SuuntoClient
from suunto_mcp.guide import create_guide_zip, validate_guide_json
from suunto_mcp.tools import require_write_tools_enabled


def register(mcp: FastMCP) -> None:
    @mcp.tool(name="suunto_list_guides", description="List SuuntoPlus Guides for an account.")
    async def list_guides(account_id: str | None = None) -> Any:
        async with SuuntoClient(account_id=account_id) as client:
            return await client.get_json("/v2/guides/items")

    @mcp.tool(name="suunto_get_guide", description="Fetch a SuuntoPlus Guide ZIP/file by id.")
    async def get_guide(
        guide_id: str,
        account_id: str | None = None,
        output_mode: OutputMode = "metadata",
    ) -> dict[str, Any]:
        async with SuuntoClient(account_id=account_id) as client:
            data, content_type = await client.get_bytes(f"/v2/guides/files/{guide_id}")
        return handle_binary_output(
            data,
            suggested_filename=f"suunto-guide-{guide_id}.zip",
            content_type=content_type or "application/zip",
            output_mode=output_mode,
        )

    @mcp.tool(
        name="suunto_validate_guide_json",
        description="Validate a SuuntoPlus guide JSON object or guide.json file before upload.",
    )
    def validate_guide(
        guide: dict[str, Any] | None = None,
        guide_json_path: str | None = None,
    ) -> dict[str, Any]:
        guide_data = guide
        if guide_data is None:
            if guide_json_path is None:
                raise ValueError("Provide guide or guide_json_path.")
            loaded: object = json.loads(
                Path(guide_json_path).expanduser().read_text(encoding="utf-8")
            )
            if not isinstance(loaded, dict):
                raise ValueError("guide_json_path must contain a JSON object.")
            guide_data = cast(dict[str, Any], loaded)
        return validate_guide_json(guide_data)

    @mcp.tool(
        name="suunto_create_guide_zip",
        description="Create a local SuuntoPlus guide ZIP from guide.json and icon.png.",
    )
    def create_zip(guide_json_path: str, icon_png_path: str, output_path: str) -> dict[str, Any]:
        return create_guide_zip(guide_json_path, icon_png_path, output_path)

    @mcp.tool(
        name="suunto_create_guide",
        description="Upload a SuuntoPlus guide ZIP. Requires SUUNTO_ENABLE_WRITE_TOOLS=true.",
    )
    async def create_guide(zip_path: str, account_id: str | None = None) -> Any:
        require_write_tools_enabled()
        data = Path(zip_path).expanduser().read_bytes()
        async with SuuntoClient(account_id=account_id) as client:
            response, content_type = await client.post_bytes(
                "/v2/guides/files",
                content=data,
                content_type="application/zip",
                accept="application/json",
            )
        if content_type and "json" in content_type:
            return json.loads(response.decode("utf-8"))
        return {"uploaded": True, "response_text": response.decode("utf-8", errors="replace")}

    @mcp.tool(
        name="suunto_update_guide",
        description="Replace a SuuntoPlus guide ZIP. Requires SUUNTO_ENABLE_WRITE_TOOLS=true.",
    )
    async def update_guide(guide_id: str, zip_path: str, account_id: str | None = None) -> Any:
        require_write_tools_enabled()
        data = Path(zip_path).expanduser().read_bytes()
        async with SuuntoClient(account_id=account_id) as client:
            response, content_type = await client.put_bytes(
                f"/v2/guides/files/{guide_id}",
                content=data,
                content_type="application/zip",
                accept="application/json",
            )
        if content_type and "json" in content_type:
            return json.loads(response.decode("utf-8"))
        return {"updated": True, "response_text": response.decode("utf-8", errors="replace")}

    @mcp.tool(
        name="suunto_delete_guide",
        description="Delete a SuuntoPlus Guide. Requires SUUNTO_ENABLE_WRITE_TOOLS=true.",
    )
    async def delete_guide(guide_id: str, account_id: str | None = None) -> Any:
        require_write_tools_enabled()
        async with SuuntoClient(account_id=account_id) as client:
            return await client.delete_json(f"/v2/guides/files/{guide_id}")
