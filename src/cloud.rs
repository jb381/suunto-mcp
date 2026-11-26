use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use reqwest::{
    Client, Method, RequestBuilder, Response, StatusCode, Url,
    header::{ACCEPT, CONTENT_TYPE},
};
use serde_json::{Map, Value, json};
use tokio::time::{Instant, sleep_until};

use crate::{App, local};

mod activity;
mod uploads;
mod workouts;

const RESPONSE_LIMIT: usize = 16 * 1024 * 1024;
const JSON_CONTENT_TYPE: &str = "application/json";
const GPX_CONTENT_TYPE: &str = "application/gpx+xml";
const ZIP_CONTENT_TYPE: &str = "application/zip";

struct Cloud<'a> {
    app: &'a App,
    account: Option<&'a str>,
}

impl Cloud<'_> {
    async fn send(&self, method: Method, path: &str, request: RequestBuilder) -> Result<Response> {
        let app = self.app;
        ensure!(
            !app.config.subscription_key.is_empty(),
            "SUUNTO_SUBSCRIPTION_KEY is required for Suunto Cloud API calls."
        );
        let record = app.record(self.account, None).await?;
        let retry = request
            .try_clone()
            .context("Cannot retry this API request")?;
        let response = self
            .send_once(&method, path, request, &record.access_token)
            .await?;
        let response = if response.status() == StatusCode::UNAUTHORIZED
            && record
                .refresh_token
                .as_deref()
                .is_some_and(|token| !token.is_empty())
        {
            drop(response);
            let refreshed = app
                .record(Some(&record.account_id), Some(&record.access_token))
                .await?;
            self.send_once(&method, path, retry, &refreshed.access_token)
                .await?
        } else {
            response
        };
        let status = response.status();
        ensure!(
            !status.is_client_error() && !status.is_server_error(),
            "Suunto API returned HTTP {}.",
            status.as_u16()
        );
        Ok(response)
    }

    async fn send_once(
        &self,
        method: &Method,
        path: &str,
        request: RequestBuilder,
        token: &str,
    ) -> Result<Response> {
        let app = self.app;
        if app.config.rate > 0.0 {
            let interval = Duration::try_from_secs_f64(60.0 / app.config.rate)
                .context("Invalid SUUNTO_API_CALLS_PER_MINUTE")?;
            let mut last = app.api_rate.lock().await;
            if let Some(previous) = *last {
                let next = previous
                    .checked_add(interval)
                    .context("Invalid SUUNTO_API_CALLS_PER_MINUTE")?;
                sleep_until(next).await;
            }
            *last = Some(Instant::now());
        }
        let account = self.account.filter(|id| !id.is_empty()).or_else(|| {
            (!app.config.default_account.is_empty()).then_some(app.config.default_account.as_str())
        });
        app.quota.record(method.as_str(), path, account)?;
        request
            .bearer_auth(token)
            .header("Ocp-Apim-Subscription-Key", &app.config.subscription_key)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Suunto API request failed")
    }

    fn request(
        &self,
        method: Method,
        path: &str,
        query: &Map<String, Value>,
    ) -> Result<RequestBuilder> {
        let url = self.app.config.api_endpoint(path)?;
        Ok(self
            .app
            .http
            .request(method, url)
            .query(&query_pairs(query)))
    }

    async fn get(&self, path: &str, query: Map<String, Value>) -> Result<Value> {
        Ok(self.get_json_with_size(path, query).await?.0)
    }

    async fn get_json_with_size(
        &self,
        path: &str,
        query: Map<String, Value>,
    ) -> Result<(Value, usize)> {
        let (_, body) = self.get_bytes(path, query, Some(JSON_CONTENT_TYPE)).await?;
        Ok((serde_json::from_slice(&body)?, body.len()))
    }

    async fn get_bytes(
        &self,
        path: &str,
        query: Map<String, Value>,
        accept: Option<&str>,
    ) -> Result<(Option<String>, Vec<u8>)> {
        let mut request = self.request(Method::GET, path, &query)?;
        if let Some(accept) = accept {
            request = request.header(ACCEPT, accept);
        }
        let response = self.send(Method::GET, path, request).await?;
        read_response(response).await
    }

    async fn post_json(&self, path: &str, body: &Value) -> Result<Value> {
        let request = self
            .request(Method::POST, path, &Map::new())?
            .header(ACCEPT, JSON_CONTENT_TYPE)
            .json(body);
        let (_, body) = read_response(self.send(Method::POST, path, request).await?).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    async fn write_bytes(
        &self,
        method: Method,
        path: &str,
        query: &Map<String, Value>,
        content_type: &str,
        body: Vec<u8>,
    ) -> Result<(Option<String>, Vec<u8>)> {
        let request = self
            .request(method.clone(), path, query)?
            .header(ACCEPT, JSON_CONTENT_TYPE)
            .header(CONTENT_TYPE, content_type)
            .body(body);
        let response = self.send(method, path, request).await?;
        read_response(response).await
    }

    async fn delete(&self, path: &str) -> Result<Value> {
        let request = self
            .request(Method::DELETE, path, &Map::new())?
            .header(ACCEPT, JSON_CONTENT_TYPE);
        let (_, body) = read_response(self.send(Method::DELETE, path, request).await?).await?;
        if body.is_empty() {
            return Ok(json!({"ok": true}));
        }
        Ok(serde_json::from_slice(&body)?)
    }

    fn require_write(&self) -> Result<()> {
        ensure!(
            self.app.config.write_tools,
            "This is a write/push operation. Set SUUNTO_ENABLE_WRITE_TOOLS=true to enable it."
        );
        Ok(())
    }

    async fn export(
        &self,
        path: &str,
        filename: &str,
        fallback_mime: &str,
        accept: Option<&str>,
        args: &Value,
    ) -> Result<Value> {
        let (mime, data) = self.get_bytes(path, Map::new(), accept).await?;
        let mode = string_or(args, "output_mode", "metadata");
        local::binary_output(
            self.app,
            &data,
            filename,
            mime.as_deref().or(Some(fallback_mime)),
            mode,
        )
    }
}

async fn read_response(mut response: Response) -> Result<(Option<String>, Vec<u8>)> {
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= RESPONSE_LIMIT as u64),
        "Suunto API response exceeds {RESPONSE_LIMIT} bytes"
    );
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(reqwest::Error::without_url)?
    {
        ensure!(
            body.len().saturating_add(chunk.len()) <= RESPONSE_LIMIT,
            "Suunto API response exceeds {RESPONSE_LIMIT} bytes"
        );
        body.extend_from_slice(&chunk);
    }
    Ok((content_type, body))
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .ok_or_else(|| anyhow!("Expected a string for {key}"))
}

fn string_or<'a>(args: &'a Value, key: &str, default: &'a str) -> &'a str {
    args[key].as_str().unwrap_or(default)
}

fn bool_or(args: &Value, key: &str, default: bool) -> bool {
    args[key].as_bool().unwrap_or(default)
}

fn args_query(args: &Value, keys: &[(&str, &str)]) -> Map<String, Value> {
    keys.iter()
        .filter_map(|(query_key, arg_key)| {
            args.get(arg_key)
                .filter(|value| !value.is_null())
                .map(|value| ((*query_key).to_owned(), value.clone()))
        })
        .collect()
}

fn raw_query(args: &Value) -> Map<String, Value> {
    args["query_params"]
        .as_object()
        .cloned()
        .unwrap_or_default()
}

fn merge_query(mut query: Map<String, Value>, args: &Value) -> Map<String, Value> {
    if let Some(extra) = args["query_params"].as_object() {
        query.extend(
            extra
                .iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    query
}

fn query_pairs(query: &Map<String, Value>) -> Vec<(&str, String)> {
    fn scalar(value: &Value) -> String {
        match value {
            Value::String(text) => text.clone(),
            Value::Null => String::new(),
            value => value.to_string(),
        }
    }

    let mut pairs = Vec::new();
    for (key, value) in query {
        if let Some(values) = value.as_array() {
            pairs.extend(values.iter().map(|value| (key.as_str(), scalar(value))));
        } else {
            pairs.push((key.as_str(), scalar(value)));
        }
    }
    pairs
}

fn id_path(prefix: &str, id: &str, suffix: &str) -> Result<String> {
    ensure!(
        !matches!(id, "" | "." | ".."),
        "Expected a nonempty API identifier"
    );
    let mut url = Url::parse("https://localhost").expect("valid placeholder URL");
    url.path_segments_mut()
        .expect("hierarchical URL")
        .clear()
        .push(id);
    Ok(format!("{prefix}{}{suffix}", url.path()))
}

async fn read_local(path: &str) -> Result<Vec<u8>> {
    let path = local::expand_path(path);
    tokio::task::spawn_blocking(move || local::read_file(&path)).await?
}

pub async fn call(app: &App, name: &str, args: &Value) -> Result<Value> {
    let cloud = Cloud {
        app,
        account: args["account_id"].as_str(),
    };
    match name {
        "suunto_list_workouts"
        | "suunto_get_workout"
        | "suunto_get_workout_fit"
        | "suunto_parse_workout_fit"
        | "suunto_get_complete_workout"
        | "suunto_add_workout_description_info"
        | "suunto_list_workouts_deprecated_v2"
        | "suunto_get_workout_deprecated_v2"
        | "suunto_get_workout_fit_deprecated_v2" => workouts::call(&cloud, name, args).await,
        "suunto_get_activity_samples"
        | "suunto_get_sleep_data"
        | "suunto_get_recovery_data"
        | "suunto_get_daily_activity_statistics"
        | "suunto_estimate_247_summary_calls"
        | "suunto_get_247_summary"
        | "suunto_get_legacy_daily_activity"
        | "suunto_get_legacy_daily_activity_statistics" => activity::call(&cloud, name, args).await,
        "suunto_init_workout_upload"
        | "suunto_upload_workout_fit_to_signed_url"
        | "suunto_get_upload_status"
        | "suunto_wait_for_upload_status"
        | "suunto_upload_workout_fit" => uploads::call(&cloud, name, args).await,
        "suunto_list_routes" => {
            let query = args_query(
                args,
                &[
                    ("page", "page"),
                    ("size", "size"),
                    ("since", "since"),
                    ("sort", "sort"),
                ],
            );
            cloud.get("/v2/route", merge_query(query, args)).await
        }
        "suunto_get_route" => {
            let id = required_str(args, "route_id")?;
            cloud.get(&id_path("/v2/route", id, "")?, Map::new()).await
        }
        "suunto_export_route_gpx" => {
            let id = required_str(args, "route_id")?;
            cloud
                .export(
                    &id_path("/v2/route", id, "/export")?,
                    &format!("suunto-route-{id}.gpx"),
                    GPX_CONTENT_TYPE,
                    Some(GPX_CONTENT_TYPE),
                    args,
                )
                .await
        }
        "suunto_parse_route_gpx" => {
            let id = required_str(args, "route_id")?;
            let (_, data) = cloud
                .get_bytes(
                    &id_path("/v2/route", id, "/export")?,
                    Map::new(),
                    Some(GPX_CONTENT_TYPE),
                )
                .await?;
            let args = args.clone();
            tokio::task::spawn_blocking(move || local::parse_gpx(&data, &args)).await?
        }
        "suunto_import_route_gpx" => import_route(&cloud, args).await,
        "suunto_list_guides" => cloud.get("/v2/guides/items", Map::new()).await,
        "suunto_get_guide" => {
            let id = required_str(args, "guide_id")?;
            cloud
                .export(
                    &id_path("/v2/guides/files", id, "")?,
                    &format!("suunto-guide-{id}.zip"),
                    ZIP_CONTENT_TYPE,
                    None,
                    args,
                )
                .await
        }
        "suunto_validate_guide_json" | "suunto_create_guide_zip" => {
            let args = args.clone();
            let name = name.to_owned();
            tokio::task::spawn_blocking(move || local::guide_call(&name, &args)).await?
        }
        "suunto_create_guide" | "suunto_update_guide" => write_guide(&cloud, name, args).await,
        "suunto_delete_guide" => {
            cloud.require_write()?;
            let id = required_str(args, "guide_id")?;
            cloud.delete(&id_path("/v2/guides/files", id, "")?).await
        }
        _ => bail!("Unknown tool {name}"),
    }
}

async fn import_route(cloud: &Cloud<'_>, args: &Value) -> Result<Value> {
    cloud.require_write()?;
    let data = read_local(required_str(args, "gpx_path")?).await?;
    let mut query = Map::new();
    if let Some(ids) = args["activity_ids"]
        .as_array()
        .filter(|ids| !ids.is_empty())
    {
        let activities = ids
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join(",");
        query.insert("activities".into(), json!(activities));
    }
    let (mime, body) = cloud
        .write_bytes(
            Method::POST,
            "/v2/route/import",
            &query,
            GPX_CONTENT_TYPE,
            data,
        )
        .await?;
    json_or_text(mime.as_deref(), &body, json!({"status":"uploaded"}))
}

async fn write_guide(cloud: &Cloud<'_>, name: &str, args: &Value) -> Result<Value> {
    cloud.require_write()?;
    let data = read_local(required_str(args, "zip_path")?).await?;
    let (method, path, fallback) = if name == "suunto_update_guide" {
        let id = required_str(args, "guide_id")?;
        (
            Method::PUT,
            id_path("/v2/guides/files", id, "")?,
            json!({"updated":true}),
        )
    } else {
        (
            Method::POST,
            "/v2/guides/files".into(),
            json!({"uploaded":true}),
        )
    };
    let (mime, body) = cloud
        .write_bytes(method, &path, &Map::new(), ZIP_CONTENT_TYPE, data)
        .await?;
    json_or_text(mime.as_deref(), &body, fallback)
}

fn json_or_text(mime: Option<&str>, body: &[u8], mut fallback: Value) -> Result<Value> {
    if mime.is_some_and(|mime| mime.contains("json")) {
        return Ok(serde_json::from_slice(body)?);
    }
    fallback["response_text"] = json!(String::from_utf8_lossy(body));
    Ok(fallback)
}

fn storage_client() -> Result<Client> {
    Ok(Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(60))
        .build()?)
}

#[cfg(test)]
mod tests;
