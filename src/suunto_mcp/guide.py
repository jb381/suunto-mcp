from __future__ import annotations

import json
import zipfile
from pathlib import Path
from typing import Any, cast


def validate_guide_json(data: dict[str, Any]) -> dict[str, Any]:
    errors: list[str] = []
    if data.get("usage") != "workout":
        errors.append("usage must be 'workout'.")
    if not isinstance(data.get("name"), str) or not data.get("name"):
        errors.append("name is required.")
    if not isinstance(data.get("owner"), str) or not data.get("owner"):
        errors.append("owner is required and should match the Suunto OAuth app name.")
    steps_obj = data.get("steps")
    if not isinstance(steps_obj, list) or not steps_obj:
        errors.append("steps must be a non-empty list.")
    elif len(cast(list[Any], steps_obj)) > 1000:
        errors.append("steps must contain at most 1000 items.")
    external_id = data.get("externalId")
    if external_id is not None and (not isinstance(external_id, str) or len(external_id) > 64):
        errors.append("externalId must be a string with at most 64 characters.")
    return {"valid": not errors, "errors": errors}


def create_guide_zip(
    guide_json_path: str | Path,
    icon_png_path: str | Path,
    output_path: str | Path,
) -> dict[str, Any]:
    guide_path = Path(guide_json_path).expanduser()
    icon_path = Path(icon_png_path).expanduser()
    out_path = Path(output_path).expanduser()
    data = json.loads(guide_path.read_text(encoding="utf-8"))
    validation = validate_guide_json(data)
    if not validation["valid"]:
        return {"created": False, "validation": validation}
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(out_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.write(guide_path, "guide.json")
        archive.write(icon_path, "icon.png")
    return {
        "created": True,
        "path": str(out_path),
        "size": out_path.stat().st_size,
        "validation": validation,
    }
