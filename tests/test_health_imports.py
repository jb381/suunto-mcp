from __future__ import annotations

from pathlib import Path
from typing import Any

from fastmcp import Client

from suunto_mcp.health import parse_apple_health_export
from suunto_mcp.server import mcp


def _write_health_export(path: Path) -> None:
    path.write_text(
        """<?xml version="1.0" encoding="UTF-8"?>
<HealthData locale="en_US">
  <Record type="HKQuantityTypeIdentifierStepCount" sourceName="Suunto"
      startDate="2026-06-01 08:00:00 +0200" endDate="2026-06-01 08:10:00 +0200"
      unit="count" value="500"/>
  <Record type="HKQuantityTypeIdentifierHeartRate" sourceName="Suunto"
      startDate="2026-06-01 08:00:00 +0200" endDate="2026-06-01 08:00:00 +0200"
      unit="count/min" value="130"/>
  <Workout workoutActivityType="HKWorkoutActivityTypeRunning" duration="1800"
      durationUnit="sec" sourceName="Suunto"
      startDate="2026-06-01 08:00:00 +0200" endDate="2026-06-01 08:30:00 +0200">
    <MetadataEntry key="HKIndoorWorkout" value="0"/>
    <WorkoutEvent type="HKWorkoutEventTypePause" date="2026-06-01 08:15:00 +0200"/>
  </Workout>
  <ActivitySummary dateComponents="2026-06-01" activeEnergyBurned="100"
      appleExerciseTime="30" appleStandHours="8"/>
</HealthData>
""",
        encoding="utf-8",
    )


def _structured(result: Any) -> Any:
    data = getattr(result, "structured_content", None)
    if data is not None:
        return data
    data = getattr(result, "data", None)
    if data is not None:
        return data
    return result


def test_parse_apple_health_export_counts_records_and_workouts(tmp_path: Path) -> None:
    source = tmp_path / "export.xml"
    _write_health_export(source)

    parsed = parse_apple_health_export(source, include_records=True)

    assert parsed["element_counts"]["Record"] == 2
    assert parsed["record_counts"]["HKQuantityTypeIdentifierStepCount"] == 1
    assert parsed["record_counts"]["HKQuantityTypeIdentifierHeartRate"] == 1
    assert parsed["workout_counts"]["HKWorkoutActivityTypeRunning"] == 1
    assert parsed["activity_summaries_included"] == 1
    assert parsed["records_included"] == 2
    assert parsed["workouts"][0]["metadata"][0]["key"] == "HKIndoorWorkout"


async def test_health_export_tools_parse_and_import(tmp_path: Path, monkeypatch) -> None:
    source = tmp_path / "export.xml"
    _write_health_export(source)
    monkeypatch.setattr("suunto_mcp.tools.imports.settings.LOCAL_DATA_DIR", str(tmp_path / "data"))

    async with Client(mcp) as client:
        parsed = _structured(
            await client.call_tool(
                "suunto_parse_health_export",
                {"path": str(source), "include_records": True},
            )
        )
        imported = _structured(
            await client.call_tool(
                "suunto_import_health_export",
                {"path": str(source), "copy_file": False, "include_records": True},
            )
        )
        listed = _structured(
            await client.call_tool(
                "suunto_list_imported_files",
                {"kind": "health"},
            )
        )

    assert parsed["record_counts"]["HKQuantityTypeIdentifierStepCount"] == 1
    assert imported["imported"]["kind"] == "health"
    assert imported["parsed"]["records_included"] == 2
    assert len(listed["imports"]) == 1
