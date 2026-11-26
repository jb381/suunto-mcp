use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::{env, sync::Arc};
use suunto_mcp::{
    App,
    config::{Config, expand_path},
    mcp::router,
};
use tower::ServiceExt;

async fn rpc(router: &Router, message: &Value, session: Option<&str>) -> (Option<String>, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(message.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let session = response
        .headers()
        .get("mcp-session-id")
        .map(|header| header.to_str().unwrap().into());
    let bytes = to_bytes(response.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap();
    (session, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
#[ignore = "uses a real Suunto token and counts against the API quota"]
async fn list_workouts_read_only() {
    let mut config = Config::load().unwrap();
    if config.redacted()["LIVE_TESTS"] != true {
        eprintln!("Set SUUNTO_LIVE_TESTS=true to run this test");
        return;
    }
    let token_path = expand_path(
        &env::var("SUUNTO_LIVE_TOKEN_FILE")
            .unwrap_or_else(|_| ".suunto-mcp/oauth-test-token.json".into()),
    );
    assert!(token_path.is_file(), "Missing live test token file");
    config.token_store = "memory".into();
    config.webhook_store = "memory".into();
    config.bearer.clear();
    config.write_tools = false;
    let app = router(Arc::new(App::new(config).unwrap()), false);

    let (session, _) = rpc(&app, &json!({"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"live","version":"1"}}}), None).await;
    let (_, imported) = rpc(
        &app,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"suunto_import_token_file","arguments":{"path":token_path}}}),
        session.as_deref(),
    )
    .await;
    assert_eq!(imported["result"]["isError"], false);
    let (_, workouts) = rpc(
        &app,
        &json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"suunto_list_workouts","arguments":{"limit":1}}}),
        session.as_deref(),
    )
    .await;
    assert_eq!(workouts["result"]["isError"], false);
    let result = &workouts["result"]["structuredContent"];
    assert!(result["error"].is_null());
    assert!(result.get("payload").is_some());
}
