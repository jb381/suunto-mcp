# pyright: reportMissingTypeStubs=false, reportUnknownMemberType=false, reportUnknownVariableType=false, reportUnknownArgumentType=false, reportAttributeAccessIssue=false
from __future__ import annotations

from collections import Counter
from datetime import date, datetime
from io import BytesIO
from pathlib import Path
from typing import Any

from fitparse import FitFile


def _jsonable(value: Any) -> Any:
    if isinstance(value, datetime | date):
        return value.isoformat()
    if isinstance(value, bytes):
        return value.hex()
    if isinstance(value, list | tuple):
        return [_jsonable(item) for item in value]
    if isinstance(value, dict):
        return {str(key): _jsonable(val) for key, val in value.items()}
    if isinstance(value, int | float | str | bool) or value is None:
        return value
    return str(value)


def _field_to_dict(field: Any) -> dict[str, Any]:
    return {
        "name": getattr(field, "name", None),
        "value": _jsonable(getattr(field, "value", None)),
        "units": getattr(field, "units", None),
        "raw_value": _jsonable(getattr(field, "raw_value", None)),
    }


def _message_to_dict(message: Any) -> dict[str, Any]:
    fields = [_field_to_dict(field) for field in message.fields]
    return {
        "name": message.name,
        "fields": {field["name"]: field["value"] for field in fields if field["name"]},
        "field_details": fields,
    }


def parse_fit_bytes(
    data: bytes,
    *,
    include_records: bool = False,
    record_limit: int = 1000,
    include_messages: bool = False,
    message_limit: int = 2000,
) -> dict[str, Any]:
    fit = FitFile(BytesIO(data), check_crc=False)
    message_counts: Counter[str] = Counter()
    records: list[dict[str, Any]] = []
    messages: list[dict[str, Any]] = []
    sessions: list[dict[str, Any]] = []
    laps: list[dict[str, Any]] = []
    events: list[dict[str, Any]] = []
    file_ids: list[dict[str, Any]] = []

    for message in fit.get_messages():
        message_counts[message.name] += 1
        message_dict = _message_to_dict(message)
        if message.name == "record" and include_records and len(records) < record_limit:
            records.append(message_dict)
        elif message.name == "session":
            sessions.append(message_dict)
        elif message.name == "lap":
            laps.append(message_dict)
        elif message.name == "event":
            events.append(message_dict)
        elif message.name == "file_id":
            file_ids.append(message_dict)

        if include_messages and len(messages) < message_limit:
            messages.append(message_dict)

    result: dict[str, Any] = {
        "size": len(data),
        "message_counts": dict(sorted(message_counts.items())),
        "file_ids": file_ids,
        "sessions": sessions,
        "laps": laps,
        "events": events,
        "records_included": include_records,
        "record_limit": record_limit,
        "record_count_returned": len(records),
        "total_record_messages": message_counts.get("record", 0),
    }
    if include_records:
        result["records"] = records
    if include_messages:
        result["messages"] = messages
        result["message_limit"] = message_limit
        result["message_count_returned"] = len(messages)
    return result


def parse_fit_file(
    path: str | Path,
    *,
    include_records: bool = False,
    record_limit: int = 1000,
    include_messages: bool = False,
    message_limit: int = 2000,
) -> dict[str, Any]:
    file_path = Path(path).expanduser()
    data = file_path.read_bytes()
    result = parse_fit_bytes(
        data,
        include_records=include_records,
        record_limit=record_limit,
        include_messages=include_messages,
        message_limit=message_limit,
    )
    result["path"] = str(file_path)
    return result
