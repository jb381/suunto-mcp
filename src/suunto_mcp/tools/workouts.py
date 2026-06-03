# pyright: reportUnusedFunction=false
from __future__ import annotations

from typing import Any, Literal

from fastmcp import FastMCP

from suunto_mcp.binary import OutputMode, handle_binary_output
from suunto_mcp.client import SuuntoClient
from suunto_mcp.fit import parse_fit_bytes
from suunto_mcp.tools import merge_params, require_write_tools_enabled

KNOWN_WORKOUT_EXTENSIONS = [
    "AltitudeStreamExtension",
    "BatteryLevelStreamExtension",
    "CadenceStreamExtension",
    "DepthStreamExtension",
    "DistanceDeltaStreamExtension",
    "DistanceLapExtension",
    "DiveHeaderExtension",
    "DurationLapExtension",
    "EnergyConsumptionStreamExtension",
    "EpocStreamExtension",
    "FitnessExtension",
    "HeartRateExtension",
    "HeartrateStreamExtension",
    "IBIStreamExtension",
    "IntensityExtension",
    "IntervalStreamExtension",
    "LocationStreamExtension",
    "ManualLapStreamExtension",
    "MultisportMarker",
    "PauseMarkerExtension",
    "PoolLengthExtension",
    "PowerStreamExtension",
    "RelativePerformanceLevelStreamExtension",
    "SeaLevelPressureStreamExtension",
    "SkiExtension",
    "SpeedStreamExtension",
    "StepCountDeltaStreamExtension",
    "StrokeRateStreamExtension",
    "StrokesDeltaStreamExtension",
    "SummaryExtension",
    "SwimmingHeaderExtension",
    "SwolfStreamExtension",
    "TemperatureStreamExtension",
    "VerticalLapExtension",
    "VerticalSpeedStreamExtension",
    "WeatherExtension",
    "ZappChannelsExtension",
]


def _extensions(value: str | list[str] | None) -> str | None:
    if value is None:
        return None
    if value == "all":
        return ",".join(KNOWN_WORKOUT_EXTENSIONS)
    if isinstance(value, list):
        if len(value) == 1 and value[0] == "all":
            return ",".join(KNOWN_WORKOUT_EXTENSIONS)
        return ",".join(value)
    return value


def register(mcp: FastMCP) -> None:
    @mcp.tool(
        name="suunto_list_workouts",
        description=(
            "List workouts for an authorized Suunto account using the current /v3/workouts API."
        ),
    )
    async def list_workouts(
        account_id: str | None = None,
        limit: int | None = None,
        offset: int | None = None,
        from_date: str | None = None,
        to_date: str | None = None,
        modified_since: str | None = None,
        extensions: Literal["all"] | list[str] | str | None = None,
        query_params: dict[str, Any] | None = None,
    ) -> Any:
        params = merge_params(
            {
                "limit": limit,
                "offset": offset,
                "from": from_date,
                "to": to_date,
                "modifiedSince": modified_since,
                "extensions": _extensions(extensions),
            },
            query_params,
        )
        async with SuuntoClient(account_id=account_id) as client:
            return await client.get_json("/v3/workouts", params=params)

    @mcp.tool(name="suunto_get_workout", description="Fetch one Suunto workout JSON document.")
    async def get_workout(
        workout_id_or_key: str,
        account_id: str | None = None,
        extensions: Literal["all"] | list[str] | str | None = None,
        query_params: dict[str, Any] | None = None,
    ) -> Any:
        params = merge_params({"extensions": _extensions(extensions)}, query_params)
        async with SuuntoClient(account_id=account_id) as client:
            return await client.get_json(f"/v3/workouts/{workout_id_or_key}", params=params)

    @mcp.tool(
        name="suunto_get_workout_fit",
        description="Download a workout FIT file as metadata, base64, file, or both.",
    )
    async def get_workout_fit(
        workout_id_or_key: str,
        account_id: str | None = None,
        output_mode: OutputMode = "metadata",
    ) -> dict[str, Any]:
        async with SuuntoClient(account_id=account_id) as client:
            data, content_type = await client.get_bytes(
                f"/v3/workouts/{workout_id_or_key}/fit",
                accept="application/octet-stream",
            )
        return handle_binary_output(
            data,
            suggested_filename=f"suunto-workout-{workout_id_or_key}.fit",
            content_type=content_type or "application/octet-stream",
            output_mode=output_mode,
        )

    @mcp.tool(
        name="suunto_parse_workout_fit",
        description=(
            "Download a workout FIT file and parse sessions, laps, events, and optional records."
        ),
    )
    async def parse_workout_fit(
        workout_id_or_key: str,
        account_id: str | None = None,
        include_records: bool = False,
        record_limit: int = 1000,
        include_messages: bool = False,
        message_limit: int = 2000,
    ) -> dict[str, Any]:
        async with SuuntoClient(account_id=account_id) as client:
            data, _content_type = await client.get_bytes(f"/v3/workouts/{workout_id_or_key}/fit")
        result = parse_fit_bytes(
            data,
            include_records=include_records,
            record_limit=record_limit,
            include_messages=include_messages,
            message_limit=message_limit,
        )
        result["workout_id_or_key"] = workout_id_or_key
        return result

    @mcp.tool(
        name="suunto_get_complete_workout",
        description="Fetch workout JSON plus optional FIT metadata and parsed FIT summary.",
    )
    async def get_complete_workout(
        workout_id_or_key: str,
        account_id: str | None = None,
        extensions: Literal["all"] | list[str] | str | None = "all",
        include_fit_metadata: bool = True,
        parse_fit: bool = False,
        include_records: bool = False,
        record_limit: int = 1000,
    ) -> dict[str, Any]:
        async with SuuntoClient(account_id=account_id) as client:
            workout = await client.get_json(
                f"/v3/workouts/{workout_id_or_key}",
                params={"extensions": _extensions(extensions)},
            )
            result: dict[str, Any] = {"workout": workout}
            if include_fit_metadata or parse_fit:
                data, content_type = await client.get_bytes(f"/v3/workouts/{workout_id_or_key}/fit")
                if include_fit_metadata:
                    result["fit"] = handle_binary_output(
                        data,
                        suggested_filename=f"suunto-workout-{workout_id_or_key}.fit",
                        content_type=content_type or "application/octet-stream",
                        output_mode="metadata",
                    )
                if parse_fit:
                    result["fit_parse"] = parse_fit_bytes(
                        data,
                        include_records=include_records,
                        record_limit=record_limit,
                    )
            return result

    @mcp.tool(
        name="suunto_add_workout_description_info",
        description=(
            "Add information to a workout description. Requires SUUNTO_ENABLE_WRITE_TOOLS=true."
        ),
    )
    async def add_workout_description_info(
        workout_id: str,
        body: dict[str, Any],
        account_id: str | None = None,
    ) -> Any:
        require_write_tools_enabled()
        async with SuuntoClient(account_id=account_id) as client:
            return await client.post_json(f"/v1/workouts/addinfo/{workout_id}", body=body)

    @mcp.tool(
        name="suunto_list_workouts_deprecated_v2",
        description=(
            "List workouts using Suunto's deprecated /v2/workouts API. Prefer suunto_list_workouts."
        ),
    )
    async def list_workouts_deprecated_v2(
        account_id: str | None = None,
        query_params: dict[str, Any] | None = None,
    ) -> Any:
        async with SuuntoClient(account_id=account_id) as client:
            return await client.get_json("/v2/workouts", params=query_params)

    @mcp.tool(
        name="suunto_get_workout_deprecated_v2",
        description=(
            "Fetch one workout using Suunto's deprecated /v2/workouts API. "
            "Prefer suunto_get_workout."
        ),
    )
    async def get_workout_deprecated_v2(
        workout_key: str,
        account_id: str | None = None,
        query_params: dict[str, Any] | None = None,
    ) -> Any:
        async with SuuntoClient(account_id=account_id) as client:
            return await client.get_json(f"/v2/workouts/{workout_key}", params=query_params)

    @mcp.tool(
        name="suunto_get_workout_fit_deprecated_v2",
        description=(
            "Download a FIT file using Suunto's deprecated /v2/workouts API. "
            "Prefer suunto_get_workout_fit."
        ),
    )
    async def get_workout_fit_deprecated_v2(
        workout_key: str,
        account_id: str | None = None,
        output_mode: OutputMode = "metadata",
    ) -> dict[str, Any]:
        async with SuuntoClient(account_id=account_id) as client:
            data, content_type = await client.get_bytes(
                f"/v2/workouts/{workout_key}/fit",
                accept="application/octet-stream",
            )
        return handle_binary_output(
            data,
            suggested_filename=f"suunto-workout-v2-{workout_key}.fit",
            content_type=content_type or "application/octet-stream",
            output_mode=output_mode,
        )
