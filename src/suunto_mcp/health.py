from __future__ import annotations

from collections import Counter
from pathlib import Path
from typing import Any

from defusedxml import ElementTree


def _attributes(element: ElementTree.Element[str]) -> dict[str, str]:
    return {key: value for key, value in element.attrib.items()}


def parse_apple_health_export(
    path: str | Path,
    *,
    include_records: bool = False,
    record_limit: int = 1000,
    include_workouts: bool = True,
    workout_limit: int = 500,
    include_activity_summaries: bool = True,
    activity_summary_limit: int = 500,
) -> dict[str, Any]:
    """Parse an Apple Health export.xml file with bounded retained samples."""
    source = Path(path).expanduser()
    element_counts: Counter[str] = Counter()
    record_counts: Counter[str] = Counter()
    workout_counts: Counter[str] = Counter()
    records: list[dict[str, Any]] = []
    workouts: list[dict[str, Any]] = []
    activity_summaries: list[dict[str, str]] = []

    current_workout: dict[str, Any] | None = None
    current_workout_metadata: list[dict[str, str]] = []
    current_workout_events: list[dict[str, str]] = []

    for event, element in ElementTree.iterparse(source, events=("start", "end")):
        tag = element.tag
        if event == "start" and tag == "Workout":
            current_workout = _attributes(element)
            current_workout_metadata = []
            current_workout_events = []
            continue

        if event != "end":
            continue

        element_counts[tag] += 1
        if tag == "Record":
            attrs = _attributes(element)
            record_type = attrs.get("type", "unknown")
            record_counts[record_type] += 1
            if include_records and len(records) < record_limit:
                records.append(attrs)
        elif tag == "ActivitySummary":
            if include_activity_summaries and len(activity_summaries) < activity_summary_limit:
                activity_summaries.append(_attributes(element))
        elif tag == "MetadataEntry" and current_workout is not None:
            current_workout_metadata.append(_attributes(element))
        elif tag == "WorkoutEvent" and current_workout is not None:
            current_workout_events.append(_attributes(element))
        elif tag == "Workout":
            attrs = current_workout or _attributes(element)
            workout_type = attrs.get("workoutActivityType", "unknown")
            workout_counts[workout_type] += 1
            if include_workouts and len(workouts) < workout_limit:
                item: dict[str, Any] = dict(attrs)
                if current_workout_metadata:
                    item["metadata"] = current_workout_metadata
                if current_workout_events:
                    item["events"] = current_workout_events
                workouts.append(item)
            current_workout = None
            current_workout_metadata = []
            current_workout_events = []

        element.clear()

    return {
        "source_path": str(source),
        "element_counts": dict(element_counts),
        "record_counts": dict(record_counts),
        "workout_counts": dict(workout_counts),
        "records_included": len(records),
        "record_limit": record_limit,
        "workouts_included": len(workouts),
        "workout_limit": workout_limit,
        "activity_summaries_included": len(activity_summaries),
        "activity_summary_limit": activity_summary_limit,
        "records": records,
        "workouts": workouts,
        "activity_summaries": activity_summaries,
    }
