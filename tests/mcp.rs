use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};
use suunto_mcp::{App, config::Config, mcp::router};
use tower::ServiceExt;

const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"integration","version":"1"}}}"#;
const NOTIFY: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
const LIST: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;

fn server(dir: &std::path::Path, bearer: &str, sse: bool) -> Router {
    let app = App::new(
        Config::from_lookup(|key| match key {
            "SUUNTO_LOCAL_DATA_DIR" => Some(dir.to_string_lossy().into_owned()),
            "SUUNTO_TOKEN_STORE" | "SUUNTO_WEBHOOK_STORE" => Some("memory".into()),
            "SUUNTO_MCP_API_TOKEN" => Some(bearer.into()),
            "SUUNTO_MCP_MAX_SESSIONS" | "SUUNTO_MCP_SESSION_IDLE_SECONDS" => Some("1".into()),
            _ => None,
        })
        .unwrap(),
    )
    .unwrap();
    router(Arc::new(app), sse)
}

fn request(method: &str, path: &str, body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(body.into())
        .unwrap()
}

fn authorized(mut request: Request<Body>) -> Request<Body> {
    request
        .headers_mut()
        .insert("authorization", "Bearer private".parse().unwrap());
    request
}

fn session(id: &str, mut request: Request<Body>) -> Request<Body> {
    request
        .headers_mut()
        .insert("mcp-session-id", id.parse().unwrap());
    request
}

async fn body_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn initialize(app: &Router) -> String {
    let response = app
        .clone()
        .oneshot(authorized(request("POST", "/mcp", INIT)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let id = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let result = body_json(response).await;
    assert_eq!(result["result"]["protocolVersion"], "2025-11-25");
    assert!(result["result"]["capabilities"]["tools"].is_object());
    id
}

#[tokio::test(start_paused = true)]
async fn http_health_never_uses_session_capacity_and_idle_sessions_expire() {
    let dir = tempfile::tempdir().unwrap();
    let app = server(dir.path(), "private", false);
    // A session cap of one makes session allocation by any probe observable.
    for _ in 0..100 {
        let probe = app
            .clone()
            .oneshot(request("GET", "/health", ""))
            .await
            .unwrap();
        assert_eq!(probe.status(), StatusCode::OK);
        assert!(!probe.headers().contains_key("mcp-session-id"));
        assert_eq!(body_json(probe).await, json!({"status":"ok"}));
    }
    let id = initialize(&app).await;
    let probe = app
        .clone()
        .oneshot(request("GET", "/health", ""))
        .await
        .unwrap();
    assert_eq!(probe.status(), StatusCode::OK);
    let rejected = app
        .clone()
        .oneshot(authorized(request("POST", "/mcp", INIT)))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    tokio::time::advance(Duration::from_secs(2)).await;
    let gone = app
        .clone()
        .oneshot(authorized(session(&id, request("POST", "/mcp", LIST))))
        .await
        .unwrap();
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
    assert_ne!(initialize(&app).await, id);
}

#[tokio::test]
async fn streamable_http_authorization_tools_resources_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let app = server(dir.path(), "private", false);

    for invalid in [None, Some("Bearer incorrect"), Some("Basic private")] {
        let mut req = request("POST", "/mcp", INIT);
        if let Some(value) = invalid {
            req.headers_mut()
                .insert("authorization", value.parse().unwrap());
        }
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()["www-authenticate"], "Bearer");
    }
    let mut req = authorized(request("POST", "/mcp", INIT));
    req.headers_mut()
        .insert("origin", "https://elsewhere.example".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    let id = initialize(&app).await;
    let send = |body: String| {
        app.clone()
            .oneshot(authorized(session(&id, request("POST", "/mcp", body))))
    };
    assert_eq!(
        send(NOTIFY.into()).await.unwrap().status(),
        StatusCode::ACCEPTED
    );
    let list = body_json(send(LIST.into()).await.unwrap()).await;
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 60);

    let quota = json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"suunto_api_quota_status","arguments":{}}});
    let quota = body_json(send(quota.to_string()).await.unwrap()).await;
    assert_eq!(quota["result"]["structuredContent"]["remaining"], 200);
    let config = json!({"jsonrpc":"2.0","id":4,"method":"resources/read",
        "params":{"uri":"suunto://config"}});
    let resource = body_json(send(config.to_string()).await.unwrap()).await;
    let text = resource["result"]["contents"][0]["text"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(text).unwrap()["MCP_API_TOKEN"],
        "***"
    );

    let delete = app
        .clone()
        .oneshot(authorized(session(&id, request("DELETE", "/mcp", ""))))
        .await
        .unwrap();
    assert_eq!(delete.status(), StatusCode::OK);
    assert_eq!(
        send(LIST.into()).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    assert_ne!(initialize(&app).await, id);
}

#[tokio::test]
async fn http_rejects_oversized_requests_and_unknown_protocols() {
    let dir = tempfile::tempdir().unwrap();
    let app = server(dir.path(), "", false);
    let too_big = vec![b' '; 1024 * 1024 + 1];
    let response = app
        .clone()
        .oneshot(request("POST", "/mcp", too_big))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let mut req = request("POST", "/mcp", INIT);
    req.headers_mut()
        .insert("mcp-protocol-version", "1970-01-01".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );

    let mut req = request("POST", "/mcp", INIT);
    req.headers_mut()
        .insert("host", "untrusted.example".parse().unwrap());
    assert_eq!(
        app.oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn legacy_sse_exchanges_messages_and_removes_closed_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let app = server(dir.path(), "private", true);
    let dropped = app
        .clone()
        .oneshot(authorized(request("GET", "/sse", "")))
        .await
        .unwrap();
    let full = app
        .clone()
        .oneshot(authorized(request("GET", "/sse", "")))
        .await
        .unwrap();
    assert_eq!(full.status(), StatusCode::SERVICE_UNAVAILABLE);
    drop(dropped);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let mut stream = client
        .get(format!("{base}/sse"))
        .bearer_auth("private")
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    let chunk = stream.chunk().await.unwrap().unwrap();
    let endpoint = std::str::from_utf8(&chunk)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap();
    assert!(endpoint.starts_with("/messages/?session_id="));
    let response = client
        .post(format!("{base}{endpoint}"))
        .bearer_auth("private")
        .header("content-type", "application/json")
        .body(INIT)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let chunk = stream.chunk().await.unwrap().unwrap();
    assert!(
        std::str::from_utf8(&chunk)
            .unwrap()
            .contains("\"protocolVersion\":\"2025-11-25\"")
    );
    task.abort();
}

#[test]
fn stdio_process_exchanges_mcp_messages() {
    let dir = tempfile::tempdir().unwrap();
    let mut process = Command::new(env!("CARGO_BIN_EXE_suunto-mcp"))
        .current_dir(dir.path())
        .env("SUUNTO_TOKEN_STORE", "memory")
        .env("SUUNTO_WEBHOOK_STORE", "memory")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = process.stdin.take().unwrap();
    writeln!(input, "{INIT}\n{NOTIFY}\n{LIST}").unwrap();
    drop(input);
    let output = BufReader::new(process.stdout.take().unwrap());
    let messages: Vec<Value> = output
        .lines()
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect();
    assert!(process.wait().unwrap().success());
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["result"]["serverInfo"]["name"], "Suunto MCP");
    assert_eq!(messages[1]["result"]["tools"].as_array().unwrap().len(), 60);
}

#[test]
fn cli_rejects_unauthenticated_public_bind() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_suunto-mcp"))
        .current_dir(dir.path())
        .env_remove("SUUNTO_MCP_API_TOKEN")
        .args(["--transport", "http", "--host", "0.0.0.0", "--port", "0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("SUUNTO_MCP_API_TOKEN"));
}
