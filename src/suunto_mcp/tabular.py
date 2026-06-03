from __future__ import annotations

import csv
from pathlib import Path
from typing import Any


def _detect_delimiter(sample: str, delimiter: str | None) -> str:
    if delimiter:
        return delimiter
    try:
        return csv.Sniffer().sniff(sample, delimiters=",;\t|").delimiter
    except csv.Error:
        return ","


def _headers(first_row: list[str], *, has_header: bool) -> list[str]:
    if has_header:
        return first_row
    return [f"column_{index}" for index in range(1, len(first_row) + 1)]


def parse_delimited_file(
    path: str | Path,
    *,
    delimiter: str | None = None,
    has_header: bool = True,
    preview_limit: int = 100,
    encoding: str = "utf-8-sig",
) -> dict[str, Any]:
    source = Path(path).expanduser()
    with source.open("r", encoding=encoding, newline="") as handle:
        sample = handle.read(8192)
        handle.seek(0)
        detected_delimiter = _detect_delimiter(sample, delimiter)
        reader = csv.reader(handle, delimiter=detected_delimiter)

        try:
            first_row = next(reader)
        except StopIteration:
            return {
                "source_path": str(source),
                "delimiter": detected_delimiter,
                "has_header": has_header,
                "fieldnames": [],
                "row_count": 0,
                "rows_included": 0,
                "preview_limit": preview_limit,
                "rows": [],
            }

        fieldnames = _headers(first_row, has_header=has_header)
        preview: list[dict[str, str]] = []
        row_count = 0

        def append_row(values: list[str]) -> None:
            nonlocal row_count
            row_count += 1
            if len(preview) < preview_limit:
                row = {
                    fieldname: values[index] if index < len(values) else ""
                    for index, fieldname in enumerate(fieldnames)
                }
                if len(values) > len(fieldnames):
                    row["_extra"] = ",".join(values[len(fieldnames) :])
                preview.append(row)

        if not has_header:
            append_row(first_row)
        for row_values in reader:
            append_row(row_values)

    return {
        "source_path": str(source),
        "delimiter": detected_delimiter,
        "has_header": has_header,
        "fieldnames": fieldnames,
        "row_count": row_count,
        "rows_included": len(preview),
        "preview_limit": preview_limit,
        "rows": preview,
    }
