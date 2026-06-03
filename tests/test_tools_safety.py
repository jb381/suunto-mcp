from __future__ import annotations

import pytest

from suunto_mcp.tools import require_write_tools_enabled


def test_write_tools_are_disabled_by_default(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("suunto_mcp.tools.settings.ENABLE_WRITE_TOOLS", False)

    with pytest.raises(RuntimeError, match="SUUNTO_ENABLE_WRITE_TOOLS"):
        require_write_tools_enabled()
