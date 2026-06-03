from __future__ import annotations

from pathlib import Path
from typing import Any

from fastmcp import Client

from suunto_mcp.server import mcp
from suunto_mcp.tabular import parse_delimited_file


def _structured(result: Any) -> Any:
    data = getattr(result, "structured_content", None)
    if data is not None:
        return data
    data = getattr(result, "data", None)
    if data is not None:
        return data
    return result


def test_parse_delimited_file_sniffs_csv_and_bounds_preview(tmp_path: Path) -> None:
    source = tmp_path / "activity.csv"
    source.write_text(
        "start_time,duration_s,distance_m\n"
        "2026-06-01T08:00:00+02:00,1800,5000\n"
        "2026-06-02T08:00:00+02:00,1200,3000\n",
        encoding="utf-8",
    )

    parsed = parse_delimited_file(source, preview_limit=1)

    assert parsed["delimiter"] == ","
    assert parsed["fieldnames"] == ["start_time", "duration_s", "distance_m"]
    assert parsed["row_count"] == 2
    assert parsed["rows_included"] == 1
    assert parsed["rows"][0]["distance_m"] == "5000"


def test_parse_delimited_file_supports_tsv_without_header(tmp_path: Path) -> None:
    source = tmp_path / "activity.tsv"
    source.write_text("2026-06-01\t1800\t5000\n", encoding="utf-8")

    parsed = parse_delimited_file(source, delimiter="\t", has_header=False)

    assert parsed["delimiter"] == "\t"
    assert parsed["fieldnames"] == ["column_1", "column_2", "column_3"]
    assert parsed["row_count"] == 1
    assert parsed["rows"][0]["column_3"] == "5000"


async def test_activity_csv_tools_parse_import_list_and_get(tmp_path: Path, monkeypatch) -> None:
    source = tmp_path / "activity.csv"
    source.write_text(
        "start_time,duration_s,distance_m\n2026-06-01T08:00:00+02:00,1800,5000\n",
        encoding="utf-8",
    )
    monkeypatch.setattr("suunto_mcp.tools.imports.settings.LOCAL_DATA_DIR", str(tmp_path / "data"))

    async with Client(mcp) as client:
        parsed = _structured(
            await client.call_tool(
                "suunto_parse_activity_csv",
                {"path": str(source)},
            )
        )
        imported = _structured(
            await client.call_tool(
                "suunto_import_activity_csv",
                {"path": str(source), "copy_file": False},
            )
        )
        listed = _structured(await client.call_tool("suunto_list_imported_files", {"kind": "csv"}))
        loaded = _structured(
            await client.call_tool(
                "suunto_get_imported_activity",
                {"import_id": imported["imported"]["import_id"]},
            )
        )

    assert parsed["row_count"] == 1
    assert imported["imported"]["kind"] == "csv"
    assert imported["parsed"]["rows"][0]["distance_m"] == "5000"
    assert len(listed["imports"]) == 1
    assert loaded["parsed"]["row_count"] == 1
