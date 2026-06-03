from __future__ import annotations

from pathlib import Path
from typing import Any

import gpxpy


def _point(point: Any) -> dict[str, Any]:
    return {
        "latitude": point.latitude,
        "longitude": point.longitude,
        "elevation": point.elevation,
        "time": point.time.isoformat() if point.time else None,
        "name": getattr(point, "name", None),
        "description": getattr(point, "description", None),
        "type": getattr(point, "type", None),
    }


def parse_gpx_text(
    text: str, *, include_points: bool = True, point_limit: int = 5000
) -> dict[str, Any]:
    gpx = gpxpy.parse(text)
    points_returned = 0
    tracks: list[dict[str, Any]] = []
    routes: list[dict[str, Any]] = []

    for track in gpx.tracks:
        track_data: dict[str, Any] = {
            "name": track.name,
            "description": track.description,
            "segments": [],
        }
        for segment in track.segments:
            segment_points = [_point(point) for point in segment.points]
            segment_data: dict[str, Any] = {
                "point_count": len(segment.points),
                "length_2d_m": segment.length_2d(),
                "length_3d_m": segment.length_3d(),
            }
            if include_points:
                remaining = max(point_limit - points_returned, 0)
                segment_data["points"] = segment_points[:remaining]
                points_returned += len(segment_data["points"])
            track_data["segments"].append(segment_data)
        tracks.append(track_data)

    for route in gpx.routes:
        route_points = [_point(point) for point in route.points]
        route_data: dict[str, Any] = {
            "name": route.name,
            "description": route.description,
            "point_count": len(route.points),
        }
        if include_points:
            remaining = max(point_limit - points_returned, 0)
            route_data["points"] = route_points[:remaining]
            points_returned += len(route_data["points"])
        routes.append(route_data)

    waypoints = [_point(point) for point in gpx.waypoints]
    if include_points:
        remaining = max(point_limit - points_returned, 0)
        waypoint_payload = waypoints[:remaining]
        points_returned += len(waypoint_payload)
    else:
        waypoint_payload = []

    return {
        "metadata": {
            "name": gpx.name,
            "description": gpx.description,
            "author_name": gpx.author_name,
            "time": gpx.time.isoformat() if gpx.time else None,
        },
        "track_count": len(gpx.tracks),
        "route_count": len(gpx.routes),
        "waypoint_count": len(gpx.waypoints),
        "tracks": tracks,
        "routes": routes,
        "waypoints": waypoint_payload if include_points else None,
        "points_included": include_points,
        "point_limit": point_limit,
        "points_returned": points_returned,
    }


def parse_gpx_file(
    path: str | Path,
    *,
    include_points: bool = True,
    point_limit: int = 5000,
) -> dict[str, Any]:
    file_path = Path(path).expanduser()
    result = parse_gpx_text(
        file_path.read_text(encoding="utf-8"),
        include_points=include_points,
        point_limit=point_limit,
    )
    result["path"] = str(file_path)
    return result
