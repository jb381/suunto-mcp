use std::{net::IpAddr, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use reqwest::{
    Response, Url,
    header::{ETAG, HeaderMap, HeaderName, HeaderValue},
};
use serde_json::{Map, Value, json};

use super::{
    Cloud, RESPONSE_LIMIT, args_query, bool_or, id_path, read_local, required_str, storage_client,
};

const UPLOAD_PATH: &str = "/v2/upload";
const MAX_POLL_ATTEMPTS: i64 = 512;
const DEFAULT_TERMINAL: &[&str] = &["COMPLETED", "COMPLETE", "DONE", "FAILED", "ERROR"];

fn init_body(args: &Value) -> Value {
    Value::Object(args_query(
        args,
        &[
            ("description", "description"),
            ("comment", "comment"),
            ("notifyUser", "notify_user"),
            ("privacy", "privacy"),
        ],
    ))
}

fn redacted_url(value: &str) -> String {
    value.split_once('?').map_or_else(
        || value.to_owned(),
        |(base, _)| format!("{base}?<redacted-signature>"),
    )
}

fn redact_response(mut value: Value) -> Value {
    for key in ["url", "curlString"] {
        if let Some(url) = value[key].as_str() {
            value[key] = json!(redacted_url(url));
        }
    }
    value
}

fn upload_status(value: &Value) -> Option<&str> {
    ["status", "state", "uploadStatus", "processingStatus"]
        .iter()
        .find_map(|key| value[*key].as_str().filter(|status| !status.is_empty()))
}

fn is_allowed_hostname(host: &str) -> bool {
    if host == "storage.googleapis.com"
        || host.ends_with(".storage.googleapis.com")
        || host.ends_with(".blob.core.windows.net")
        || host.ends_with(".r2.cloudflarestorage.com")
    {
        return true;
    }
    host.strip_suffix(".amazonaws.com").is_some_and(|prefix| {
        prefix
            .split('.')
            .any(|label| label == "s3" || label.starts_with("s3-"))
    })
}

fn validate_signed_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).context("signed_url must be an absolute https:// URL.")?;
    ensure!(
        url.scheme() == "https",
        "signed_url must use https:// scheme."
    );
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "signed_url must not contain embedded credentials."
    );
    let host = url
        .host_str()
        .context("signed_url must have a valid hostname.")?;
    ensure!(host != "localhost", "signed_url must not target localhost.");
    ensure!(
        host.trim_matches(['[', ']']).parse::<IpAddr>().is_err(),
        "signed_url must target an allowed object-storage hostname, not an IP address."
    );
    ensure!(
        is_allowed_hostname(host),
        "signed_url hostname is not an allowed object-storage host."
    );
    Ok(url)
}

async fn send_signed(url: Url, path: &str, headers: &Value) -> Result<Response> {
    let mut request_headers = HeaderMap::new();
    if let Some(headers) = headers.as_object() {
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes())?;
            let value = value
                .as_str()
                .context("Signed upload header values must be strings")?;
            request_headers.insert(name, HeaderValue::from_str(value)?);
        }
    }
    let body = read_local(path).await?;
    storage_client()?
        .put(url)
        .headers(request_headers)
        .body(body)
        .send()
        .await
        .map_err(reqwest::Error::without_url)
        .context("Signed FIT upload request failed")
}

fn upload_result(response: &Response) -> Value {
    let status = response.status();
    json!({
        "status_code": status.as_u16(),
        "ok": status.is_success(),
        "etag": response.headers().get(ETAG).and_then(|value| value.to_str().ok()),
    })
}

async fn wait_for_status(
    cloud: &Cloud<'_>,
    id: &str,
    args: &Value,
    max_arg: &str,
) -> Result<Value> {
    let max_attempts = args[max_arg].as_i64().unwrap_or(12);
    ensure!(
        (1..=MAX_POLL_ATTEMPTS).contains(&max_attempts),
        "{max_arg} must be between 1 and {MAX_POLL_ATTEMPTS}."
    );
    let poll_interval = Duration::try_from_secs_f64(
        args["poll_interval_seconds"]
            .as_f64()
            .unwrap_or(5.0)
            .max(0.0),
    )
    .context("Invalid poll_interval_seconds")?;
    let terminal_statuses = args["terminal_statuses"]
        .as_array()
        .filter(|values| !values.is_empty())
        .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_else(|| DEFAULT_TERMINAL.to_vec());
    let path = id_path(UPLOAD_PATH, id, "")?;
    let mut seen = Vec::new();
    let mut terminal = false;
    let mut total_size = 0usize;
    for attempt in 1..=max_attempts {
        let (status, size) = cloud.get_json_with_size(&path, Map::new()).await?;
        total_size += size;
        ensure!(
            total_size <= RESPONSE_LIMIT,
            "Suunto upload status history exceeds {RESPONSE_LIMIT} bytes"
        );
        terminal = upload_status(&status).is_some_and(|value| {
            terminal_statuses
                .iter()
                .any(|status| value.eq_ignore_ascii_case(status))
        });
        seen.push(status);
        if terminal {
            break;
        }
        if attempt < max_attempts && !poll_interval.is_zero() {
            tokio::time::sleep(poll_interval).await;
        }
    }
    Ok(json!({
        "terminal": terminal,
        "attempt_count": seen.len(),
        "terminal_statuses": terminal_statuses,
        "final_status": seen.last(),
        "attempts": seen,
    }))
}

pub(super) async fn call(cloud: &Cloud<'_>, name: &str, args: &Value) -> Result<Value> {
    match name {
        "suunto_init_workout_upload" => {
            cloud.require_write()?;
            Ok(redact_response(
                cloud.post_json(UPLOAD_PATH, &init_body(args)).await?,
            ))
        }
        "suunto_upload_workout_fit_to_signed_url" => {
            cloud.require_write()?;
            let url = required_str(args, "signed_url")?;
            let response = send_signed(
                validate_signed_url(url)?,
                required_str(args, "fit_path")?,
                &args["headers"],
            )
            .await?;
            let mut result = upload_result(&response);
            result["content_md5"] = json!(
                response
                    .headers()
                    .get("content-md5")
                    .and_then(|v| v.to_str().ok())
            );
            result["signed_url"] = json!(redacted_url(url));
            Ok(result)
        }
        "suunto_get_upload_status" => {
            let id = required_str(args, "upload_id")?;
            cloud.get(&id_path(UPLOAD_PATH, id, "")?, Map::new()).await
        }
        "suunto_wait_for_upload_status" => {
            let id = required_str(args, "upload_id")?;
            wait_for_status(cloud, id, args, "max_attempts").await
        }
        "suunto_upload_workout_fit" => upload_fit(cloud, args).await,
        _ => bail!("Unknown upload tool {name}"),
    }
}

async fn upload_fit(cloud: &Cloud<'_>, args: &Value) -> Result<Value> {
    cloud.require_write()?;
    let initialized = cloud.post_json(UPLOAD_PATH, &init_body(args)).await?;
    ensure!(
        initialized.is_object(),
        "Upload initialization response was not an object."
    );
    let signed_url = initialized["url"].as_str();
    let method = initialized.get("method").map_or(Some("PUT"), Value::as_str);
    let url = signed_url
        .filter(|_| method == Some("PUT"))
        .context("Upload initialization did not return a PUT signed URL.")?;
    let response = send_signed(
        validate_signed_url(url)?,
        required_str(args, "fit_path")?,
        &initialized["headers"],
    )
    .await?;
    let uploaded = upload_result(&response);
    drop(response);
    let id = initialized["id"]
        .as_str()
        .filter(|_| bool_or(args, "fetch_status", true));
    let status = match id {
        Some(id) if bool_or(args, "wait_for_status", false) => {
            wait_for_status(cloud, id, args, "max_status_attempts").await?
        }
        Some(id) => {
            cloud
                .get(&id_path(UPLOAD_PATH, id, "")?, Map::new())
                .await?
        }
        None => Value::Null,
    };
    Ok(json!({
        "initialized": redact_response(initialized),
        "uploaded": uploaded,
        "status": status,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_signed_urls() {
        for value in [
            "https://s3.amazonaws.com/bucket/object?sig=abc",
            "https://bucket.s3.us-west-2.amazonaws.com/object?sig=abc",
            "https://account.blob.core.windows.net/container/object?sig=abc",
            "https://storage.googleapis.com/bucket/object?sig=abc",
            "https://bucket.storage.googleapis.com/object?sig=abc",
            "https://bucket.account.r2.cloudflarestorage.com/object?sig=abc",
        ] {
            assert!(validate_signed_url(value).is_ok(), "{value}");
        }
        for value in [
            "http://storage.googleapis.com/bucket/object",
            "https://localhost/upload",
            "https://127.0.0.1/upload",
            "https://[::1]/upload",
            "https://8.8.8.8/upload",
            "https://sub.storage.googleapis.com.example.com/upload",
            "https://user:pass@storage.googleapis.com/upload",
        ] {
            assert!(validate_signed_url(value).is_err(), "{value}");
        }
    }

    #[test]
    fn redaction_and_status_aliases() {
        let response = redact_response(json!({
            "id": "upload-1",
            "url": "https://s3.amazonaws.com/upload?signature=secret",
            "curlString": "curl https://s3.amazonaws.com/upload?signature=secret",
        }));
        assert_eq!(
            response["url"],
            "https://s3.amazonaws.com/upload?<redacted-signature>"
        );
        assert_eq!(
            response["curlString"],
            "curl https://s3.amazonaws.com/upload?<redacted-signature>"
        );
        assert_eq!(response["id"], "upload-1");
        assert_eq!(
            upload_status(&json!({"status":"","processingStatus":"DONE"})),
            Some("DONE")
        );
        assert_eq!(
            upload_status(&json!({"uploadStatus":"FAILED"})),
            Some("FAILED")
        );
    }
}
