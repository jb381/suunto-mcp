from __future__ import annotations

import pytest

from suunto_mcp.binary import handle_binary_output
from suunto_mcp.config import Settings


def test_handle_binary_output_file_mode_fails_when_export_dir_unset() -> None:
    settings = Settings(EXPORT_DIR="")
    with pytest.raises(RuntimeError, match="SUUNTO_EXPORT_DIR"):
        handle_binary_output(
            b"data",
            suggested_filename="test.txt",
            output_mode="file",
            settings_obj=settings,
        )


def test_handle_binary_output_both_mode_falls_back_when_export_dir_unset() -> None:
    settings = Settings(EXPORT_DIR="", MAX_BASE64_BYTES=1024)
    result = handle_binary_output(
        b"data",
        suggested_filename="test.txt",
        output_mode="both",
        settings_obj=settings,
    )
    assert result["output_mode"] == "both"
    assert "base64" in result
    assert "path" not in result
