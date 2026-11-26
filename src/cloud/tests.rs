use super::*;

#[test]
fn query_overrides_defaults_and_expands_arrays() {
    let args = json!({"from_date": "old", "limit": 11, "query_params": {
        "from": "new", "names": ["a", "b"], "flag": false, "limit": null
    }});
    let base = args_query(&args, &[("from", "from_date"), ("limit", "limit")]);
    let merged = merge_query(base, &args);
    let mut pairs = query_pairs(&merged);
    pairs.sort();
    assert_eq!(
        pairs,
        vec![
            ("flag", "false".into()),
            ("from", "new".into()),
            ("limit", "11".into()),
            ("names", "a".into()),
            ("names", "b".into())
        ]
    );
}

#[test]
fn identifiers_do_not_change_request_host_path_or_query_structure() -> Result<()> {
    assert_eq!(
        id_path("/v3/workouts", "a/b?x#y%z", "/fit")?,
        "/v3/workouts/a%2Fb%3Fx%23y%25z/fit"
    );
    assert!(id_path("/v3/workouts", "..", "").is_err());
    Ok(())
}

#[test]
fn upload_reply_keeps_json_and_plain_text_shapes() -> Result<()> {
    assert_eq!(
        json_or_text(
            Some("application/problem+json"),
            br#"{"id":7}"#,
            Value::Null
        )?,
        json!({"id":7})
    );
    assert_eq!(
        json_or_text(Some("text/plain"), b"done", json!({"uploaded":true}))?,
        json!({"uploaded":true,"response_text":"done"})
    );
    Ok(())
}

use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    response::{IntoResponse, Response as AxumResponse},
};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::config::Config;

#[derive(Default)]
struct MockState {
    requests: Mutex<Vec<(String, String)>>,
    upload_polls: Mutex<usize>,
}

async fn api(State(state): State<Arc<MockState>>, request: Request) -> AxumResponse {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path();
    let authorization = parts
        .headers
        .get("authorization")
        .and_then(|header| header.to_str().ok());
    state
        .requests
        .lock()
        .unwrap()
        .push((parts.method.to_string(), parts.uri.to_string()));

    if path == "/oauth/token" {
        assert_eq!(authorization, Some("Basic Y2xpZW50OnNlY3JldA=="));
        let body = to_bytes(body, 1024).await.unwrap();
        assert_eq!(body, "grant_type=refresh_token&refresh_token=refresh-1");
        return Json(json!({"access_token":"fresh","refresh_token":"refresh-2"})).into_response();
    }

    assert_eq!(
        parts.headers["ocp-apim-subscription-key"],
        "test-subscription"
    );
    if authorization == Some("Bearer stale") {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    assert_eq!(authorization, Some("Bearer fresh"));

    match (parts.method, path) {
        (Method::GET, "/v3/workouts") => Json(json!({"payload":[{"id":"r1"}]})).into_response(),
        (Method::GET, "/247samples/activity") => {
            assert_eq!(parts.uri.query(), Some("from=1767225600000&to=1767312000000"));
            Json(json!([{"steps":4500}])).into_response()
        }
        (Method::GET, "/247samples/daily-activity-statistics") => {
            assert_eq!(parts.uri.query(), Some("startdate=2026-01-01&enddate=2026-01-02"));
            Json(json!({"days":1})).into_response()
        }
        (Method::GET, "/v2/workouts/old/fit") => (
            [(CONTENT_TYPE, "application/fit")], Body::from("fit")
        ).into_response(),
        (Method::GET, "/v2/route/r1/export") => (
            [(CONTENT_TYPE, GPX_CONTENT_TYPE)],
            r#"<gpx version="1.1" creator="test"><trk><trkseg><trkpt lat="45" lon="9"/></trkseg></trk></gpx>"#,
        ).into_response(),
        (Method::POST, "/v2/route/import") => {
            assert_eq!(parts.uri.query(), Some("activities=1%2C2"));
            assert_eq!(parts.headers[CONTENT_TYPE], GPX_CONTENT_TYPE);
            assert_eq!(to_bytes(body, 1024).await.unwrap().as_ref(), b"gpx content");
            Json(json!({"id":"imported-route"})).into_response()
        }
        (Method::POST, "/v2/upload") => {
            let request: Value = serde_json::from_slice(&to_bytes(body, 1024).await.unwrap()).unwrap();
            if request["description"] == "untrusted-location" {
                return Json(json!({"id":"up-1","url":"https://example.com/object?secret=one"})).into_response();
            }
            assert_eq!(request, json!({"notifyUser":false,"privacy":"PRIVATE"}));
            Json(json!({"id":"up-1","url":"https://s3.amazonaws.com/object?secret=one"})).into_response()
        }
        (Method::GET, "/v2/upload/up-1") => {
            let mut count = state.upload_polls.lock().unwrap();
            *count += 1;
            if *count == 1 {
                Json(json!({"processingStatus":"pending"})).into_response()
            } else {
                Json(json!({"state":"completed"})).into_response()
            }
        }
        (Method::DELETE, "/v2/guides/files/obsolete") => StatusCode::NO_CONTENT.into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn fixture(
    write_tools: bool,
    limit: i64,
) -> Result<(App, Arc<MockState>, tempfile::TempDir, JoinHandle<()>)> {
    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let base = format!("http://{}", listener.local_addr()?);
    let state = Arc::<MockState>::default();
    let router = Router::new().fallback(api).with_state(state.clone());
    let mock = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let config = Config::from_lookup(|key| {
        Some(match key {
            "SUUNTO_API_BASE" | "SUUNTO_OAUTH_BASE" => base.clone(),
            "SUUNTO_SUBSCRIPTION_KEY" => "test-subscription".into(),
            "SUUNTO_CLIENT_ID" => "client".into(),
            "SUUNTO_CLIENT_SECRET" => "secret".into(),
            "SUUNTO_TOKEN_STORE" | "SUUNTO_WEBHOOK_STORE" => "memory".into(),
            "SUUNTO_ENABLE_WRITE_TOOLS" => write_tools.to_string(),
            "SUUNTO_API_CALLS_PER_MINUTE" => "0".into(),
            "SUUNTO_API_WEEKLY_CALL_LIMIT" => limit.to_string(),
            "SUUNTO_LOCAL_DATA_DIR" => directory.path().join("data").to_string_lossy().into_owned(),
            _ => return None,
        })
    })?;
    Ok((App::new(config)?, state, directory, mock))
}

async fn import_account(app: &App, access_token: &str) -> Result<()> {
    crate::stores::call(
        app,
        "suunto_import_token_set",
        &json!({
            "account_id": "runner", "access_token": access_token, "refresh_token": "refresh-1",
        }),
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn cloud_retries_rejected_token_once_and_counts_each_api_request() -> Result<()> {
    let (app, state, _directory, mock) = fixture(false, 10).await?;
    import_account(&app, "stale").await?;
    let result = call(
        &app,
        "suunto_list_workouts",
        &json!({
            "account_id":"runner", "limit":2,"query_params":{"limit":5}
        }),
    )
    .await?;
    assert_eq!(result, json!({"payload":[{"id":"r1"}]}));
    assert_eq!(app.quota.status()?["used"], 2);
    assert_eq!(
        app.record(None, None).await?.refresh_token.as_deref(),
        Some("refresh-2")
    );

    let result = call(
        &app,
        "suunto_get_workout_fit_deprecated_v2",
        &json!({
            "workout_key":"old","output_mode":"base64"
        }),
    )
    .await?;
    assert_eq!(result["filename"], "suunto-workout-v2-old.fit");
    assert_eq!(result["content_type"], "application/fit");
    assert_eq!(result["base64"], "Zml0");
    assert_eq!(
        *state.requests.lock().unwrap(),
        [
            ("GET".into(), "/v3/workouts?limit=5".into()),
            ("POST".into(), "/oauth/token".into()),
            ("GET".into(), "/v3/workouts?limit=5".into()),
            ("GET".into(), "/v2/workouts/old/fit".into()),
        ]
    );
    mock.abort();
    Ok(())
}

#[tokio::test]
async fn route_upload_poll_and_guide_delete_preserve_tool_shapes() -> Result<()> {
    let (app, _, directory, mock) = fixture(true, 10).await?;
    import_account(&app, "fresh").await?;
    let route = call(&app, "suunto_parse_route_gpx", &json!({"route_id":"r1"})).await?;
    assert_eq!(route["track_count"], 1);
    let file = directory.path().join("route.gpx");
    std::fs::write(&file, b"gpx content")?;
    let result = call(
        &app,
        "suunto_import_route_gpx",
        &json!({
            "gpx_path":file, "activity_ids":[1,2]
        }),
    )
    .await?;
    assert_eq!(result["id"], "imported-route");

    let result = call(
        &app,
        "suunto_init_workout_upload",
        &json!({
            "notify_user": false, "privacy": "PRIVATE",
        }),
    )
    .await?;
    assert_eq!(
        result["url"],
        "https://s3.amazonaws.com/object?<redacted-signature>"
    );
    let result = call(
        &app,
        "suunto_wait_for_upload_status",
        &json!({
            "upload_id":"up-1", "terminal_statuses":["COMPLETED"], "poll_interval_seconds":0.0
        }),
    )
    .await?;
    assert_eq!(result["terminal"], true);
    assert_eq!(result["attempt_count"], 2);
    assert_eq!(result["final_status"], json!({"state":"completed"}));

    let result = call(&app, "suunto_delete_guide", &json!({"guide_id":"obsolete"})).await?;
    assert_eq!(result, json!({"ok":true}));
    mock.abort();
    Ok(())
}

#[tokio::test]
async fn quota_preflight_and_write_gate_prevent_upstream_requests() -> Result<()> {
    let (app, state, _directory, mock) = fixture(false, 1).await?;
    let options = json!({
        "from_value":"2026-01-01T00:00:00Z","to_value":"2026-01-02T00:00:00Z",
        "include_sleep":false,"include_recovery":false
    });
    let estimate = call(&app, "suunto_estimate_247_summary_calls", &options).await?;
    assert_eq!(estimate["call_count"], 2);
    assert_eq!(estimate["would_exceed_quota"], true);
    let error = call(&app, "suunto_get_247_summary", &options)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("would require 2 API calls"));
    let error = call(
        &app,
        "suunto_import_route_gpx",
        &json!({"gpx_path":"unused"}),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("SUUNTO_ENABLE_WRITE_TOOLS"));
    assert_eq!(app.quota.status()?["used"], 0);
    assert!(state.requests.lock().unwrap().is_empty());
    mock.abort();
    Ok(())
}

#[tokio::test]
async fn activity_summary_fetches_samples_and_daily_statistics() -> Result<()> {
    let (app, _, _directory, mock) = fixture(false, 10).await?;
    import_account(&app, "fresh").await?;
    let result = call(
        &app,
        "suunto_get_247_summary",
        &json!({
            "from_value":"2026-01-01T00:00:00Z","to_value":"2026-01-02T00:00:00Z",
            "include_sleep":false,"include_recovery":false,
        }),
    )
    .await?;
    assert_eq!(result["call_count"], 2);
    assert_eq!(result["quota"]["used"], 0);
    assert_eq!(
        result["results"],
        json!({
            "activity":[[{"steps":4500}]], "sleep":[], "recovery":[],
            "daily_activity_statistics":[{"days":1}],
        })
    );
    assert_eq!(app.quota.status()?["used"], 2);
    mock.abort();
    Ok(())
}

#[tokio::test]
async fn combined_upload_rejects_untrusted_destination_before_reading_file() -> Result<()> {
    let (app, state, _directory, mock) = fixture(true, 10).await?;
    import_account(&app, "fresh").await?;
    let result = call(
        &app,
        "suunto_upload_workout_fit",
        &json!({
            "fit_path":"unreadable-file.fit","description":"untrusted-location"
        }),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(result.contains("allowed object-storage host"));
    assert!(!result.contains("secret=one"));
    assert_eq!(state.requests.lock().unwrap().len(), 1);
    mock.abort();
    Ok(())
}
