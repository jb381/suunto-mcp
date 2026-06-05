# pyright: reportUnusedFunction=false
from __future__ import annotations

import json
import os
import shutil
import uuid
from pathlib import Path
from typing import Any, Literal, cast

from fastmcp import FastMCP

from suunto_mcp.binary import sha256_bytes
from suunto_mcp.config import settings
from suunto_mcp.fit import parse_fit_file
from suunto_mcp.gpx import parse_gpx_file
from suunto_mcp.health import parse_apple_health_export
from suunto_mcp.models import ImportedFileRecord
from suunto_mcp.tabular import parse_delimited_file

ImportKind = Literal["fit", "gpx", "json", "health", "csv"]


def _data_dir() -> Path:
    path = Path(settings.LOCAL_DATA_DIR).expanduser()
    path.mkdir(parents=True, exist_ok=True)
    return path


def _index_path() -> Path:
    return _data_dir() / "imports.json"


def _read_index() -> dict[str, Any]:
    path = _index_path()
    if not path.exists():
        return {"imports": {}}
    data_obj: object = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data_obj, dict):
        raise ValueError(f"Invalid import index at {path}.")
    data = cast(dict[str, Any], data_obj)
    if not isinstance(data.get("imports"), dict):
        raise ValueError(f"Invalid import index at {path}.")
    return data


def _write_index(data: dict[str, Any]) -> None:
    path = _index_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(data, indent=2, sort_keys=True), encoding="utf-8")
    os.replace(tmp, path)


def _record_import(
    path: str,
    *,
    kind: ImportKind,
    account_id: str | None = None,
    copy_file: bool = True,
    parsed_summary: dict[str, Any] | None = None,
) -> ImportedFileRecord:
    source = Path(path).expanduser()
    raw = source.read_bytes()
    digest = sha256_bytes(raw)
    stored_path: str | None = None
    if copy_file:
        imports_dir = _data_dir() / "files"
        imports_dir.mkdir(parents=True, exist_ok=True)
        target = imports_dir / f"{digest[:16]}-{source.name}"
        if not target.exists():
            shutil.copy2(source, target)
        stored_path = str(target)
    record = ImportedFileRecord(
        import_id=str(uuid.uuid4()),
        source_path=str(source),
        stored_path=stored_path,
        account_id=account_id,
        kind=kind,
        sha256=digest,
        size=len(raw),
        parsed_summary=parsed_summary or {},
    )
    index = _read_index()
    index["imports"][record.import_id] = record.model_dump(mode="json")
    _write_index(index)
    return record


def _load_record(import_id: str) -> ImportedFileRecord:
    data = _read_index()["imports"].get(import_id)
    if not data:
        raise ValueError(f"No imported file found for import_id={import_id!r}.")
    return ImportedFileRecord.model_validate(data)


def _path_for_record(record: ImportedFileRecord) -> Path:
    return Path(record.stored_path or record.source_path).expanduser()


def register(mcp: FastMCP) -> None:
    @mcp.tool(
        name="suunto_parse_fit_file", description="Parse a local FIT file without storing it."
    )
    def parse_fit(
        path: str,
        include_records: bool = False,
        record_limit: int = 1000,
        include_messages: bool = False,
        message_limit: int = 2000,
    ) -> dict[str, Any]:
        return parse_fit_file(
            path,
            include_records=include_records,
            record_limit=record_limit,
            include_messages=include_messages,
            message_limit=message_limit,
        )

    @mcp.tool(
        name="suunto_parse_gpx_file", description="Parse a local GPX file without storing it."
    )
    def parse_gpx(
        path: str, include_points: bool = True, point_limit: int = 5000
    ) -> dict[str, Any]:
        return parse_gpx_file(path, include_points=include_points, point_limit=point_limit)

    @mcp.tool(
        name="suunto_import_fit_file",
        description=(
            "Register a local FIT file and cache a parsed summary in the local import index."
        ),
    )
    def import_fit_file(
        path: str,
        account_id: str | None = None,
        copy_file: bool = True,
        include_records: bool = False,
        record_limit: int = 1000,
    ) -> dict[str, Any]:
        parsed = parse_fit_file(path, include_records=include_records, record_limit=record_limit)
        record = _record_import(
            path,
            kind="fit",
            account_id=account_id,
            copy_file=copy_file,
            parsed_summary={
                "message_counts": parsed.get("message_counts", {}),
                "sessions": parsed.get("sessions", []),
                "laps_count": len(parsed.get("laps", [])),
                "events_count": len(parsed.get("events", [])),
            },
        )
        return {"imported": record.safe_dict(), "parsed": parsed}

    @mcp.tool(
        name="suunto_import_gpx_file",
        description="Register a local GPX route/track file and cache a parsed summary.",
    )
    def import_gpx_file(
        path: str,
        account_id: str | None = None,
        copy_file: bool = True,
        include_points: bool = True,
        point_limit: int = 5000,
    ) -> dict[str, Any]:
        parsed = parse_gpx_file(path, include_points=include_points, point_limit=point_limit)
        record = _record_import(
            path,
            kind="gpx",
            account_id=account_id,
            copy_file=copy_file,
            parsed_summary={
                "metadata": parsed.get("metadata", {}),
                "track_count": parsed.get("track_count"),
                "route_count": parsed.get("route_count"),
                "waypoint_count": parsed.get("waypoint_count"),
            },
        )
        return {"imported": record.safe_dict(), "parsed": parsed}

    @mcp.tool(
        name="suunto_import_activity_json",
        description="Register a local normalized activity JSON file.",
    )
    def import_activity_json(
        path: str,
        account_id: str | None = None,
        copy_file: bool = True,
    ) -> dict[str, Any]:
        source = Path(path).expanduser()
        parsed_obj: object = json.loads(source.read_text(encoding="utf-8"))
        parsed_dict = cast(dict[str, Any], parsed_obj) if isinstance(parsed_obj, dict) else None
        parsed_list = cast(list[Any], parsed_obj) if isinstance(parsed_obj, list) else None
        if parsed_dict is not None:
            top_level_type = "dict"
        elif parsed_list is not None:
            top_level_type = "list"
        elif parsed_obj is None:
            top_level_type = "null"
        elif isinstance(parsed_obj, str):
            top_level_type = "str"
        elif isinstance(parsed_obj, bool):
            top_level_type = "bool"
        elif isinstance(parsed_obj, int):
            top_level_type = "int"
        elif isinstance(parsed_obj, float):
            top_level_type = "float"
        else:
            top_level_type = "unknown"
        summary: dict[str, Any] = {
            "top_level_type": top_level_type,
            "keys": list(parsed_dict.keys()) if parsed_dict is not None else None,
            "items": len(parsed_list) if parsed_list is not None else None,
        }
        record = _record_import(
            path,
            kind="json",
            account_id=account_id,
            copy_file=copy_file,
            parsed_summary=summary,
        )
        return {"imported": record.safe_dict(), "summary": summary}

    @mcp.tool(
        name="suunto_parse_activity_csv",
        description="Parse a local CSV/TSV activity file without storing it.",
    )
    def parse_activity_csv(
        path: str,
        delimiter: str | None = None,
        has_header: bool = True,
        preview_limit: int = 100,
        encoding: str = "utf-8-sig",
    ) -> dict[str, Any]:
        return parse_delimited_file(
            path,
            delimiter=delimiter,
            has_header=has_header,
            preview_limit=preview_limit,
            encoding=encoding,
        )

    @mcp.tool(
        name="suunto_import_activity_csv",
        description="Register a local CSV/TSV activity file and cache a bounded preview summary.",
    )
    def import_activity_csv(
        path: str,
        account_id: str | None = None,
        copy_file: bool = True,
        delimiter: str | None = None,
        has_header: bool = True,
        preview_limit: int = 100,
        encoding: str = "utf-8-sig",
    ) -> dict[str, Any]:
        parsed = parse_delimited_file(
            path,
            delimiter=delimiter,
            has_header=has_header,
            preview_limit=preview_limit,
            encoding=encoding,
        )
        summary = {
            "delimiter": parsed.get("delimiter"),
            "has_header": parsed.get("has_header"),
            "fieldnames": parsed.get("fieldnames", []),
            "row_count": parsed.get("row_count"),
            "rows_included": parsed.get("rows_included"),
        }
        record = _record_import(
            path,
            kind="csv",
            account_id=account_id,
            copy_file=copy_file,
            parsed_summary=summary,
        )
        return {"imported": record.safe_dict(), "parsed": parsed}

    @mcp.tool(
        name="suunto_parse_health_export",
        description="Parse a local Apple Health export.xml file without storing it.",
    )
    def parse_health_export(
        path: str,
        include_records: bool = False,
        record_limit: int = 1000,
        include_workouts: bool = True,
        workout_limit: int = 500,
        include_activity_summaries: bool = True,
        activity_summary_limit: int = 500,
    ) -> dict[str, Any]:
        return parse_apple_health_export(
            path,
            include_records=include_records,
            record_limit=record_limit,
            include_workouts=include_workouts,
            workout_limit=workout_limit,
            include_activity_summaries=include_activity_summaries,
            activity_summary_limit=activity_summary_limit,
        )

    @mcp.tool(
        name="suunto_import_health_export",
        description="Register a local Apple Health export.xml file and cache a bounded summary.",
    )
    def import_health_export(
        path: str,
        account_id: str | None = None,
        copy_file: bool = True,
        include_records: bool = False,
        record_limit: int = 1000,
        include_workouts: bool = True,
        workout_limit: int = 500,
        include_activity_summaries: bool = True,
        activity_summary_limit: int = 500,
    ) -> dict[str, Any]:
        parsed = parse_apple_health_export(
            path,
            include_records=include_records,
            record_limit=record_limit,
            include_workouts=include_workouts,
            workout_limit=workout_limit,
            include_activity_summaries=include_activity_summaries,
            activity_summary_limit=activity_summary_limit,
        )
        summary = {
            "element_counts": parsed.get("element_counts", {}),
            "record_counts": parsed.get("record_counts", {}),
            "workout_counts": parsed.get("workout_counts", {}),
            "records_included": parsed.get("records_included"),
            "workouts_included": parsed.get("workouts_included"),
            "activity_summaries_included": parsed.get("activity_summaries_included"),
        }
        record = _record_import(
            path,
            kind="health",
            account_id=account_id,
            copy_file=copy_file,
            parsed_summary=summary,
        )
        return {"imported": record.safe_dict(), "parsed": parsed}

    @mcp.tool(name="suunto_list_imported_files", description="List locally imported files.")
    def list_imported_files(
        account_id: str | None = None, kind: ImportKind | None = None
    ) -> dict[str, Any]:
        records = [
            ImportedFileRecord.model_validate(value) for value in _read_index()["imports"].values()
        ]
        if account_id:
            records = [record for record in records if record.account_id == account_id]
        if kind:
            records = [record for record in records if record.kind == kind]
        return {"imports": [record.safe_dict() for record in records]}

    @mcp.tool(
        name="suunto_get_imported_activity", description="Inspect and parse an imported file."
    )
    def get_imported_activity(
        import_id: str,
        include_records: bool = False,
        record_limit: int = 1000,
        include_points: bool = True,
        point_limit: int = 5000,
        include_health_records: bool = False,
        health_record_limit: int = 1000,
        csv_preview_limit: int = 100,
    ) -> dict[str, Any]:
        record = _load_record(import_id)
        path = _path_for_record(record)
        parsed: Any
        if record.kind == "fit":
            parsed = parse_fit_file(
                path, include_records=include_records, record_limit=record_limit
            )
        elif record.kind == "gpx":
            parsed = parse_gpx_file(path, include_points=include_points, point_limit=point_limit)
        elif record.kind == "json":
            parsed = json.loads(path.read_text(encoding="utf-8"))
        elif record.kind == "health":
            parsed = parse_apple_health_export(
                path,
                include_records=include_health_records,
                record_limit=health_record_limit,
            )
        elif record.kind == "csv":
            parsed = parse_delimited_file(path, preview_limit=csv_preview_limit)
        else:
            raise ValueError(f"Unsupported import kind: {record.kind}")
        return {"imported": record.safe_dict(), "parsed": parsed}
