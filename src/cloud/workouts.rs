use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

use super::{Cloud, args_query, bool_or, id_path, merge_query, raw_query, required_str};
use crate::local;

const FIT_MIME: &str = "application/octet-stream";
const KNOWN_WORKOUT_EXTENSIONS: &[&str] = &[
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
];

fn extensions(args: &Value, default_to_all: bool) -> Map<String, Value> {
    let format_extensions = |value: &str| {
        if value == "all" {
            KNOWN_WORKOUT_EXTENSIONS.join(",")
        } else {
            value.to_owned()
        }
    };
    let value = match args.get("extensions") {
        Some(Value::String(value)) => Some(format_extensions(value)),
        Some(Value::Array(values)) => {
            let values: Vec<_> = values.iter().filter_map(Value::as_str).collect();
            Some(if values == ["all"] {
                format_extensions("all")
            } else {
                values.join(",")
            })
        }
        None if default_to_all => Some(format_extensions("all")),
        _ => None,
    };
    value
        .map(|value| ("extensions".to_owned(), json!(value)))
        .into_iter()
        .collect()
}

pub(super) async fn call(cloud: &Cloud<'_>, name: &str, args: &Value) -> Result<Value> {
    match name {
        "suunto_list_workouts" => {
            let mut query = extensions(args, false);
            query.extend(args_query(
                args,
                &[
                    ("limit", "limit"),
                    ("offset", "offset"),
                    ("from", "from_date"),
                    ("to", "to_date"),
                    ("modifiedSince", "modified_since"),
                ],
            ));
            cloud.get("/v3/workouts", merge_query(query, args)).await
        }
        "suunto_get_workout" => {
            let id = required_str(args, "workout_id_or_key")?;
            cloud
                .get(
                    &id_path("/v3/workouts", id, "")?,
                    merge_query(extensions(args, false), args),
                )
                .await
        }
        "suunto_get_workout_fit" | "suunto_get_workout_fit_deprecated_v2" => {
            let (prefix, key, label) = if name == "suunto_get_workout_fit" {
                ("/v3/workouts", "workout_id_or_key", "suunto-workout")
            } else {
                ("/v2/workouts", "workout_key", "suunto-workout-v2")
            };
            let id = required_str(args, key)?;
            cloud
                .export(
                    &id_path(prefix, id, "/fit")?,
                    &format!("{label}-{id}.fit"),
                    FIT_MIME,
                    Some(FIT_MIME),
                    args,
                )
                .await
        }
        "suunto_parse_workout_fit" => {
            let id = required_str(args, "workout_id_or_key")?;
            let (_, data) = cloud
                .get_bytes(&id_path("/v3/workouts", id, "/fit")?, Map::new(), None)
                .await?;
            let args = args.clone();
            let mut result =
                tokio::task::spawn_blocking(move || local::parse_fit(&data, &args)).await??;
            result["workout_id_or_key"] = json!(id);
            Ok(result)
        }
        "suunto_get_complete_workout" => complete(cloud, args).await,
        "suunto_add_workout_description_info" => {
            cloud.require_write()?;
            let id = required_str(args, "workout_id")?;
            cloud
                .post_json(&id_path("/v1/workouts/addinfo", id, "")?, &args["body"])
                .await
        }
        "suunto_list_workouts_deprecated_v2" => cloud.get("/v2/workouts", raw_query(args)).await,
        "suunto_get_workout_deprecated_v2" => {
            let id = required_str(args, "workout_key")?;
            cloud
                .get(&id_path("/v2/workouts", id, "")?, raw_query(args))
                .await
        }
        _ => bail!("Unknown workout tool {name}"),
    }
}

async fn complete(cloud: &Cloud<'_>, args: &Value) -> Result<Value> {
    let id = required_str(args, "workout_id_or_key")?;
    let path = id_path("/v3/workouts", id, "")?;
    let mut query = extensions(args, true);
    if args.get("extensions").is_some_and(Value::is_null) {
        query.insert("extensions".into(), Value::Null);
    }
    let workout = cloud.get(&path, query).await?;
    let mut result = Value::Object(Map::from_iter([("workout".into(), workout)]));
    let include_fit_metadata = bool_or(args, "include_fit_metadata", true);
    let parse_fit = bool_or(args, "parse_fit", false);
    if include_fit_metadata || parse_fit {
        let (mime, data) = cloud
            .get_bytes(&format!("{path}/fit"), Map::new(), None)
            .await?;
        if include_fit_metadata {
            result["fit"] = local::binary_output(
                cloud.app,
                &data,
                &format!("suunto-workout-{id}.fit"),
                mime.as_deref().or(Some(FIT_MIME)),
                "metadata",
            )?;
        }
        if parse_fit {
            let options = json!({
                "include_records": args["include_records"],
                "record_limit": args["record_limit"]
            });
            result["fit_parse"] =
                tokio::task::spawn_blocking(move || local::parse_fit(&data, &options)).await??;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_modes_match_workout_query_contract() {
        let all = KNOWN_WORKOUT_EXTENSIONS.join(",");
        assert_eq!(extensions(&json!({}), true)["extensions"], all);
        assert_eq!(
            extensions(&json!({"extensions":["all"]}), false)["extensions"],
            all
        );
        assert!(extensions(&json!({"extensions":null}), true).is_empty());
        assert_eq!(
            extensions(&json!({"extensions":["altitude","distance"]}), false)["extensions"],
            "altitude,distance"
        );
    }
}
