use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, TimeDelta, TimeZone};
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::{Cloud, RESPONSE_LIMIT, args_query, bool_or, merge_query, raw_query, required_str};

const WINDOW_DAYS: i64 = 28;
const MAX_CHUNKS: usize = 512;
const DAILY_PATH: &str = "/247samples/daily-activity-statistics";
const SAMPLES: [(&str, &str, &str); 3] = [
    ("activity", "/247samples/activity", "include_activity"),
    ("sleep", "/247samples/sleep", "include_sleep"),
    ("recovery", "/247samples/recovery", "include_recovery"),
];

#[derive(Clone, Copy)]
enum Timestamp {
    Local(NaiveDateTime),
    Fixed(DateTime<FixedOffset>),
}

impl Timestamp {
    fn parse(value: &str) -> Result<Self> {
        if let Ok(date) = value.parse::<DateTime<FixedOffset>>() {
            return Ok(Self::Fixed(date));
        }
        for format in [
            "%Y-%m-%dT%H:%M:%S%.f",
            "%Y-%m-%d %H:%M:%S%.f",
            "%Y-%m-%dT%H:%M",
            "%Y-%m-%d %H:%M",
        ] {
            if let Ok(date) = NaiveDateTime::parse_from_str(value, format) {
                return Ok(Self::Local(date));
            }
        }
        let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .context("Expected an ISO 8601 date or timestamp")?;
        Ok(Self::Local(date.and_hms_opt(0, 0, 0).unwrap()))
    }

    fn milliseconds(self) -> Result<i64> {
        match self {
            Self::Fixed(value) => Ok(value.timestamp_millis()),
            Self::Local(value) => Local
                .from_local_datetime(&value)
                .earliest()
                .map(|timestamp| timestamp.timestamp_millis())
                .context("Local timestamp has no UTC equivalent. Supply a UTC offset."),
        }
    }

    fn advance(self) -> Result<Self> {
        let window = TimeDelta::days(WINDOW_DAYS);
        match self {
            Self::Fixed(value) => value.checked_add_signed(window).map(Self::Fixed),
            Self::Local(value) => value.checked_add_signed(window).map(Self::Local),
        }
        .context("Timestamp exceeds the supported range")
    }

    fn compare(self, other: Self) -> Result<std::cmp::Ordering> {
        match (self, other) {
            (Self::Fixed(left), Self::Fixed(right)) => Ok(left.cmp(&right)),
            (Self::Local(left), Self::Local(right)) => Ok(left.cmp(&right)),
            _ => bail!("from_value and to_value must both include or both omit a UTC offset."),
        }
    }
}

#[derive(Serialize)]
struct PlannedCall {
    endpoint: &'static str,
    path: &'static str,
    #[serde(flatten)]
    params: Map<String, Value>,
}

fn plan(args: &Value) -> Result<(usize, Vec<PlannedCall>)> {
    let from = required_str(args, "from_value")?;
    let to = required_str(args, "to_value")?;
    let start = Timestamp::parse(from)?;
    let end = Timestamp::parse(to)?;
    ensure!(
        !start.compare(end)?.is_gt(),
        "to_value must be after from_value."
    );

    let mut count = 0;
    let mut calls = Vec::new();
    let mut cursor = start;
    loop {
        count += 1;
        ensure!(
            count <= MAX_CHUNKS,
            "24/7 summary is limited to {MAX_CHUNKS} windows. Narrow the date range."
        );
        let next = cursor.advance()?;
        let last = !next.compare(end)?.is_lt();
        let chunk_end = if last { end } else { next };
        for (endpoint, path, flag) in SAMPLES {
            if bool_or(args, flag, true) {
                calls.push(PlannedCall {
                    endpoint,
                    path,
                    params: Map::from_iter([
                        ("from".into(), json!(cursor.milliseconds()?)),
                        ("to".into(), json!(chunk_end.milliseconds()?)),
                    ]),
                });
            }
        }
        if last {
            break;
        }
        cursor = chunk_end;
    }
    if bool_or(args, "include_daily_statistics", true) {
        calls.push(PlannedCall {
            endpoint: "daily_activity_statistics",
            path: DAILY_PATH,
            params: Map::from_iter([
                (
                    "startdate".into(),
                    json!(from.chars().take(10).collect::<String>()),
                ),
                (
                    "enddate".into(),
                    json!(to.chars().take(10).collect::<String>()),
                ),
            ]),
        });
    }
    Ok((count, calls))
}

pub(super) async fn call(cloud: &Cloud<'_>, name: &str, args: &Value) -> Result<Value> {
    match name {
        "suunto_get_activity_samples" | "suunto_get_sleep_data" | "suunto_get_recovery_data" => {
            let path = match name {
                "suunto_get_activity_samples" => SAMPLES[0].1,
                "suunto_get_sleep_data" => SAMPLES[1].1,
                _ => SAMPLES[2].1,
            };
            let start = Timestamp::parse(required_str(args, "from_value")?)?.milliseconds()?;
            let end = Timestamp::parse(required_str(args, "to_value")?)?.milliseconds()?;
            let query = Map::from_iter([("from".into(), json!(start)), ("to".into(), json!(end))]);
            cloud.get(path, merge_query(query, args)).await
        }
        "suunto_get_daily_activity_statistics" | "suunto_get_legacy_daily_activity_statistics" => {
            let path = if name == "suunto_get_daily_activity_statistics" {
                DAILY_PATH
            } else {
                "/247/daily-activity-statistics"
            };
            let query = args_query(args, &[("startdate", "startdate"), ("enddate", "enddate")]);
            cloud.get(path, merge_query(query, args)).await
        }
        "suunto_get_legacy_daily_activity" => cloud.get("/247", raw_query(args)).await,
        "suunto_estimate_247_summary_calls" | "suunto_get_247_summary" => {
            summary(cloud, args, name).await
        }
        _ => bail!("Unknown activity tool {name}"),
    }
}

async fn summary(cloud: &Cloud<'_>, args: &Value, name: &str) -> Result<Value> {
    let (chunk_count, calls) = plan(args)?;
    let quota = cloud.app.quota.status()?;
    let would_exceed_quota = quota["remaining"]
        .as_u64()
        .is_some_and(|remaining| calls.len() as u64 > remaining);
    if name == "suunto_estimate_247_summary_calls" {
        return Ok(json!({
            "call_count": calls.len(), "chunk_count": chunk_count, "max_window_days": WINDOW_DAYS,
            "calls": calls, "quota": quota, "would_exceed_quota": would_exceed_quota
        }));
    }
    ensure!(
        !would_exceed_quota,
        "Suunto 24/7 summary would require {} API calls, but only {} of {} weekly calls remain. Narrow the range, disable some include_* options, or wait for quota reset.",
        calls.len(),
        quota["remaining"],
        quota["limit"]
    );
    let mut results = BTreeMap::from([
        ("activity", Vec::new()),
        ("sleep", Vec::new()),
        ("recovery", Vec::new()),
        ("daily_activity_statistics", Vec::new()),
    ]);
    let mut total_size = 0usize;
    for call in &calls {
        let (payload, size) = cloud
            .get_json_with_size(call.path, call.params.clone())
            .await?;
        total_size += size;
        ensure!(
            total_size <= RESPONSE_LIMIT,
            "Suunto 24/7 summary exceeds {RESPONSE_LIMIT} bytes. Narrow the range or include fewer endpoints."
        );
        results
            .get_mut(call.endpoint)
            .expect("known endpoint")
            .push(payload);
    }
    let mut reply = json!({"call_count":calls.len(),"calls":calls,"quota":quota});
    reply["results"] = Value::Object(
        results
            .into_iter()
            .map(|(endpoint, values)| (endpoint.to_owned(), Value::Array(values)))
            .collect(),
    );
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_breaks_sample_calls_into_28_day_windows() -> Result<()> {
        let (chunks, calls) = plan(&json!({
            "from_value":"2026-01-01T00:00:00+00:00",
            "to_value":"2026-03-01T00:00:00+00:00",
        }))?;
        assert_eq!(chunks, 3);
        assert_eq!(calls.len(), 10);
        assert_eq!(calls[0].endpoint, "activity");
        assert_eq!(calls[0].params["from"], json!(1_767_225_600_000_i64));
        assert_eq!(calls[0].params["to"], calls[3].params["from"]);
        assert_eq!(calls[9].path, DAILY_PATH);
        assert_eq!(calls[9].params["enddate"], "2026-03-01");
        Ok(())
    }

    #[test]
    fn summary_accepts_equal_endpoints_and_filters_requested_samples() -> Result<()> {
        let (chunks, calls) = plan(&json!({
            "from_value":"2026-01-01", "to_value":"2026-01-01", "include_activity":false,
            "include_recovery":false, "include_daily_statistics":false,
        }))?;
        assert_eq!(chunks, 1);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].endpoint, "sleep");
        assert_eq!(calls[0].params["from"], calls[0].params["to"]);
        assert!(plan(&json!({"from_value":"2026-01-02","to_value":"2026-01-01"})).is_err());
        Ok(())
    }
}
