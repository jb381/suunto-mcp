from __future__ import annotations

import base64
import hashlib
import re
from pathlib import Path
from typing import Any, Literal

from suunto_mcp.config import Settings, settings

OutputMode = Literal["metadata", "base64", "file", "both"]


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def safe_filename(value: str, default: str = "suunto-export") -> str:
    cleaned = re.sub(r"[^A-Za-z0-9._-]+", "-", value).strip(".-")
    return cleaned or default


def resolve_export_dir(settings_obj: Settings = settings) -> Path | None:
    if not settings_obj.EXPORT_DIR:
        return None
    path = Path(settings_obj.EXPORT_DIR).expanduser()
    path.mkdir(parents=True, exist_ok=True)
    return path


def handle_binary_output(
    data: bytes,
    *,
    suggested_filename: str,
    content_type: str | None = None,
    output_mode: OutputMode = "metadata",
    settings_obj: Settings = settings,
) -> dict[str, Any]:
    digest = sha256_bytes(data)
    result: dict[str, Any] = {
        "filename": safe_filename(suggested_filename),
        "content_type": content_type,
        "size": len(data),
        "sha256": digest,
        "output_mode": output_mode,
    }

    if output_mode in {"file", "both"}:
        export_dir = resolve_export_dir(settings_obj)
        if export_dir is None:
            if output_mode == "file":
                raise RuntimeError(
                    "SUUNTO_EXPORT_DIR is required when output_mode='file' but is not configured."
                )
        else:
            path = export_dir / result["filename"]
            path.write_bytes(data)
            result["path"] = str(path)

    if output_mode in {"base64", "both"}:
        if len(data) > settings_obj.MAX_BASE64_BYTES:
            result["base64_omitted"] = True
            result["note"] = (
                f"Payload is {len(data)} bytes, above SUUNTO_MAX_BASE64_BYTES="
                f"{settings_obj.MAX_BASE64_BYTES}."
            )
        else:
            result["base64"] = base64.b64encode(data).decode("ascii")

    return result
