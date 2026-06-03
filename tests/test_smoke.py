from __future__ import annotations

from suunto_mcp.server import mcp


def test_server_created() -> None:
    assert mcp.name == "Suunto MCP"
    assert mcp.version == "0.1.0"
    instructions = mcp.instructions or ""
    assert "Quick Start" in instructions
